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
use std::path::{Path, PathBuf};

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
    #[serde(default)]
    pub tenants: Vec<TenantConfig>,
}

fn default_tenants_dir() -> PathBuf {
    PathBuf::from("tenants")
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
pub struct TenantConfig {
    pub id: String,
    #[serde(default)]
    pub keys: Vec<TenantKeyConfig>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Hash)]
pub struct TenantKeyConfig {
    pub source: TenantKeySource,
    pub name: String,
    pub value: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum TenantKeySource {
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
        self.validate_unambiguous_keys()?;
        Ok(())
    }

    pub fn schema_roots(&self, single_config_dir: &Path) -> Vec<TenantSchema> {
        match self.mode {
            TenancyMode::Single => vec![TenantSchema {
                tenant_id: None,
                root_dir: single_config_dir.to_path_buf(),
            }],
            TenancyMode::Multi => self
                .tenants
                .iter()
                .map(|tenant| TenantSchema {
                    tenant_id: Some(tenant.id.clone()),
                    root_dir: self.tenants_dir.join(&tenant.id),
                })
                .collect(),
        }
    }

    pub fn runtime_registry_dir(&self, single_config_dir: &Path) -> Result<PathBuf, String> {
        match self.mode {
            TenancyMode::Single => Ok(single_config_dir.to_path_buf()),
            // This foundation slice only establishes tenancy schema layout and validation.
            // Full tenant runtime loading and tenant-scoped provider matching land later.
            TenancyMode::Multi => Err(
                "tenancy.mode=\"multi\" is configured, but tenant-scoped provider loading is not implemented yet".to_string(),
            ),
        }
    }

    fn validate_duplicate_tenant_ids(&self) -> Result<(), String> {
        let mut seen = HashSet::new();

        for tenant in &self.tenants {
            let normalized_id = tenant.id.trim();
            if normalized_id.is_empty() {
                return Err("tenancy.tenants entries must include a non-empty id".to_string());
            }

            if !seen.insert(normalized_id.to_string()) {
                return Err(format!("duplicate tenancy tenant id: '{normalized_id}'"));
            }
        }

        Ok(())
    }

    fn validate_unambiguous_keys(&self) -> Result<(), String> {
        let mut seen: HashMap<NormalizedTenantKey, String> = HashMap::new();

        for tenant in &self.tenants {
            for key in &tenant.keys {
                let normalized = NormalizedTenantKey::from(key);

                if let Some(existing_tenant_id) = seen.get(&normalized) {
                    if existing_tenant_id == &tenant.id {
                        return Err(format!(
                            "duplicate tenant key definition for tenant '{}' on {}",
                            tenant.id,
                            key.describe(),
                        ));
                    }

                    return Err(format!(
                        "ambiguous tenant key match between '{}' and '{}' on {}",
                        existing_tenant_id,
                        tenant.id,
                        key.describe(),
                    ));
                }

                seen.insert(normalized, tenant.id.clone());
            }
        }

        Ok(())
    }
}

impl TenantKeyConfig {
    fn describe(&self) -> String {
        format!("{:?} '{}={}'", self.source, self.name, self.value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct NormalizedTenantKey {
    source: TenantKeySource,
    name: String,
    value: String,
}

impl From<&TenantKeyConfig> for NormalizedTenantKey {
    fn from(key: &TenantKeyConfig) -> Self {
        Self {
            source: key.source.clone(),
            name: key.name.trim().to_ascii_lowercase(),
            value: key.value.trim().to_ascii_lowercase(),
        }
    }
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
    fn multi_mode_uses_tenants_dir_schema() {
        let tenancy = TenancyConfig {
            mode: TenancyMode::Multi,
            tenants_dir: PathBuf::from("tenants"),
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
            tenants: vec![
                TenantConfig {
                    id: "acme".to_string(),
                    keys: vec![TenantKeyConfig {
                        source: TenantKeySource::Header,
                        name: "X-Tenant".to_string(),
                        value: "shared".to_string(),
                    }],
                },
                TenantConfig {
                    id: "globex".to_string(),
                    keys: vec![TenantKeyConfig {
                        source: TenantKeySource::Header,
                        name: "x-tenant".to_string(),
                        value: "shared".to_string(),
                    }],
                },
            ],
        };

        let error = tenancy.validate().unwrap_err();
        assert!(error.contains("ambiguous tenant key match"));
    }
}
