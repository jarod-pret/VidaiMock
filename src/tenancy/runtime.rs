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

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::sync::Arc;

use crate::config::AppConfig;
use crate::provider::{build_registry_from_layers, ProviderRegistry};

use super::config::{TenancyConfig, TenancyMode, TenantConfig, TenantKeySource, DEFAULT_TENANT_ID};
use super::resolution::ResolvedRequestKey;

pub struct TenantRuntime {
    pub label: String,
    pub registry: Arc<ProviderRegistry>,
    pub requires_key: bool,
}

pub struct TenantStore {
    pub(crate) mode: TenancyMode,
    pub(crate) tenant_header_name: String,
    pub(crate) default_tenant: Arc<TenantRuntime>,
    pub(crate) tenants_by_id: HashMap<String, Arc<TenantRuntime>>,
    pub(crate) header_lookup: HashMap<String, String>,
    pub(crate) key_lookup: HashMap<ResolvedRequestKey, String>,
    pub(crate) known_header_key_names: HashSet<String>,
    pub(crate) known_query_key_names: HashSet<String>,
}

impl TenantStore {
    pub fn new(
        mode: TenancyMode,
        tenant_header_name: String,
        default_tenant: Arc<TenantRuntime>,
        tenants_by_id: HashMap<String, Arc<TenantRuntime>>,
        header_lookup: HashMap<String, String>,
        key_lookup: HashMap<ResolvedRequestKey, String>,
        known_header_key_names: HashSet<String>,
        known_query_key_names: HashSet<String>,
    ) -> Self {
        Self {
            mode,
            tenant_header_name,
            default_tenant,
            tenants_by_id,
            header_lookup,
            key_lookup,
            known_header_key_names,
            known_query_key_names,
        }
    }

    pub fn default_tenant(&self) -> Arc<TenantRuntime> {
        self.default_tenant.clone()
    }

    pub fn tenant_by_id(&self, tenant_id: &str) -> Option<Arc<TenantRuntime>> {
        if tenant_id == DEFAULT_TENANT_ID {
            return Some(self.default_tenant());
        }

        self.tenants_by_id.get(tenant_id).cloned()
    }
}

pub fn build_runtime_store(config: &AppConfig) -> Result<Arc<TenantStore>, Box<dyn Error>> {
    config.validate()?;

    match config.tenancy.mode {
        TenancyMode::Single => build_single_mode_store(config),
        TenancyMode::Multi => build_multi_mode_store(config),
    }
}

fn build_single_mode_store(config: &AppConfig) -> Result<Arc<TenantStore>, Box<dyn Error>> {
    let registry = build_registry_from_layers(&[config.config_dir.as_path()])?;
    let default_tenant = Arc::new(TenantRuntime {
        label: DEFAULT_TENANT_ID.to_string(),
        registry,
        requires_key: false,
    });

    Ok(Arc::new(TenantStore::new(
        TenancyMode::Single,
        config.tenancy.normalized_tenant_header(),
        default_tenant,
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        HashSet::new(),
        HashSet::new(),
    )))
}

fn build_multi_mode_store(config: &AppConfig) -> Result<Arc<TenantStore>, Box<dyn Error>> {
    let tenancy = &config.tenancy;
    let tenant_header = tenancy.normalized_tenant_header();
    let default_root = tenancy.tenants_dir.join(DEFAULT_TENANT_ID);
    let default_registry = build_registry_from_layers(&[default_root.as_path()])?;
    let default_tenant = Arc::new(TenantRuntime {
        label: DEFAULT_TENANT_ID.to_string(),
        registry: default_registry,
        requires_key: false,
    });

    let mut tenants_by_id = HashMap::new();
    let mut header_lookup = HashMap::new();
    let mut key_lookup = HashMap::new();
    let mut known_header_key_names = HashSet::new();
    let mut known_query_key_names = HashSet::new();

    for tenant in &tenancy.tenants {
        let root_dir = tenancy.tenants_dir.join(&tenant.id);
        // Each tenant runtime is isolated to built-ins plus its own overlay.
        let registry = build_registry_from_layers(&[root_dir.as_path()])?;
        let runtime = Arc::new(TenantRuntime {
            label: tenant.id.clone(),
            registry,
            requires_key: tenant.requires_key(&tenant_header),
        });

        register_header_lookups(tenant, tenancy, &mut header_lookup)?;
        register_key_lookups(
            tenant,
            &tenant_header,
            &mut key_lookup,
            &mut known_header_key_names,
            &mut known_query_key_names,
        )?;
        tenants_by_id.insert(tenant.id.clone(), runtime);
    }

    Ok(Arc::new(TenantStore::new(
        TenancyMode::Multi,
        tenant_header,
        default_tenant,
        tenants_by_id,
        header_lookup,
        key_lookup,
        known_header_key_names,
        known_query_key_names,
    )))
}

fn register_header_lookups(
    tenant: &TenantConfig,
    tenancy: &TenancyConfig,
    header_lookup: &mut HashMap<String, String>,
) -> Result<(), Box<dyn Error>> {
    header_lookup.insert(tenant.id.trim().to_ascii_lowercase(), tenant.id.clone());

    for value in tenant.explicit_header_values(&tenancy.normalized_tenant_header())? {
        header_lookup.insert(value.trim().to_ascii_lowercase(), tenant.id.clone());
    }

    Ok(())
}

fn register_key_lookups(
    tenant: &TenantConfig,
    tenant_header: &str,
    key_lookup: &mut HashMap<ResolvedRequestKey, String>,
    known_header_key_names: &mut HashSet<String>,
    known_query_key_names: &mut HashSet<String>,
) -> Result<(), Box<dyn Error>> {
    for api_key in tenant.api_keys(tenant_header)? {
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

    Ok(())
}
