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

use axum::http::HeaderMap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::config::{TenancyMode, TenantKeySource};
use super::runtime::{TenantRuntime, TenantStore};

#[derive(Clone)]
pub struct TenantResolution {
    pub tenant: Arc<TenantRuntime>,
    pub metrics: TenantRequestMetrics,
}

#[derive(Debug, Clone)]
pub enum TenantRequestMetrics {
    Accepted { tenant: String },
    Rejected { reason: &'static str },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TenantResolutionRejection {
    UnknownTenant,
    UnknownKey,
    MissingKey,
    Conflict,
}

#[derive(Debug, Clone)]
pub struct TenantResolutionError {
    pub rejection: TenantResolutionRejection,
}

impl TenantResolutionError {
    pub fn metric_label(&self) -> &'static str {
        self.rejection.metric_label()
    }
}

impl TenantResolutionRejection {
    pub fn metric_label(&self) -> &'static str {
        match self {
            TenantResolutionRejection::UnknownTenant => "unknown_tenant",
            TenantResolutionRejection::UnknownKey => "unknown_key",
            TenantResolutionRejection::MissingKey => "missing_key",
            TenantResolutionRejection::Conflict => "header_key_conflict",
        }
    }
}

impl TenantStore {
    pub fn resolve_request(
        &self,
        headers: &HeaderMap,
        query_params: &HashMap<String, String>,
    ) -> Result<TenantResolution, TenantResolutionError> {
        if self.mode == TenancyMode::Single {
            return Ok(self.accept(self.default_tenant()));
        }

        let header_tenant_id = self.resolve_header_tenant(headers)?;
        let key_tenant_id = self.resolve_key_tenant(headers, query_params)?;

        match (header_tenant_id, key_tenant_id) {
            (Some(header_tenant_id), Some(key_tenant_id)) => {
                if header_tenant_id != key_tenant_id {
                    return Err(TenantResolutionError {
                        rejection: TenantResolutionRejection::Conflict,
                    });
                }

                Ok(self.accept(self.tenant_by_id(&header_tenant_id).unwrap()))
            }
            (Some(header_tenant_id), None) => {
                let tenant = self.tenant_by_id(&header_tenant_id).unwrap();
                if tenant.requires_key {
                    return Err(TenantResolutionError {
                        rejection: TenantResolutionRejection::MissingKey,
                    });
                }

                Ok(self.accept(tenant))
            }
            (None, Some(key_tenant_id)) => {
                let tenant = self.tenant_by_id(&key_tenant_id).unwrap();
                Ok(self.accept(tenant))
            }
            (None, None) => Ok(self.accept(self.default_tenant())),
        }
    }

    fn accept(&self, tenant: Arc<TenantRuntime>) -> TenantResolution {
        TenantResolution {
            metrics: TenantRequestMetrics::Accepted {
                tenant: tenant.label.clone(),
            },
            tenant,
        }
    }

    fn resolve_header_tenant(
        &self,
        headers: &HeaderMap,
    ) -> Result<Option<String>, TenantResolutionError> {
        let Some(header_value) = headers.get(&self.tenant_header_name) else {
            return Ok(None);
        };

        let header_value = header_value
            .to_str()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if header_value.is_empty() {
            return Err(TenantResolutionError {
                rejection: TenantResolutionRejection::UnknownTenant,
            });
        }

        self.header_lookup
            .get(&header_value)
            .cloned()
            .map(Some)
            .ok_or(TenantResolutionError {
                rejection: TenantResolutionRejection::UnknownTenant,
            })
    }

    fn resolve_key_tenant(
        &self,
        headers: &HeaderMap,
        query_params: &HashMap<String, String>,
    ) -> Result<Option<String>, TenantResolutionError> {
        let provided_keys = self.collect_provided_keys(headers, query_params);
        if provided_keys.is_empty() {
            return Ok(None);
        }

        let mut matched_tenants = HashSet::new();
        for provided_key in provided_keys {
            let Some(tenant_id) = self.key_lookup.get(&provided_key).cloned() else {
                return Err(TenantResolutionError {
                    rejection: TenantResolutionRejection::UnknownKey,
                });
            };
            matched_tenants.insert(tenant_id);
        }

        if matched_tenants.len() > 1 {
            return Err(TenantResolutionError {
                rejection: TenantResolutionRejection::Conflict,
            });
        }

        Ok(matched_tenants.into_iter().next())
    }

    fn collect_provided_keys(
        &self,
        headers: &HeaderMap,
        query_params: &HashMap<String, String>,
    ) -> Vec<ResolvedRequestKey> {
        let mut provided = Vec::new();

        for key_name in &self.known_header_key_names {
            if let Some(value) = headers.get(key_name).and_then(|value| value.to_str().ok()) {
                for candidate in request_key_variants(TenantKeySource::Header, key_name, value) {
                    provided.push(candidate);
                }
            }
        }

        for key_name in &self.known_query_key_names {
            if let Some(value) = query_params.get(key_name) {
                for candidate in request_key_variants(TenantKeySource::Query, key_name, value) {
                    provided.push(candidate);
                }
            }
        }

        provided
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedRequestKey {
    pub source: TenantKeySource,
    pub name: String,
    pub value: String,
}

fn request_key_variants(
    source: TenantKeySource,
    name: &str,
    value: &str,
) -> Vec<ResolvedRequestKey> {
    let mut variants = vec![ResolvedRequestKey {
        source: source.clone(),
        name: name.to_ascii_lowercase(),
        value: value.trim().to_string(),
    }];

    if matches!(source, TenantKeySource::Header) && name.eq_ignore_ascii_case("authorization") {
        if let Some(stripped) = value
            .trim()
            .strip_prefix("Bearer ")
            .or_else(|| value.trim().strip_prefix("bearer "))
        {
            variants.push(ResolvedRequestKey {
                source,
                name: name.to_ascii_lowercase(),
                value: stripped.trim().to_string(),
            });
        }
    }

    variants
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderRegistry;
    use crate::tenancy::config::{TenancyConfig, TenantConfig, TenantKeyConfig};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn build_test_store(tenants: Vec<TenantConfig>) -> TenantStore {
        let default_runtime = Arc::new(TenantRuntime {
            label: "default".to_string(),
            registry: Arc::new(ProviderRegistry::new()),
            requires_key: false,
        });

        let tenancy = TenancyConfig {
            mode: TenancyMode::Multi,
            tenants_dir: PathBuf::from("tenants"),
            tenant_header: "x-tenant".to_string(),
            tenants,
        };

        let mut tenants_by_id = HashMap::new();
        let mut header_lookup = HashMap::new();
        let mut key_lookup = HashMap::new();
        let mut known_header_key_names = HashSet::new();
        let mut known_query_key_names = HashSet::new();

        for tenant in &tenancy.tenants {
            let requires_key = tenant.requires_key(&tenancy.normalized_tenant_header());
            let runtime = Arc::new(TenantRuntime {
                label: tenant.id.clone(),
                registry: Arc::new(ProviderRegistry::new()),
                requires_key,
            });

            tenants_by_id.insert(tenant.id.clone(), runtime);
            header_lookup.insert(tenant.id.clone(), tenant.id.clone());
            for value in tenant
                .explicit_header_values(&tenancy.normalized_tenant_header())
                .unwrap()
            {
                header_lookup.insert(value.to_ascii_lowercase(), tenant.id.clone());
            }

            for api_key in tenant
                .api_keys(&tenancy.normalized_tenant_header())
                .unwrap()
            {
                match api_key.source {
                    TenantKeySource::Header => {
                        known_header_key_names.insert(api_key.name.clone());
                    }
                    TenantKeySource::Query => {
                        known_query_key_names.insert(api_key.name.clone());
                    }
                    _ => {}
                }

                key_lookup.insert(
                    ResolvedRequestKey {
                        source: api_key.source,
                        name: api_key.name,
                        value: api_key.value,
                    },
                    tenant.id.clone(),
                );
            }
        }

        TenantStore::new(
            TenancyMode::Multi,
            tenancy.normalized_tenant_header(),
            default_runtime,
            tenants_by_id,
            header_lookup,
            key_lookup,
            known_header_key_names,
            known_query_key_names,
        )
    }

    #[test]
    fn header_only_tenant_resolution() {
        let store = build_test_store(vec![TenantConfig {
            id: "acme".to_string(),
            keys: Vec::new(),
        }]);

        let mut headers = HeaderMap::new();
        headers.insert("x-tenant", "acme".parse().unwrap());

        let resolved = store.resolve_request(&headers, &HashMap::new()).unwrap();
        assert_eq!(resolved.tenant.label, "acme");
    }

    #[test]
    fn key_only_tenant_resolution() {
        let store = build_test_store(vec![TenantConfig {
            id: "acme".to_string(),
            keys: vec![TenantKeyConfig {
                source: TenantKeySource::Header,
                name: "x-api-key".to_string(),
                value: "secret-acme".to_string(),
                value_file: None,
                value_env: None,
            }],
        }]);

        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", "secret-acme".parse().unwrap());

        let resolved = store.resolve_request(&headers, &HashMap::new()).unwrap();
        assert_eq!(resolved.tenant.label, "acme");
    }

    #[test]
    fn header_and_key_agreement() {
        let store = build_test_store(vec![TenantConfig {
            id: "acme".to_string(),
            keys: vec![TenantKeyConfig {
                source: TenantKeySource::Header,
                name: "x-api-key".to_string(),
                value: "secret-acme".to_string(),
                value_file: None,
                value_env: None,
            }],
        }]);

        let mut headers = HeaderMap::new();
        headers.insert("x-tenant", "acme".parse().unwrap());
        headers.insert("x-api-key", "secret-acme".parse().unwrap());

        let resolved = store.resolve_request(&headers, &HashMap::new()).unwrap();
        assert_eq!(resolved.tenant.label, "acme");
    }

    #[test]
    fn header_and_key_conflict_is_rejected() {
        let store = build_test_store(vec![
            TenantConfig {
                id: "acme".to_string(),
                keys: vec![TenantKeyConfig {
                    source: TenantKeySource::Header,
                    name: "x-api-key".to_string(),
                    value: "secret-acme".to_string(),
                    value_file: None,
                    value_env: None,
                }],
            },
            TenantConfig {
                id: "globex".to_string(),
                keys: vec![TenantKeyConfig {
                    source: TenantKeySource::Header,
                    name: "x-api-key".to_string(),
                    value: "secret-globex".to_string(),
                    value_file: None,
                    value_env: None,
                }],
            },
        ]);

        let mut headers = HeaderMap::new();
        headers.insert("x-tenant", "acme".parse().unwrap());
        headers.insert("x-api-key", "secret-globex".parse().unwrap());

        let error = store
            .resolve_request(&headers, &HashMap::new())
            .err()
            .unwrap();
        assert_eq!(error.rejection, TenantResolutionRejection::Conflict);
    }

    #[test]
    fn unknown_tenant_is_rejected() {
        let store = build_test_store(vec![TenantConfig {
            id: "acme".to_string(),
            keys: Vec::new(),
        }]);

        let mut headers = HeaderMap::new();
        headers.insert("x-tenant", "missing".parse().unwrap());

        let error = store
            .resolve_request(&headers, &HashMap::new())
            .err()
            .unwrap();
        assert_eq!(error.rejection, TenantResolutionRejection::UnknownTenant);
    }

    #[test]
    fn unknown_key_is_rejected() {
        let store = build_test_store(vec![TenantConfig {
            id: "acme".to_string(),
            keys: vec![TenantKeyConfig {
                source: TenantKeySource::Header,
                name: "x-api-key".to_string(),
                value: "secret-acme".to_string(),
                value_file: None,
                value_env: None,
            }],
        }]);

        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", "unknown".parse().unwrap());

        let error = store
            .resolve_request(&headers, &HashMap::new())
            .err()
            .unwrap();
        assert_eq!(error.rejection, TenantResolutionRejection::UnknownKey);
    }

    #[test]
    fn default_tenant_fallback() {
        let store = build_test_store(vec![TenantConfig {
            id: "acme".to_string(),
            keys: Vec::new(),
        }]);

        let resolved = store
            .resolve_request(&HeaderMap::new(), &HashMap::new())
            .unwrap();
        assert_eq!(resolved.tenant.label, "default");
    }

    #[test]
    fn header_only_is_rejected_when_tenant_requires_key() {
        let store = build_test_store(vec![TenantConfig {
            id: "acme".to_string(),
            keys: vec![TenantKeyConfig {
                source: TenantKeySource::Header,
                name: "x-api-key".to_string(),
                value: "secret-acme".to_string(),
                value_file: None,
                value_env: None,
            }],
        }]);

        let mut headers = HeaderMap::new();
        headers.insert("x-tenant", "acme".parse().unwrap());

        let error = store
            .resolve_request(&headers, &HashMap::new())
            .err()
            .unwrap();
        assert_eq!(error.rejection, TenantResolutionRejection::MissingKey);
    }
}
