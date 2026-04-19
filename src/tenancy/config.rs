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
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_TENANT_ID: &str = "default";

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum TenancyMode {
    #[default]
    Single,
    Multi,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq, Eq)]
pub struct TenancyConfig {
    #[serde(default)]
    pub mode: TenancyMode,
    #[serde(default = "default_tenants_dir")]
    pub tenants_dir: PathBuf,
    #[serde(default = "default_tenant_header")]
    pub tenant_header: String,
    #[serde(default)]
    pub tenants: Vec<TenantConfig>,
}

fn default_tenants_dir() -> PathBuf {
    PathBuf::from("tenants")
}

fn default_tenant_header() -> String {
    "x-tenant".to_string()
}

#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq, Eq)]
pub struct TenantConfig {
    pub id: String,
    #[serde(default)]
    pub keys: Vec<TenantKeyConfig>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq, Eq, Hash)]
pub struct TenantKeyConfig {
    pub source: TenantKeySource,
    pub name: String,
    #[serde(default, skip_serializing)]
    pub value: String,
    #[serde(default, skip_serializing)]
    pub value_file: Option<PathBuf>,
    #[serde(default, skip_serializing)]
    pub value_env: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum TenantKeySource {
    #[default]
    Header,
    Query,
    Host,
    Path,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantSchema {
    pub tenant_id: Option<String>,
    pub root_dir: PathBuf,
}

impl TenancyConfig {
    pub fn validate(&self) -> Result<(), String> {
        self.validate_duplicate_tenant_ids()?;
        self.validate_lookup_uniqueness()?;
        Ok(())
    }

    pub fn schema_roots(&self, single_config_dir: &Path) -> Vec<TenantSchema> {
        match self.mode {
            TenancyMode::Single => vec![TenantSchema {
                tenant_id: None,
                root_dir: single_config_dir.to_path_buf(),
            }],
            TenancyMode::Multi => {
                let mut schemas = vec![TenantSchema {
                    tenant_id: Some(DEFAULT_TENANT_ID.to_string()),
                    root_dir: self.tenants_dir.join(DEFAULT_TENANT_ID),
                }];
                schemas.extend(self.tenants.iter().map(|tenant| TenantSchema {
                    tenant_id: Some(tenant.id.clone()),
                    root_dir: self.tenants_dir.join(&tenant.id),
                }));
                schemas
            }
        }
    }

    pub fn normalized_tenant_header(&self) -> String {
        normalize_name(&self.tenant_header)
    }

    fn validate_duplicate_tenant_ids(&self) -> Result<(), String> {
        let mut seen = HashSet::new();

        for tenant in &self.tenants {
            let normalized_id = tenant.id.trim();
            if normalized_id.is_empty() {
                return Err("tenancy.tenants entries must include a non-empty id".to_string());
            }

            if normalize_header_value(normalized_id) == DEFAULT_TENANT_ID {
                return Err(format!(
                    "tenancy tenant id '{normalized_id}' is reserved for the internal default tenant"
                ));
            }

            if !seen.insert(normalize_header_value(normalized_id)) {
                return Err(format!("duplicate tenancy tenant id: '{normalized_id}'"));
            }
        }

        Ok(())
    }

    fn validate_lookup_uniqueness(&self) -> Result<(), String> {
        let tenant_header = self.normalized_tenant_header();
        let mut seen_header_values: HashMap<NormalizedLookupKey, String> = HashMap::new();
        let mut seen_api_keys: HashMap<NormalizedLookupKey, String> = HashMap::new();

        for tenant in &self.tenants {
            let implicit_header = NormalizedLookupKey {
                source: TenantKeySource::Header,
                name: tenant_header.clone(),
                value: normalize_header_value(&tenant.id),
            };
            register_lookup(
                &mut seen_header_values,
                implicit_header,
                tenant,
                format!("Header '{}'='{}'", self.tenant_header, tenant.id),
            )?;

            for key in &tenant.keys {
                if !key.supports_runtime_resolution(&tenant_header) {
                    return Err(format!(
                        "unsupported tenant key source {:?} for tenant '{}'",
                        key.source, tenant.id
                    ));
                }

                let resolved_value = key.resolved_value()?;
                let normalized = if key.is_tenant_header_key(&tenant_header) {
                    NormalizedLookupKey {
                        source: TenantKeySource::Header,
                        name: tenant_header.clone(),
                        value: normalize_header_value(&resolved_value),
                    }
                } else {
                    NormalizedLookupKey {
                        source: key.source.clone(),
                        name: normalize_name(&key.name),
                        value: normalize_secret_value(&resolved_value),
                    }
                };

                if key.is_tenant_header_key(&tenant_header) {
                    register_lookup(
                        &mut seen_header_values,
                        normalized,
                        tenant,
                        key.describe(&resolved_value),
                    )?;
                } else {
                    register_lookup(
                        &mut seen_api_keys,
                        normalized,
                        tenant,
                        key.describe(&resolved_value),
                    )?;
                }
            }
        }

        Ok(())
    }
}

impl TenantConfig {
    pub fn requires_key(&self, tenant_header: &str) -> bool {
        self.keys
            .iter()
            .any(|key| !key.is_tenant_header_key(tenant_header))
    }

    pub fn explicit_header_values(&self, tenant_header: &str) -> Result<Vec<String>, String> {
        let mut values = Vec::new();

        for key in &self.keys {
            if key.is_tenant_header_key(tenant_header) {
                values.push(key.resolved_value()?);
            }
        }

        Ok(values)
    }

    pub fn api_keys(&self, tenant_header: &str) -> Result<Vec<ResolvedTenantKey>, String> {
        let mut keys = Vec::new();

        for key in &self.keys {
            if key.is_tenant_header_key(tenant_header) {
                continue;
            }

            keys.push(ResolvedTenantKey {
                source: key.source.clone(),
                name: normalize_name(&key.name),
                value: normalize_secret_value(&key.resolved_value()?),
            });
        }

        Ok(keys)
    }
}

impl TenantKeyConfig {
    pub fn supports_runtime_resolution(&self, tenant_header: &str) -> bool {
        matches!(
            self.source,
            TenantKeySource::Header | TenantKeySource::Query
        ) || self.is_tenant_header_key(tenant_header)
    }

    pub fn is_tenant_header_key(&self, tenant_header: &str) -> bool {
        matches!(self.source, TenantKeySource::Header)
            && normalize_name(&self.name) == normalize_name(tenant_header)
    }

    pub fn resolved_value(&self) -> Result<String, String> {
        let value = if let Some(path) = &self.value_file {
            fs::read_to_string(path).map_err(|error| {
                format!(
                    "failed to read tenant secret file '{}': {}",
                    path.display(),
                    error
                )
            })?
        } else if let Some(env_name) = &self.value_env {
            std::env::var(env_name).map_err(|error| {
                format!(
                    "failed to read tenant secret env '{}' : {}",
                    env_name, error
                )
            })?
        } else {
            self.value.clone()
        };

        let trimmed = value.trim().to_string();
        if trimmed.is_empty() {
            return Err(format!(
                "tenant key '{}' from {:?} must resolve to a non-empty value",
                self.name, self.source
            ));
        }

        Ok(trimmed)
    }

    fn describe(&self, resolved_value: &str) -> String {
        format!("{:?} '{}={}'", self.source, self.name, resolved_value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedTenantKey {
    pub source: TenantKeySource,
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct NormalizedLookupKey {
    source: TenantKeySource,
    name: String,
    value: String,
}

fn register_lookup(
    seen: &mut HashMap<NormalizedLookupKey, String>,
    normalized: NormalizedLookupKey,
    tenant: &TenantConfig,
    description: String,
) -> Result<(), String> {
    if let Some(existing_tenant_id) = seen.get(&normalized) {
        if existing_tenant_id == &tenant.id {
            return Err(format!(
                "duplicate tenant key definition for tenant '{}' on {}",
                tenant.id, description
            ));
        }

        return Err(format!(
            "ambiguous tenant key match between '{}' and '{}' on {}",
            existing_tenant_id, tenant.id, description
        ));
    }

    seen.insert(normalized, tenant.id.clone());
    Ok(())
}

fn normalize_name(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

fn normalize_header_value(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn normalize_secret_value(value: &str) -> String {
    value.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_mode_uses_config_dir_schema() {
        let tenancy = TenancyConfig::default();
        let schemas = tenancy.schema_roots(Path::new("config"));

        assert_eq!(
            schemas,
            vec![TenantSchema {
                tenant_id: None,
                root_dir: PathBuf::from("config"),
            }]
        );
    }

    #[test]
    fn multi_mode_uses_tenants_dir_schema_with_internal_default() {
        let tenancy = TenancyConfig {
            mode: TenancyMode::Multi,
            tenants_dir: PathBuf::from("tenants"),
            tenant_header: default_tenant_header(),
            tenants: vec![
                TenantConfig {
                    id: "acme".to_string(),
                    keys: Vec::new(),
                },
                TenantConfig {
                    id: "globex".to_string(),
                    keys: Vec::new(),
                },
            ],
        };

        let schemas = tenancy.schema_roots(Path::new("config"));

        assert_eq!(
            schemas,
            vec![
                TenantSchema {
                    tenant_id: Some(DEFAULT_TENANT_ID.to_string()),
                    root_dir: PathBuf::from("tenants/default"),
                },
                TenantSchema {
                    tenant_id: Some("acme".to_string()),
                    root_dir: PathBuf::from("tenants/acme"),
                },
                TenantSchema {
                    tenant_id: Some("globex".to_string()),
                    root_dir: PathBuf::from("tenants/globex"),
                },
            ]
        );
    }

    #[test]
    fn duplicate_tenant_ids_are_rejected() {
        let tenancy = TenancyConfig {
            mode: TenancyMode::Multi,
            tenants_dir: PathBuf::from("tenants"),
            tenant_header: default_tenant_header(),
            tenants: vec![
                TenantConfig {
                    id: "acme".to_string(),
                    keys: Vec::new(),
                },
                TenantConfig {
                    id: "acme".to_string(),
                    keys: Vec::new(),
                },
            ],
        };

        let error = tenancy.validate().unwrap_err();
        assert!(error.contains("duplicate tenancy tenant id"));
    }

    #[test]
    fn ambiguous_tenant_keys_are_rejected() {
        let tenancy = TenancyConfig {
            mode: TenancyMode::Multi,
            tenants_dir: PathBuf::from("tenants"),
            tenant_header: default_tenant_header(),
            tenants: vec![
                TenantConfig {
                    id: "acme".to_string(),
                    keys: vec![TenantKeyConfig {
                        source: TenantKeySource::Header,
                        name: "X-API-Key".to_string(),
                        value: "shared".to_string(),
                        value_file: None,
                        value_env: None,
                    }],
                },
                TenantConfig {
                    id: "globex".to_string(),
                    keys: vec![TenantKeyConfig {
                        source: TenantKeySource::Header,
                        name: "x-api-key".to_string(),
                        value: "shared".to_string(),
                        value_file: None,
                        value_env: None,
                    }],
                },
            ],
        };

        let error = tenancy.validate().unwrap_err();
        assert!(error.contains("ambiguous tenant key match"));
    }

    #[test]
    fn implicit_and_explicit_header_alias_conflicts_are_rejected() {
        let tenancy = TenancyConfig {
            mode: TenancyMode::Multi,
            tenants_dir: PathBuf::from("tenants"),
            tenant_header: default_tenant_header(),
            tenants: vec![
                TenantConfig {
                    id: "acme".to_string(),
                    keys: Vec::new(),
                },
                TenantConfig {
                    id: "globex".to_string(),
                    keys: vec![TenantKeyConfig {
                        source: TenantKeySource::Header,
                        name: "x-tenant".to_string(),
                        value: "acme".to_string(),
                        value_file: None,
                        value_env: None,
                    }],
                },
            ],
        };

        let error = tenancy.validate().unwrap_err();
        assert!(error.contains("ambiguous tenant key match"));
    }
}
