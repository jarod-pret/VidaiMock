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

use serde::Serialize;

use crate::config::AppConfig;

use super::{TenancyMode, TenantStore, DEFAULT_TENANT_ID};

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct TenantView {
    pub id: String,
    pub is_default: bool,
    pub requires_key: bool,
    pub provider_count: usize,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct TenantListView {
    pub mode: TenancyMode,
    pub tenants: Vec<TenantView>,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct ReloadView {
    pub reloaded: Vec<String>,
}

pub fn list_tenants(config: &AppConfig, store: &TenantStore) -> TenantListView {
    let mut tenants = vec![tenant_view_from_runtime(
        DEFAULT_TENANT_ID,
        store.default_tenant(),
        true,
    )];

    if config.tenancy.mode == TenancyMode::Multi {
        tenants.extend(
            config
                .tenancy
                .tenants
                .iter()
                .filter_map(|tenant| store.tenant_by_id(&tenant.id))
                .map(|runtime| tenant_view_from_runtime(&runtime.label, runtime.clone(), false)),
        );
    }

    TenantListView {
        mode: config.tenancy.mode.clone(),
        tenants,
    }
}

pub fn tenant_view(config: &AppConfig, store: &TenantStore, tenant_id: &str) -> Option<TenantView> {
    if config.tenancy.mode == TenancyMode::Single {
        return (tenant_id == DEFAULT_TENANT_ID)
            .then(|| tenant_view_from_runtime(DEFAULT_TENANT_ID, store.default_tenant(), true));
    }

    if tenant_id == DEFAULT_TENANT_ID {
        return Some(tenant_view_from_runtime(
            DEFAULT_TENANT_ID,
            store.default_tenant(),
            true,
        ));
    }

    store.tenant_by_id(tenant_id).map(|runtime| {
        let tenant_id = runtime.label.clone();
        tenant_view_from_runtime(&tenant_id, runtime, false)
    })
}

fn tenant_view_from_runtime(
    tenant_id: &str,
    runtime: std::sync::Arc<super::TenantRuntime>,
    is_default: bool,
) -> TenantView {
    TenantView {
        id: tenant_id.to_string(),
        is_default,
        requires_key: runtime.requires_key,
        provider_count: runtime.registry.providers.len(),
    }
}
