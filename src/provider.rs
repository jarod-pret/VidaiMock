/*
 * Copyright (c) 2025 Vidai UK.
 * Author: n@gu
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 *
 * VidaiMock: High-performance LLM API Mock Server.
 */

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::fs;
use glob::glob;
use regex::Regex;
use tera::Tera;
use rand::Rng; // For random functions
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "config/"]
pub struct Asset;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    /// Regex pattern to match the request path (e.g., "^/v1/chat/completions$")
    pub matcher: String,
    /// Mapping of internal variable names to template expressions
    /// e.g. "prompt": "{{ json.messages | last | get('content') }}"
    #[serde(default)]
    pub request_mapping: HashMap<String, String>,
    /// Path to the detailed response template file (relative to config/templates/)
    pub response_template: Option<String>,
    /// Inline response template string (optional override)
    pub response_body: Option<String>,
    #[serde(default)]
    pub stream: Option<ProviderStreamConfig>,
    /// HTTP status code to return (default: 200). Supports static codes ("400")
    /// or Tera expressions ("{{ path_segments | last }}") for dynamic extraction.
    pub status_code: Option<String>,
    /// Optional template rendered when a chaos/failure override triggers an error
    /// response (e.g. `?chaos_status=500` query param, `X-Vidai-Chaos-Drop`).
    /// Falls back to the standard `response_template` when absent.
    /// Lets providers supply provider-shaped error envelopes
    /// (OpenAI: `{"error": {...}}`, Anthropic: `{"type": "error", ...}`, etc.).
    pub error_template: Option<String>,
    /// Priority for matching (higher matches first)
    #[serde(default)]
    pub priority: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderStreamConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Default format string for data tokens if no detailed lifecycle is provided
    /// e.g. "data: {{ json }}\n\n"
    pub format: Option<String>,
    /// Encoding for the stream (e.g. "aws-event-stream"). Default is SSE.
    pub encoding: Option<String>,
    /// Controls SSE frame wrapping. Default "sse" auto-prefixes each chunk with "data: ".
    /// Set to "raw" to emit template output verbatim — the template controls all framing
    /// including event: and data: lines. Required for providers with typed SSE events.
    pub frame_format: Option<String>,
    /// Detailed lifecycle events for complex streams (Anthropic, etc.)
    pub lifecycle: Option<StreamLifecycle>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamLifecycle {
    pub on_start: Option<StreamEvent>,
    pub on_chunk: Option<StreamEvent>,
    pub on_stop: Option<StreamEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEvent {
    pub event_name: Option<String>, // e.g., "message_start"
    pub template_path: Option<String>,
    pub template_body: Option<String>,
}

pub struct ProviderRegistry {
    pub providers: Vec<ProviderConfig>,
    compiled_matchers: Vec<(Regex, usize)>, // (Regex, index in providers)
    pub tera: Arc<Tera>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
            compiled_matchers: Vec::new(),
            tera: Arc::new(Tera::default()),
        }
    }

    pub fn load_from_dir(&mut self, config_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let providers_pattern = config_dir.join("providers/*.yaml");
        // let templates_pattern = config_dir.join("templates/**/*");

        // 1. Load Templates
        // Setup Tera with all templates (embedded + disk)
        let mut tera = Tera::default();
        
        // Disable autoescape for JSON generation
        tera.autoescape_on(vec![]);

        // Register custom functions
        tera.register_function("uuid", |_args: &HashMap<String, tera::Value>| -> tera::Result<tera::Value> {
            Ok(tera::Value::String(uuid::Uuid::new_v4().to_string()))
        });

        tera.register_function("timestamp", |_args: &HashMap<String, tera::Value>| -> tera::Result<tera::Value> {
             let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
             Ok(tera::Value::Number(serde_json::Number::from(now)))
        });

        tera.register_function("iso_timestamp", |_args: &HashMap<String, tera::Value>| -> tera::Result<tera::Value> {
             Ok(tera::Value::String(chrono::Utc::now().to_rfc3339()))
        });

        tera.register_function("random_int", |args: &HashMap<String, tera::Value>| -> tera::Result<tera::Value> {
             let min = args.get("min").and_then(|v| v.as_i64()).unwrap_or(0);
             let max = args.get("max").and_then(|v| v.as_i64()).unwrap_or(100);
             let val = rand::rng().random_range(min..=max);
             Ok(tera::Value::Number(serde_json::Number::from(val)))
        });
        
        tera.register_function("random_float", |args: &HashMap<String, tera::Value>| -> tera::Result<tera::Value> {
             let min = args.get("min").and_then(|v| v.as_f64()).unwrap_or(0.0);
             let max = args.get("max").and_then(|v| v.as_f64()).unwrap_or(1.0);
             let val = rand::rng().random_range(min..=max);
             Ok(tera::Value::from(val))
        });

        let pick_filter = |value: &tera::Value, args: &HashMap<String, tera::Value>| -> tera::Result<tera::Value> {
             let key = args.get("key").and_then(|v| v.as_str())
                 .or_else(|| args.get("0").and_then(|v| v.as_str()))
                 .ok_or_else(|| tera::Error::msg("Filter 'get' or 'pick' requires a 'key' argument"))?;
             match value {
                 tera::Value::Object(map) => {
                     Ok(map.get(key).cloned().unwrap_or(tera::Value::Null))
                 },
                 _ => Ok(tera::Value::Null)
             }
        };

        tera.register_filter("pick", pick_filter);
        tera.register_filter("get", pick_filter);
        
        tera.register_filter("minify", |value: &tera::Value, _args: &HashMap<String, tera::Value>| -> tera::Result<tera::Value> {
            match value {
                tera::Value::String(s) => {
                    // Simple minification: parse as JSON and re-serialize to compact string
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(s) {
                        Ok(tera::Value::String(serde_json::to_string(&json).unwrap()))
                    } else {
                        // Fallback to basic whitespace removal if not valid JSON
                        Ok(tera::Value::String(s.lines().map(|line| line.trim()).collect::<Vec<_>>().join("")))
                    }
                },
                _ => Ok(value.clone())
            }
        });

        // a. Collect all templates (embedded first, then disk for overrides)
        let mut template_map = HashMap::new();

        // 1. Embedded templates
        for file in Asset::iter() {
            if file.starts_with("templates/") {
                if let Some(content) = Asset::get(&file) {
                    let template_str = std::str::from_utf8(content.data.as_ref())?.to_string();
                    let name = file["templates/".len()..].to_string();
                    tracing::debug!("Found embedded template: {}", name);
                    template_map.insert(name, template_str);
                }
            }
        }

        // 2. Disk templates (overrides embedded)
        let templates_glob = config_dir.join("templates/**/*");
        if let Ok(entries) = glob(templates_glob.to_str().ok_or("Invalid template path")?) {
            for entry in entries {
                if let Ok(path) = entry {
                    if path.is_file() {
                         let content = fs::read_to_string(&path)?;
                         let rel_path = path.strip_prefix(config_dir.join("templates/"))?;
                         let name = rel_path.to_str().ok_or("Invalid path")?.to_string();
                         template_map.insert(name.clone(), content);
                         tracing::debug!("Found disk template override: {}", name);
                    }
                }
            }
        }

        // b. Add all collected templates to Tera
        for (name, content) in template_map {
            // Register with the relative name (e.g., openai/chat.json.j2)
            if let Err(e) = tera.add_raw_template(&name, &content) {
                 tracing::error!("Failed to parse template '{}': {:?}", name, e);
            } else {
                 tracing::debug!("Registered template: {}", name);
            }
            
            // Also register with config/templates/ prefix for compatibility with some provider configs
            let full_name = format!("config/templates/{}", name);
            if let Err(_) = tera.add_raw_template(&full_name, &content) {
                // Ignore errors for the alias if the template already failed or something
            }
        }

        // 2. Load Providers
        // a. Load embedded and disk providers with shadowing
        let mut loaded_provider_names = std::collections::HashSet::new();
        
        // 2. Load providers into a temporary list for sorting
        let mut all_configs = Vec::new();

        // 1. Load disk providers
        if let Ok(entries) = glob(providers_pattern.to_str().unwrap()) {
            for entry in entries {
                if let Ok(path) = entry {
                    if path.is_file() {
                        let content = fs::read_to_string(&path)?;
                        if let Ok(config) = serde_yaml::from_str::<ProviderConfig>(&content) {
                            tracing::debug!("Discovered disk provider: {} ({})", config.name, path.display());
                            all_configs.push(config);
                            loaded_provider_names.insert(format!("providers/{}", path.file_name().unwrap().to_str().unwrap()));
                        }
                    }
                }
            }
        }

        // 2. Load embedded providers that were NOT on disk
        for file in Asset::iter() {
            if file.starts_with("providers/") && file.ends_with(".yaml") && !loaded_provider_names.contains(file.as_ref()) {
                if let Some(content) = Asset::get(&file) {
                    let config_str = std::str::from_utf8(content.data.as_ref())?;
                    if let Ok(config) = serde_yaml::from_str::<ProviderConfig>(config_str) {
                        tracing::debug!("Discovered embedded provider: {}", config.name);
                        all_configs.push(config);
                    }
                }
            }
        }

        // 3. Sort by priority (descending)
        all_configs.sort_by(|a, b| b.priority.cmp(&a.priority));

        // 4. Register sorted providers
        self.providers.clear();
        self.compiled_matchers.clear();
        for config in all_configs {
            let regex = Regex::new(&config.matcher)?;
            self.compiled_matchers.push((regex, self.providers.len()));
            self.providers.push(config);
        }

        if self.providers.is_empty() {
            tracing::warn!("No providers found in configuration directory: {}", config_dir.display());
        } else {
            tracing::info!("Registered {} providers (Disk + Embedded)", self.providers.len());
        }
        
        self.tera = Arc::new(tera);
        Ok(())
    }

    pub fn find_provider(&self, path: &str) -> Option<&ProviderConfig> {
        for (regex, idx) in &self.compiled_matchers {
            if regex.is_match(path) {
                return Some(&self.providers[*idx]);
            }
        }
        None
    }

    pub fn add_provider(&mut self, config: ProviderConfig) -> Result<(), Box<dyn std::error::Error>> {
        let regex = Regex::new(&config.matcher)?;
        self.compiled_matchers.push((regex, self.providers.len()));
        self.providers.push(config);
        Ok(())
    }

    /// Renders an ad-hoc string using the registered Tera instance
    pub fn render_str(&self, template: &str, context: &tera::Context) -> tera::Result<String> {
        tera::Tera::one_off(template, context, false)
    }
}

pub fn init_registry(config_path: &Path) -> Arc<ProviderRegistry> {
    let mut registry = ProviderRegistry::new();
    if let Err(e) = registry.load_from_dir(config_path) {
        tracing::error!("Failed to load provider registry: {}", e);
    }
    Arc::new(registry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn test_embedded_assets_load() {
        let mut registry = ProviderRegistry::new();
        // Load from a non-existent directory to ensure it only loads embedded
        registry.load_from_dir(&PathBuf::from("non_existent_dir")).unwrap();
        
        // Check if openai provider is in the list by name
        let has_openai = registry.providers.iter().any(|p| p.name == "openai");
        assert!(has_openai, "Embedded 'openai' provider should be loaded");
    }

    #[test]
    fn test_disk_shadowing() {
        // Use a unique subdir for this test to avoid conflicts
        let temp_base = std::env::current_dir().unwrap().join("target/test_shadowing_unique");
        if temp_base.exists() {
            fs::remove_dir_all(&temp_base).unwrap();
        }
        fs::create_dir_all(temp_base.join("providers")).unwrap();
        
        // We shadow openai.yaml
        let custom_provider = r#"
name: "custom-openai"
matcher: "^/v1/custom/chat/completions$"
response_template: "openai/chat.json.j2"
"#;
        fs::write(temp_base.join("providers/openai.yaml"), custom_provider).unwrap();

        let mut registry = ProviderRegistry::new();
        registry.load_from_dir(&temp_base).unwrap();

        // The disk-based "openai.yaml" should have been loaded instead of embedded "openai.yaml"
        // Let's find the provider that matches our custom matcher
        let provider = registry.find_provider("/v1/custom/chat/completions");
        assert!(provider.is_some(), "Disk provider should be matched");
        assert_eq!(provider.unwrap().name, "custom-openai");

        // ALSO check that the embedded "openai" provider is NOT loaded because it was shadowed by filename
        let embedded_openai = registry.providers.iter().find(|p| p.name == "openai");
        assert!(embedded_openai.is_none(), "Shadowed embedded provider should NOT be loaded");

        // Clean up
        fs::remove_dir_all(temp_base).unwrap();
    }
}
