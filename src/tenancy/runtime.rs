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
use std::sync::{Arc, RwLock};

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

pub struct TenantStoreHandle {
    current: RwLock<Arc<TenantStore>>,
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

impl TenantStoreHandle {
    pub fn new(initial: Arc<TenantStore>) -> Self {
        Self {
            current: RwLock::new(initial),
        }
    }

    pub fn current(&self) -> Arc<TenantStore> {
        self.current.read().unwrap().clone()
    }

    pub fn reload_all(&self, config: &AppConfig) -> Result<Arc<TenantStore>, Box<dyn Error>> {
        let rebuilt = build_runtime_store(config)?;
        *self.current.write().unwrap() = rebuilt.clone();
        Ok(rebuilt)
    }

    pub fn reload_tenant(
        &self,
        config: &AppConfig,
        tenant_id: &str,
    ) -> Result<Arc<TenantStore>, Box<dyn Error>> {
        config.validate()?;

        let current = self.current();
        let updated = match config.tenancy.mode {
            TenancyMode::Single => reload_single_mode_store(config, tenant_id)?,
            TenancyMode::Multi => reload_multi_mode_tenant(config, &current, tenant_id)?,
        };

        *self.current.write().unwrap() = updated.clone();
        Ok(updated)
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
    let default_tenant = build_single_mode_default_runtime(config)?;

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
    let default_tenant = build_multi_mode_default_runtime(config)?;
    let tenants_by_id = build_named_tenant_runtimes(tenancy, &tenant_header)?;
    let lookup_state = build_lookup_state(tenancy, &tenant_header)?;

    Ok(Arc::new(TenantStore::new(
        TenancyMode::Multi,
        tenant_header,
        default_tenant,
        tenants_by_id,
        lookup_state.header_lookup,
        lookup_state.key_lookup,
        lookup_state.known_header_key_names,
        lookup_state.known_query_key_names,
    )))
}

fn build_single_mode_default_runtime(
    config: &AppConfig,
) -> Result<Arc<TenantRuntime>, Box<dyn Error>> {
    let registry = build_registry_from_layers(&[config.config_dir.as_path()])?;
    Ok(Arc::new(TenantRuntime {
        label: DEFAULT_TENANT_ID.to_string(),
        registry,
        requires_key: false,
    }))
}

fn build_multi_mode_default_runtime(
    config: &AppConfig,
) -> Result<Arc<TenantRuntime>, Box<dyn Error>> {
    let default_root = config.tenancy.tenants_dir.join(DEFAULT_TENANT_ID);
    let registry = build_registry_from_layers(&[default_root.as_path()])?;
    Ok(Arc::new(TenantRuntime {
        label: DEFAULT_TENANT_ID.to_string(),
        registry,
        requires_key: false,
    }))
}

fn reload_single_mode_store(
    config: &AppConfig,
    tenant_id: &str,
) -> Result<Arc<TenantStore>, Box<dyn Error>> {
    if tenant_id != DEFAULT_TENANT_ID {
        return Err(format!("unknown tenant '{}'", tenant_id).into());
    }

    build_single_mode_store(config)
}

fn reload_multi_mode_tenant(
    config: &AppConfig,
    current: &Arc<TenantStore>,
    tenant_id: &str,
) -> Result<Arc<TenantStore>, Box<dyn Error>> {
    let tenancy = &config.tenancy;
    let tenant_header = tenancy.normalized_tenant_header();
    let mut tenants_by_id = current.tenants_by_id.clone();
    let default_tenant = if tenant_id == DEFAULT_TENANT_ID {
        build_multi_mode_default_runtime(config)?
    } else {
        current.default_tenant()
    };
    let mut header_lookup = current.header_lookup.clone();
    let mut key_lookup = current.key_lookup.clone();

    if tenant_id != DEFAULT_TENANT_ID {
        let tenant_config = tenancy
            .tenant_config(tenant_id)
            .ok_or_else(|| format!("unknown tenant '{}'", tenant_id))?;
        let runtime = build_named_tenant_runtime(tenancy, tenant_config, &tenant_header)?;
        tenants_by_id.insert(tenant_id.to_string(), runtime);

        refresh_tenant_lookup_entries(
            tenancy,
            tenant_config,
            tenant_id,
            &tenant_header,
            &mut header_lookup,
            &mut key_lookup,
        )?;
    }

    let (known_header_key_names, known_query_key_names) = collect_known_key_names(&key_lookup);

    Ok(Arc::new(TenantStore::new(
        current.mode.clone(),
        current.tenant_header_name.clone(),
        default_tenant,
        tenants_by_id,
        header_lookup,
        key_lookup,
        known_header_key_names,
        known_query_key_names,
    )))
}

struct LookupState {
    header_lookup: HashMap<String, String>,
    key_lookup: HashMap<ResolvedRequestKey, String>,
    known_header_key_names: HashSet<String>,
    known_query_key_names: HashSet<String>,
}

fn build_named_tenant_runtimes(
    tenancy: &TenancyConfig,
    tenant_header: &str,
) -> Result<HashMap<String, Arc<TenantRuntime>>, Box<dyn Error>> {
    let mut tenants_by_id = HashMap::new();

    for tenant in &tenancy.tenants {
        let runtime = build_named_tenant_runtime(tenancy, tenant, tenant_header)?;
        tenants_by_id.insert(tenant.id.clone(), runtime);
    }

    Ok(tenants_by_id)
}

fn build_named_tenant_runtime(
    tenancy: &TenancyConfig,
    tenant: &TenantConfig,
    tenant_header: &str,
) -> Result<Arc<TenantRuntime>, Box<dyn Error>> {
    let root_dir = tenancy.tenants_dir.join(&tenant.id);
    // Each tenant runtime is isolated to built-ins plus its own overlay.
    let registry = build_registry_from_layers(&[root_dir.as_path()])?;
    Ok(Arc::new(TenantRuntime {
        label: tenant.id.clone(),
        registry,
        requires_key: tenant.requires_key(tenant_header),
    }))
}

fn build_lookup_state(
    tenancy: &TenancyConfig,
    tenant_header: &str,
) -> Result<LookupState, Box<dyn Error>> {
    let mut header_lookup = HashMap::new();
    let mut key_lookup = HashMap::new();

    for tenant in &tenancy.tenants {
        register_header_lookups(tenant, tenancy, &mut header_lookup)?;
        register_key_lookups(tenant, tenant_header, &mut key_lookup)?;
    }

    let (known_header_key_names, known_query_key_names) = collect_known_key_names(&key_lookup);

    Ok(LookupState {
        header_lookup,
        key_lookup,
        known_header_key_names,
        known_query_key_names,
    })
}

fn refresh_tenant_lookup_entries(
    tenancy: &TenancyConfig,
    tenant: &TenantConfig,
    tenant_id: &str,
    tenant_header: &str,
    header_lookup: &mut HashMap<String, String>,
    key_lookup: &mut HashMap<ResolvedRequestKey, String>,
) -> Result<(), Box<dyn Error>> {
    // Rebuild the target tenant's lookup entries before swapping so auth rotation is atomic.
    header_lookup.retain(|_, mapped_tenant_id| mapped_tenant_id != tenant_id);
    key_lookup.retain(|_, mapped_tenant_id| mapped_tenant_id != tenant_id);

    register_header_lookups(tenant, tenancy, header_lookup)?;
    register_key_lookups(tenant, tenant_header, key_lookup)?;
    Ok(())
}

fn collect_known_key_names(
    key_lookup: &HashMap<ResolvedRequestKey, String>,
) -> (HashSet<String>, HashSet<String>) {
    let mut known_header_key_names = HashSet::new();
    let mut known_query_key_names = HashSet::new();

    for key in key_lookup.keys() {
        match key.source {
            TenantKeySource::Header => {
                known_header_key_names.insert(key.name.clone());
            }
            TenantKeySource::Query => {
                known_query_key_names.insert(key.name.clone());
            }
            _ => {}
        }
    }

    (known_header_key_names, known_query_key_names)
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
) -> Result<(), Box<dyn Error>> {
    for api_key in tenant.api_keys(tenant_header)? {
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
