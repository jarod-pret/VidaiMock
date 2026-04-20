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

use arc_swap::ArcSwap;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

use crate::config::{AppConfig, ChaosConfig, LatencyConfig};
use crate::provider::{build_registry_from_layers, ProviderRegistry};

use super::config::{
    AdminAuthConfig, TenancyConfig, TenancyMode, TenantConfig, TenantKeySource,
    TenantTemplateMetadata, DEFAULT_TENANT_ID,
};
use super::resolution::ResolvedRequestKey;

pub struct TenantRuntime {
    pub label: String,
    pub template_metadata: TenantTemplateMetadata,
    pub registry: Arc<ProviderRegistry>,
    pub requires_key: bool,
    pub management_auth_header: String,
    pub management_auth_secret: Option<String>,
    pub latency: LatencyConfig,
    pub chaos: ChaosConfig,
}

pub struct TenantStore {
    pub(crate) mode: TenancyMode,
    pub(crate) config_dir: PathBuf,
    pub(crate) tenancy: TenancyConfig,
    pub(crate) global_latency: LatencyConfig,
    pub(crate) global_chaos: ChaosConfig,
    pub(crate) admin_auth_header: String,
    pub(crate) admin_auth_secret: Option<String>,
    pub(crate) tenant_header_name: String,
    pub(crate) default_tenant: Arc<TenantRuntime>,
    pub(crate) tenants_by_id: HashMap<String, Arc<TenantRuntime>>,
    pub(crate) header_lookup: HashMap<String, String>,
    pub(crate) key_lookup: HashMap<ResolvedRequestKey, String>,
    pub(crate) known_header_key_names: HashSet<String>,
    pub(crate) known_query_key_names: HashSet<String>,
}

pub struct TenantStoreHandle {
    current: ArcSwap<TenantStore>,
}

impl TenantStore {
    pub fn new(
        mode: TenancyMode,
        config_dir: PathBuf,
        tenancy: TenancyConfig,
        global_latency: LatencyConfig,
        global_chaos: ChaosConfig,
        admin_auth_header: String,
        admin_auth_secret: Option<String>,
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
            config_dir,
            tenancy,
            global_latency,
            global_chaos,
            admin_auth_header,
            admin_auth_secret,
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

    fn runtime_config(&self) -> AppConfig {
        AppConfig {
            host: String::new(),
            port: 0,
            workers: 0,
            log_level: String::new(),
            config_dir: self.config_dir.clone(),
            tenancy: self.tenancy.clone(),
            latency: self.global_latency.clone(),
            chaos: self.global_chaos.clone(),
            endpoints: Vec::new(),
            response_file: None,
            reload_args: None,
        }
    }
}

impl TenantStoreHandle {
    pub fn new(initial: Arc<TenantStore>) -> Self {
        Self {
            current: ArcSwap::from(initial),
        }
    }

    pub fn current(&self) -> Arc<TenantStore> {
        self.current.load_full()
    }

    pub fn reload_all(&self, config: &AppConfig) -> Result<Arc<TenantStore>, Box<dyn Error>> {
        let rebuilt = build_runtime_store(config)?;
        self.current.store(rebuilt.clone());
        Ok(rebuilt)
    }

    pub fn reload_tenant(&self, tenant_id: &str) -> Result<Arc<TenantStore>, Box<dyn Error>> {
        let current = self.current();
        let updated = match current.mode {
            TenancyMode::Single => reload_single_mode_store(&current, tenant_id)?,
            TenancyMode::Multi => reload_multi_mode_tenant(&current, tenant_id)?,
        };

        self.current.store(updated.clone());
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
    let admin_auth = resolve_admin_auth(&config.tenancy.admin_auth)?;

    Ok(Arc::new(TenantStore::new(
        TenancyMode::Single,
        config.config_dir.clone(),
        config.tenancy.clone(),
        config.latency.clone(),
        config.chaos.clone(),
        config.tenancy.admin_auth.header.clone(),
        admin_auth,
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
    let discovered_tenants = tenancy.load_discovered_tenants()?;
    let default_tenant =
        build_multi_mode_default_runtime(config, discovered_tenants.default_tenant.as_ref())?;
    let tenants_by_id = build_named_tenant_runtimes(
        config,
        tenancy,
        &discovered_tenants.named_tenants,
        &tenant_header,
    )?;
    let lookup_state =
        build_lookup_state(tenancy, &discovered_tenants.named_tenants, &tenant_header)?;
    let admin_auth = resolve_admin_auth(&tenancy.admin_auth)?;

    Ok(Arc::new(TenantStore::new(
        TenancyMode::Multi,
        config.config_dir.clone(),
        tenancy.clone(),
        config.latency.clone(),
        config.chaos.clone(),
        tenancy.admin_auth.header.clone(),
        admin_auth,
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
    build_single_mode_default_runtime_from_path(config)
}

fn build_single_mode_default_runtime_from_path(
    config: &AppConfig,
) -> Result<Arc<TenantRuntime>, Box<dyn Error>> {
    let registry = build_registry_from_layers(&[config.config_dir.as_path()])?;
    Ok(Arc::new(TenantRuntime {
        label: DEFAULT_TENANT_ID.to_string(),
        template_metadata: TenantTemplateMetadata {
            id: DEFAULT_TENANT_ID.to_string(),
            ..TenantTemplateMetadata::default()
        },
        registry,
        requires_key: false,
        management_auth_header: "x-tenant-admin-key".to_string(),
        management_auth_secret: None,
        latency: config.latency.clone(),
        chaos: config.chaos.clone(),
    }))
}

fn build_multi_mode_default_runtime(
    config: &AppConfig,
    tenant_config: Option<&TenantConfig>,
) -> Result<Arc<TenantRuntime>, Box<dyn Error>> {
    build_multi_mode_default_runtime_from_tenancy(config, tenant_config)
}

fn build_multi_mode_default_runtime_from_tenancy(
    config: &AppConfig,
    tenant_config: Option<&TenantConfig>,
) -> Result<Arc<TenantRuntime>, Box<dyn Error>> {
    let default_root = config.tenancy.tenants_dir.join(DEFAULT_TENANT_ID);
    let registry = build_registry_from_layers(&[default_root.as_path()])?;
    let (management_auth_header, management_auth_secret) =
        resolve_tenant_management_auth(tenant_config)?;
    Ok(Arc::new(TenantRuntime {
        label: DEFAULT_TENANT_ID.to_string(),
        template_metadata: tenant_config
            .map(TenantConfig::template_metadata)
            .unwrap_or_else(|| TenantTemplateMetadata {
                id: DEFAULT_TENANT_ID.to_string(),
                ..TenantTemplateMetadata::default()
            }),
        registry,
        requires_key: false,
        management_auth_header,
        management_auth_secret,
        latency: tenant_config
            .map(|tenant| tenant.effective_latency(&config.latency))
            .unwrap_or_else(|| config.latency.clone()),
        chaos: tenant_config
            .map(|tenant| tenant.effective_chaos(&config.chaos))
            .unwrap_or_else(|| config.chaos.clone()),
    }))
}

fn reload_single_mode_store(
    current: &Arc<TenantStore>,
    tenant_id: &str,
) -> Result<Arc<TenantStore>, Box<dyn Error>> {
    if tenant_id != DEFAULT_TENANT_ID {
        return Err(format!("unknown tenant '{}'", tenant_id).into());
    }

    let config = current.runtime_config();
    let default_tenant = build_single_mode_default_runtime_from_path(&config)?;

    Ok(Arc::new(TenantStore::new(
        TenancyMode::Single,
        current.config_dir.clone(),
        current.tenancy.clone(),
        current.global_latency.clone(),
        current.global_chaos.clone(),
        current.admin_auth_header.clone(),
        current.admin_auth_secret.clone(),
        current.tenant_header_name.clone(),
        default_tenant,
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        HashSet::new(),
        HashSet::new(),
    )))
}

fn reload_multi_mode_tenant(
    current: &Arc<TenantStore>,
    tenant_id: &str,
) -> Result<Arc<TenantStore>, Box<dyn Error>> {
    let tenancy = &current.tenancy;
    let tenant_header = tenancy.normalized_tenant_header();
    let config = current.runtime_config();
    let mut tenants_by_id = current.tenants_by_id.clone();
    let default_tenant = if tenant_id == DEFAULT_TENANT_ID {
        let default_tenant_config = tenancy.load_default_tenant()?;
        build_multi_mode_default_runtime_from_tenancy(&config, default_tenant_config.as_ref())?
    } else {
        current.default_tenant()
    };
    let mut header_lookup = current.header_lookup.clone();
    let mut key_lookup = current.key_lookup.clone();

    if tenant_id != DEFAULT_TENANT_ID {
        let tenant_config = tenancy.load_named_tenant(tenant_id)?;
        let runtime = build_named_tenant_runtime(&config, tenancy, &tenant_config, &tenant_header)?;

        validate_and_refresh_tenant_lookup_entries(
            &tenant_config,
            tenant_id,
            &tenant_header,
            &mut header_lookup,
            &mut key_lookup,
        )?;

        tenants_by_id.insert(tenant_id.to_string(), runtime);
    }

    let (known_header_key_names, known_query_key_names) = collect_known_key_names(&key_lookup);

    Ok(Arc::new(TenantStore::new(
        current.mode.clone(),
        current.config_dir.clone(),
        current.tenancy.clone(),
        current.global_latency.clone(),
        current.global_chaos.clone(),
        current.admin_auth_header.clone(),
        current.admin_auth_secret.clone(),
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
    config: &AppConfig,
    tenancy: &TenancyConfig,
    named_tenants: &[TenantConfig],
    tenant_header: &str,
) -> Result<HashMap<String, Arc<TenantRuntime>>, Box<dyn Error>> {
    let mut tenants_by_id = HashMap::new();

    for tenant in named_tenants {
        let runtime = build_named_tenant_runtime(config, tenancy, tenant, tenant_header)?;
        tenants_by_id.insert(tenant.id.clone(), runtime);
    }

    Ok(tenants_by_id)
}

fn build_named_tenant_runtime(
    config: &AppConfig,
    tenancy: &TenancyConfig,
    tenant: &TenantConfig,
    tenant_header: &str,
) -> Result<Arc<TenantRuntime>, Box<dyn Error>> {
    let root_dir = tenancy.tenants_dir.join(&tenant.id);
    // Each tenant runtime is isolated to built-ins plus its own overlay.
    let registry = build_registry_from_layers(&[root_dir.as_path()])?;
    let (management_auth_header, management_auth_secret) =
        resolve_tenant_management_auth(Some(tenant))?;
    Ok(Arc::new(TenantRuntime {
        label: tenant.id.clone(),
        template_metadata: tenant.template_metadata(),
        registry,
        requires_key: tenant.requires_key(tenant_header),
        management_auth_header,
        management_auth_secret,
        latency: tenant.effective_latency(&config.latency),
        chaos: tenant.effective_chaos(&config.chaos),
    }))
}

fn build_lookup_state(
    tenancy: &TenancyConfig,
    named_tenants: &[TenantConfig],
    tenant_header: &str,
) -> Result<LookupState, Box<dyn Error>> {
    let mut header_lookup = HashMap::new();
    let mut key_lookup = HashMap::new();

    for tenant in named_tenants {
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

fn validate_and_refresh_tenant_lookup_entries(
    tenant: &TenantConfig,
    tenant_id: &str,
    tenant_header: &str,
    header_lookup: &mut HashMap<String, String>,
    key_lookup: &mut HashMap<ResolvedRequestKey, String>,
) -> Result<(), Box<dyn Error>> {
    // Rebuild the target tenant's lookup entries before swapping so auth rotation is atomic.
    header_lookup.retain(|_, mapped_tenant_id| mapped_tenant_id != tenant_id);
    key_lookup.retain(|_, mapped_tenant_id| mapped_tenant_id != tenant_id);

    register_header_lookups_for_tenant_header(tenant, tenant_header, header_lookup)?;
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

fn resolve_admin_auth(admin_auth: &AdminAuthConfig) -> Result<Option<String>, Box<dyn Error>> {
    Ok(admin_auth.resolved_value()?)
}

fn resolve_tenant_management_auth(
    tenant: Option<&TenantConfig>,
) -> Result<(String, Option<String>), Box<dyn Error>> {
    let Some(tenant) = tenant else {
        return Ok(("x-tenant-admin-key".to_string(), None));
    };

    Ok(tenant
        .resolved_management_auth()?
        .map(|management_auth| (management_auth.header, Some(management_auth.secret)))
        .unwrap_or_else(|| ("x-tenant-admin-key".to_string(), None)))
}

fn register_header_lookups(
    tenant: &TenantConfig,
    tenancy: &TenancyConfig,
    header_lookup: &mut HashMap<String, String>,
) -> Result<(), Box<dyn Error>> {
    register_header_lookups_for_tenant_header(
        tenant,
        &tenancy.normalized_tenant_header(),
        header_lookup,
    )
}

fn register_header_lookups_for_tenant_header(
    tenant: &TenantConfig,
    tenant_header: &str,
    header_lookup: &mut HashMap<String, String>,
) -> Result<(), Box<dyn Error>> {
    register_header_lookup_value(
        header_lookup,
        tenant_header.to_string(),
        tenant.id.trim().to_ascii_lowercase(),
        &tenant.id,
    )?;

    for value in tenant.explicit_header_values(tenant_header)? {
        register_header_lookup_value(
            header_lookup,
            tenant_header.to_string(),
            value.trim().to_ascii_lowercase(),
            &tenant.id,
        )?;
    }

    Ok(())
}

fn register_key_lookups(
    tenant: &TenantConfig,
    tenant_header: &str,
    key_lookup: &mut HashMap<ResolvedRequestKey, String>,
) -> Result<(), Box<dyn Error>> {
    for api_key in tenant.api_keys(tenant_header)? {
        register_key_lookup_value(
            key_lookup,
            ResolvedRequestKey {
                source: api_key.source,
                name: api_key.name,
                value: api_key.value,
            },
            &tenant.id,
        )?;
    }

    Ok(())
}

fn register_header_lookup_value(
    header_lookup: &mut HashMap<String, String>,
    header_name: String,
    lookup_value: String,
    tenant_id: &str,
) -> Result<(), Box<dyn Error>> {
    if let Some(existing_tenant_id) = header_lookup.get(&lookup_value) {
        if existing_tenant_id != tenant_id {
            return Err(format!(
                "ambiguous tenant key match between '{}' and '{}' on Header '{}={}'",
                existing_tenant_id, tenant_id, header_name, lookup_value
            )
            .into());
        }
    }

    header_lookup.insert(lookup_value, tenant_id.to_string());
    Ok(())
}

fn register_key_lookup_value(
    key_lookup: &mut HashMap<ResolvedRequestKey, String>,
    resolved_key: ResolvedRequestKey,
    tenant_id: &str,
) -> Result<(), Box<dyn Error>> {
    if let Some(existing_tenant_id) = key_lookup.get(&resolved_key) {
        if existing_tenant_id != tenant_id {
            return Err(format!(
                "ambiguous tenant key match between '{}' and '{}' on {:?} '{}={}'",
                existing_tenant_id,
                tenant_id,
                resolved_key.source,
                resolved_key.name,
                resolved_key.value
            )
            .into());
        }
    }

    key_lookup.insert(resolved_key, tenant_id.to_string());
    Ok(())
}
