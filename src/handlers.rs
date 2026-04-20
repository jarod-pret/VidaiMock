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

use crate::replacer::Replacer;
use axum::{
    body::Bytes,
    extract::{Json, OriginalUri, Path, Query},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Extension,
};
use futures::stream::{self};
use futures::StreamExt;
use rand::Rng;
use serde_json::Value;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::time::{sleep, Duration};

use crate::config::AppConfig;
use crate::provider::ProviderRegistry;
use crate::tenancy::{
    list_tenants, tenant_view, ReloadView, TenancyMode, TenantRequestMetrics, TenantResolution,
    TenantResolutionError, TenantResolutionRejection, TenantStoreHandle,
};
// use crate::formats::load_response; // Function removed/unused

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub tenants: Arc<TenantStoreHandle>,
}

/// Apply configured latency delay for realistic simulation mode
async fn apply_latency(config: &AppConfig, headers: &HeaderMap) {
    let mut base = config.latency.base_ms;
    let mut jitter_pct = config.latency.jitter_pct;

    // Header Overrides
    if let Some(val) = headers.get("x-vidai-latency") {
        if let Ok(ms) = val.to_str().unwrap_or_default().parse::<u64>() {
            base = ms;
        }
    }
    if let Some(val) = headers.get("x-vidai-jitter") {
        if let Ok(pct) = val.to_str().unwrap_or_default().parse::<f64>() {
            jitter_pct = pct;
        }
    }

    if base > 0 {
        let mut final_delay = base;
        if jitter_pct > 0.0 {
            let variance = (base as f64 * jitter_pct) as u64;
            // Use rng for simple jitter
            let jitter = rand::rng().random_range(0..=(variance * 2)) as i64 - variance as i64;
            final_delay = (base as i64 + jitter).max(0) as u64;
        }
        sleep(Duration::from_millis(final_delay)).await;
    }
}

/// Checks for chaos overrides and returns an error response if chaos triggers.
fn check_chaos_failure(config: &AppConfig, headers: &HeaderMap) -> Option<Response> {
    let mut drop_pct = config.chaos.drop_pct;
    let mut malformed_pct = config.chaos.malformed_pct;

    // Header Overrides
    if let Some(val) = headers.get("x-vidai-chaos-drop") {
        if let Ok(val) = val.to_str().unwrap_or_default().parse::<f64>() {
            drop_pct = val.clamp(0.0, 100.0);
        }
    }
    if let Some(val) = headers.get("x-vidai-chaos-malformed") {
        if let Ok(val) = val.to_str().unwrap_or_default().parse::<f64>() {
            malformed_pct = val.clamp(0.0, 100.0);
        }
    }

    drop_pct = drop_pct.clamp(0.0, 100.0);
    malformed_pct = malformed_pct.clamp(0.0, 100.0);

    let mut rng = rand::rng();

    // 1. Connection Drop (HTTP 500)
    if drop_pct > 0.0 && rng.random_bool(drop_pct / 100.0) {
        return Some(
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Simulated Internal Server Error",
            )
                .into_response(),
        );
    }

    // 2. Malformed Response
    if malformed_pct > 0.0 && rng.random_bool(malformed_pct / 100.0) {
        let mut resp = Response::new(axum::body::Body::from(
            "This is not valid JSON { missing_brace",
        ));
        *resp.status_mut() = StatusCode::OK;
        return Some(resp);
    }

    None
}

/// Resolves the HTTP status code for a response.
/// Priority: X-Mock-Status header > provider status_code config > 200.
fn resolve_status_code(
    headers: &HeaderMap,
    status_code: Option<&str>,
    context: &tera::Context,
    registry: &Arc<ProviderRegistry>,
) -> StatusCode {
    // Header override takes precedence — allows any endpoint to return any status
    if let Some(val) = headers.get("x-mock-status") {
        if let Ok(code) = val.to_str().unwrap_or_default().parse::<u16>() {
            if let Ok(status) = StatusCode::from_u16(code) {
                return status;
            }
        }
    }

    match status_code {
        None => StatusCode::OK,
        Some(raw) => {
            let code_str = if raw.contains("{{") {
                registry.render_str(raw, context).unwrap_or_default()
            } else {
                raw.to_string()
            };
            code_str
                .trim()
                .parse::<u16>()
                .ok()
                .and_then(|code| StatusCode::from_u16(code).ok())
                .unwrap_or(StatusCode::OK)
        }
    }
}

pub async fn health_check() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({"status": "ok"})))
}

fn response_with_metrics(mut response: Response, metrics: TenantRequestMetrics) -> Response {
    response.extensions_mut().insert(metrics);
    response
}

fn tenant_rejection_response(error: TenantResolutionError) -> Response {
    let status = match error.rejection {
        TenantResolutionRejection::UnknownTenant => StatusCode::NOT_FOUND,
        TenantResolutionRejection::UnknownKey
        | TenantResolutionRejection::MissingKey
        | TenantResolutionRejection::Conflict => StatusCode::UNAUTHORIZED,
    };

    let body = match error.rejection {
        TenantResolutionRejection::UnknownTenant => "Unknown tenant.",
        TenantResolutionRejection::UnknownKey => "Unknown tenant API key.",
        TenantResolutionRejection::MissingKey => "Tenant requires an API key.",
        TenantResolutionRejection::Conflict => {
            "Tenant header and API key must resolve to the same tenant."
        }
    };

    response_with_metrics(
        (status, body).into_response(),
        TenantRequestMetrics::Rejected {
            reason: error.metric_label(),
        },
    )
}

fn resolve_tenant_or_reject(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    query_params: &HashMap<String, String>,
) -> Result<TenantResolution, Response> {
    state
        .tenants
        .current()
        .resolve_request(headers, query_params)
        .map_err(tenant_rejection_response)
}

fn resolve_tenant_admin_or_reject(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    query_params: &HashMap<String, String>,
) -> Result<TenantResolution, Response> {
    let store = state.tenants.current();
    if store.mode == TenancyMode::Multi && !store.has_explicit_tenant_signal(headers, query_params)
    {
        return Err((StatusCode::UNAUTHORIZED, "Tenant admin auth required.").into_response());
    }

    store
        .resolve_request(headers, query_params)
        .map_err(tenant_rejection_response)
}

fn authorize_admin_or_reject(state: &Arc<AppState>, headers: &HeaderMap) -> Result<(), Response> {
    let store = state.tenants.current();
    let Some(expected_secret) = store.admin_auth_secret.as_deref() else {
        tracing::warn!("Admin endpoint access denied because admin auth is not configured");
        return Err((StatusCode::UNAUTHORIZED, "Admin authentication required.").into_response());
    };

    let Some(provided) = headers
        .get(store.admin_auth_header.as_str())
        .and_then(|value| value.to_str().ok())
    else {
        return Err((StatusCode::UNAUTHORIZED, "Admin authentication required.").into_response());
    };

    let provided = provided.trim();
    let header_matches = provided == expected_secret
        || (store
            .admin_auth_header
            .eq_ignore_ascii_case("authorization")
            && provided
                .strip_prefix("Bearer ")
                .or_else(|| provided.strip_prefix("bearer "))
                .is_some_and(|value| value.trim() == expected_secret));

    if header_matches {
        Ok(())
    } else {
        Err((StatusCode::UNAUTHORIZED, "Admin authentication required.").into_response())
    }
}

fn internal_error_response(message: &str, error: impl std::fmt::Display) -> Response {
    tracing::error!("{}: {}", message, error);
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{}.", message)).into_response()
}

pub async fn status_handler(Extension(state): Extension<Arc<AppState>>) -> Json<AppConfig> {
    Json(state.config.as_ref().clone())
}

pub async fn admin_tenants_handler(
    Extension(state): Extension<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = authorize_admin_or_reject(&state, &headers) {
        return response;
    }

    let store = state.tenants.current();
    Json(list_tenants(&store)).into_response()
}

pub async fn admin_tenant_handler(
    Extension(state): Extension<Arc<AppState>>,
    headers: HeaderMap,
    Path(tenant_id): Path<String>,
) -> Response {
    if let Err(response) = authorize_admin_or_reject(&state, &headers) {
        return response;
    }

    let store = state.tenants.current();
    let Some(view) = tenant_view(&store, &tenant_id) else {
        return (StatusCode::NOT_FOUND, "Unknown tenant.").into_response();
    };

    Json(view).into_response()
}

pub async fn admin_reload_handler(
    Extension(state): Extension<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = authorize_admin_or_reject(&state, &headers) {
        return response;
    }

    // Reload re-reads the startup config source and rebuilds the live tenancy/admin/runtime state.
    // It does not rebuild the listener, router, or the serialized startup AppConfig returned by /status.
    let reloaded_config = match state.config.reload_from_source() {
        Ok(config) => config,
        Err(error) => return internal_error_response("Reload failed", error),
    };

    let store = match state.tenants.reload_all(&reloaded_config) {
        Ok(store) => store,
        Err(error) => return internal_error_response("Reload failed", error),
    };

    let reloaded = list_tenants(&store)
        .tenants
        .into_iter()
        .map(|tenant| tenant.id)
        .collect();

    Json(ReloadView { reloaded }).into_response()
}

pub async fn tenant_handler(
    Extension(state): Extension<Arc<AppState>>,
    headers: HeaderMap,
    Query(query_params): Query<HashMap<String, String>>,
) -> Response {
    let resolution = match resolve_tenant_admin_or_reject(&state, &headers, &query_params) {
        Ok(resolution) => resolution,
        Err(response) => return response,
    };

    let store = state.tenants.current();
    let Some(view) = tenant_view(&store, &resolution.tenant.label) else {
        return internal_error_response(
            "Tenant inspection failed",
            format!("missing runtime for tenant '{}'", resolution.tenant.label),
        );
    };

    response_with_metrics(Json(view).into_response(), resolution.metrics)
}

pub async fn tenant_reload_handler(
    Extension(state): Extension<Arc<AppState>>,
    headers: HeaderMap,
    Query(query_params): Query<HashMap<String, String>>,
) -> Response {
    let resolution = match resolve_tenant_admin_or_reject(&state, &headers, &query_params) {
        Ok(resolution) => resolution,
        Err(response) => return response,
    };

    if let Err(error) = state.tenants.reload_tenant(&resolution.tenant.label) {
        return internal_error_response("Reload failed", error);
    }

    response_with_metrics(
        Json(ReloadView {
            reloaded: vec![resolution.tenant.label.clone()],
        })
        .into_response(),
        resolution.metrics,
    )
}

pub async fn mock_handler(
    Extension(state): Extension<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query_params): Query<HashMap<String, String>>,
    Json(request_json): Json<Value>,
) -> impl IntoResponse {
    let resolution = match resolve_tenant_or_reject(&state, &headers, &query_params) {
        Ok(resolution) => resolution,
        Err(response) => return response,
    };

    // Apply latency simulation (with header override support)
    apply_latency(&state.config, &headers).await;

    // Check for chaos failures (500s or Malformed)
    if let Some(chaos_response) = check_chaos_failure(&state.config, &headers) {
        return response_with_metrics(chaos_response.into_response(), resolution.metrics);
    }

    // Check for streaming request
    let is_streaming_path = uri.path().contains(":streamGenerateContent")
        || uri.path().contains("/converse-stream")
        || uri.path().contains("/invoke-with-response-stream")
        || uri.path().ends_with("/stream");

    if is_streaming_path
        || request_json
            .get("stream")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    {
        return streaming_handler_inner(
            state,
            resolution,
            uri,
            headers,
            query_params,
            request_json,
        )
        .await
        .into_response();
    }

    let path = uri.path();
    let registry = resolution.tenant.registry.clone();

    // 1. Try to find a matching provider in the registry
    if let Some(provider) = registry.find_provider(path) {
        let mut header_map = HashMap::new();
        for (k, v) in headers.iter() {
            header_map.insert(k.to_string(), v.to_str().unwrap_or_default().to_string());
        }
        let path_segments: Vec<String> = path
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();

        // Build context
        let mut context = Replacer::build_context(
            Some(&request_json),
            &header_map,
            &query_params,
            &path_segments,
            &provider.name,
        );

        // Process request_mapping: Extract variables using Tera (e.g. prompt extraction)
        for (key, expr) in &provider.request_mapping {
            // Render the expression string using the current context
            // Render the expression string using the registry's Tera (so filters are available)
            let val = match registry.render_str(expr, &context) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("Failed to evaluate mapping '{}': {}", key, e);
                    continue;
                }
            };
            context.insert(key, &val);
        }

        // Render response
        // Either from template file or inline body
        let rendered = if let Some(template_path) = &provider.response_template {
            match registry.tera.render(template_path, &context) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("Template render failed: {}", e);
                    return response_with_metrics(
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("Template Error: {}", e),
                        )
                            .into_response(),
                        resolution.metrics,
                    );
                }
            }
        } else if let Some(body) = &provider.response_body {
            // Use registry.render_str for ad-hoc strings to ensure filters are available
            match registry.render_str(body, &context) {
                Ok(s) => s,
                Err(e) => {
                    return response_with_metrics(
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("Template Error: {}", e),
                        )
                            .into_response(),
                        resolution.metrics,
                    )
                }
            }
        } else {
            return response_with_metrics(
                (StatusCode::NOT_FOUND, "No response template defined").into_response(),
                resolution.metrics,
            );
        };

        let status = resolve_status_code(
            &headers,
            provider.status_code.as_deref(),
            &context,
            &registry,
        );
        let mut response = Response::new(axum::body::Body::from(rendered));
        *response.status_mut() = status;
        response.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        return response_with_metrics(response.into_response(), resolution.metrics);
    }

    // No matching provider found
    response_with_metrics(
        (
            StatusCode::NOT_FOUND,
            "No provider matched this request route.",
        )
            .into_response(),
        resolution.metrics,
    )
}

pub async fn models_handler(
    Extension(state): Extension<Arc<AppState>>,
    OriginalUri(_uri): OriginalUri,
    headers: HeaderMap,
) -> impl IntoResponse {
    let query_params = HashMap::new();
    let resolution = match resolve_tenant_or_reject(&state, &headers, &query_params) {
        Ok(resolution) => resolution,
        Err(response) => return response,
    };

    apply_latency(&state.config, &headers).await;

    let mut model_list = Vec::new();

    // Add models from loaded providers
    for provider in &resolution.tenant.registry.providers {
        model_list.push(serde_json::json!({
            "id": provider.name,
            "object": "model",
            "created": 1700000000,
            "owned_by": "vidai"
        }));
    }

    // Fallback to defaults if no providers loaded (shouldn't happen with embedded defaults)
    if model_list.is_empty() {
        model_list.push(serde_json::json!({
            "id": "gpt-4",
            "object": "model",
            "created": 1687882411,
            "owned_by": "openai"
        }));
    }

    let response_json = serde_json::json!({
        "object": "list",
        "data": model_list
    });

    Response::builder()
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(
            serde_json::to_string(&response_json).unwrap(),
        ))
        .map(|response| response_with_metrics(response, resolution.metrics))
        .unwrap()
}

pub async fn streaming_handler(
    Extension(state): Extension<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query_params): Query<HashMap<String, String>>,
    Json(request_json): Json<Value>,
) -> Response {
    let resolution = match resolve_tenant_or_reject(&state, &headers, &query_params) {
        Ok(resolution) => resolution,
        Err(response) => return response,
    };

    streaming_handler_inner(state, resolution, uri, headers, query_params, request_json).await
}

async fn streaming_handler_inner(
    state: Arc<AppState>,
    resolution: TenantResolution,
    uri: axum::http::Uri,
    headers: HeaderMap,
    query_params: HashMap<String, String>,
    request_json: Value,
) -> Response {
    let path = uri.path();
    let registry = resolution.tenant.registry.clone();

    // 1. Try to find provider
    if let Some(provider_ref) = registry.find_provider(path) {
        let provider = provider_ref.clone();
        if let Some(stream_config) = provider.stream.clone() {
            let mut header_map = HashMap::new();
            for (k, v) in headers.iter() {
                header_map.insert(k.to_string(), v.to_str().unwrap_or_default().to_string());
            }
            let path_segments: Vec<String> = path
                .split('/')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();

            // Extract context variables
            let mut base_context = Replacer::build_context(
                Some(&request_json),
                &header_map,
                &query_params,
                &path_segments,
                &provider.name,
            );

            // Process request_mapping
            for (key, expr) in &provider.request_mapping {
                let val = match registry.render_str(expr, &base_context) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!("Failed to evaluate mapping '{}': {}", key, e);
                        continue;
                    }
                };
                base_context.insert(key, &val);
            }

            // Simulate chunks (for now, still split by whitespace until we implement better tokenization/generation)
            let full_response = if let Some(template_path) = &provider.response_template {
                registry
                    .tera
                    .render(template_path, &base_context)
                    .unwrap_or_default()
            } else if let Some(body) = &provider.response_body {
                registry.render_str(body, &base_context).unwrap_or_default()
            } else {
                String::new()
            };

            // Extract content to stream
            // Support Tool Calls: extract_content returns Value
            let (content_val, has_tool_calls) = extract_content_value(&full_response);

            let chunks: Vec<serde_json::Value> = if has_tool_calls {
                // If tool calls exist, send as a single chunk.
                vec![content_val]
            } else if let Some(s) = content_val.as_str() {
                // Otherwise, split the string content into chunks by whitespace (per-word chunked streaming).
                s.split_whitespace()
                    .map(|w| serde_json::Value::String(format!("{} ", w)))
                    .collect()
            } else {
                // Fallback: single chunk if not string or not split-able.
                vec![content_val]
            };

            let lifecycle = stream_config.lifecycle.clone();
            let stream_fmt = stream_config.format.clone();
            let encoding = stream_config.encoding.clone().unwrap_or_default(); // "aws-event-stream" or empty for SSE
            let is_raw_frame = stream_config.frame_format.as_deref() == Some("raw");

            let mut trickle_ms = state.config.chaos.trickle_ms;
            let mut disconnect_pct = state.config.chaos.disconnect_pct;

            // Header Overrides for Streaming Chaos
            if let Some(val) = headers.get("x-vidai-chaos-trickle") {
                if let Ok(ms) = val.to_str().unwrap_or_default().parse::<u64>() {
                    trickle_ms = ms;
                }
            }
            if let Some(val) = headers.get("x-vidai-chaos-disconnect") {
                if let Ok(pct) = val.to_str().unwrap_or_default().parse::<f64>() {
                    disconnect_pct = pct;
                }
            }

            // Use 20ms default if no trickle configured, to ensure *some* streaming effect
            if trickle_ms == 0 {
                trickle_ms = 20;
            }

            let registry_inner = registry.clone();
            let stream_status = resolve_status_code(
                &headers,
                provider.status_code.as_deref(),
                &base_context,
                &registry,
            );

            let stream = stream::unfold(
                (
                    0,
                    chunks,
                    full_response,
                    base_context,
                    lifecycle,
                    registry_inner,
                    trickle_ms,
                    disconnect_pct,
                    stream_fmt,
                    encoding.clone(),
                    is_raw_frame,
                ),
                move |(
                    idx,
                    chunks,
                    full_response,
                    mut ctx,
                    lifecycle,
                    registry,
                    trickle_ms,
                    disconnect_pct,
                    stream_fmt,
                    encoding,
                    is_raw_frame,
                )| async move {
                    // Chaos: Early Disconnect
                    if disconnect_pct > 0.0 {
                        if rand::rng().random_bool(disconnect_pct / 100.0) {
                            return None;
                        }
                    }
                    // + 1 is necessary here in order to return DONE
                    if idx > chunks.len() + 1 {
                        return None;
                    }

                    let raw_data: Option<String> = if idx == 0 {
                        // Start Event
                        if let Some(lc) = &lifecycle {
                            if let Some(on_start) = &lc.on_start {
                                // Context setup?
                                if let Some(tmpl) = &on_start.template_body {
                                    Some(tera::Tera::one_off(tmpl, &ctx, false).unwrap_or_default())
                                } else if let Some(path) = &on_start.template_path {
                                    Some(registry.tera.render(path, &ctx).unwrap_or_default())
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else if idx <= chunks.len() {
                        // Chunk Event
                        let chunk = &chunks[idx - 1];
                        ctx.insert("chunk", chunk);

                        if let Some(lc) = &lifecycle {
                            if let Some(on_chunk) = &lc.on_chunk {
                                // Context already has "chunk"
                                if let Some(tmpl) = &on_chunk.template_body {
                                    Some(tera::Tera::one_off(tmpl, &ctx, false).unwrap_or_default())
                                } else if let Some(path) = &on_chunk.template_path {
                                    Some(registry.tera.render(path, &ctx).unwrap_or_default())
                                } else {
                                    Some(chunk.to_string())
                                }
                            } else {
                                // Default format from config or raw
                                if let Some(fmt) = &stream_fmt {
                                    let mut chunk_ctx = ctx.clone();
                                    chunk_ctx.insert("chunk", &chunk);
                                    Some(
                                        tera::Tera::one_off(fmt, &chunk_ctx, false)
                                            .unwrap_or_else(|_| chunk.to_string()),
                                    )
                                } else {
                                    Some(chunk.to_string())
                                }
                            }
                        } else {
                            Some(chunk.to_string())
                        }
                    } else {
                        // Stop Event
                        if let Some(lc) = &lifecycle {
                            if let Some(on_stop) = &lc.on_stop {
                                if let Some(tmpl) = &on_stop.template_body {
                                    Some(tera::Tera::one_off(tmpl, &ctx, false).unwrap_or_default())
                                } else if let Some(path) = &on_stop.template_path {
                                    Some(registry.tera.render(path, &ctx).unwrap_or_default())
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        } else {
                            if encoding == "aws-event-stream" {
                                None
                            } else {
                                Some("[DONE]".to_string())
                            }
                        }
                    };

                    let next_idx = idx + 1;
                    sleep(Duration::from_millis(trickle_ms)).await;

                    if let Some(data) = raw_data {
                        // Format output based on encoding
                        if encoding == "aws-event-stream" {
                            let bytes =
                                crate::aws_event_stream::AwsEventStreamEncoder::encode_chunk(&data);
                            Some((
                                Ok::<_, Infallible>(axum::body::Bytes::from(bytes)),
                                (
                                    next_idx,
                                    chunks,
                                    full_response,
                                    ctx,
                                    lifecycle,
                                    registry,
                                    trickle_ms,
                                    disconnect_pct,
                                    stream_fmt,
                                    encoding,
                                    is_raw_frame,
                                ),
                            ))
                        } else if is_raw_frame {
                            // Raw frame mode: emit template output verbatim with trailing newlines.
                            // Template controls all framing (event: lines, data: lines, etc).
                            let mut buf = Vec::new();
                            for line in data.lines() {
                                let line = line.trim();
                                if !line.is_empty() {
                                    buf.extend_from_slice(line.as_bytes());
                                    buf.extend_from_slice(b"\n");
                                }
                            }
                            buf.extend_from_slice(b"\n");
                            Some((
                                Ok::<_, Infallible>(axum::body::Bytes::from(buf)),
                                (
                                    next_idx,
                                    chunks,
                                    full_response,
                                    ctx,
                                    lifecycle,
                                    registry,
                                    trickle_ms,
                                    disconnect_pct,
                                    stream_fmt,
                                    encoding,
                                    is_raw_frame,
                                ),
                            ))
                        } else {
                            // SSE Format: auto-wrap with data: prefix
                            let minified = minify_json(data);
                            let mut buf = Vec::new();

                            // Determine event name
                            let mut event_name = None;
                            if idx == 0 {
                                if let Some(lc) = &lifecycle {
                                    if let Some(s) = &lc.on_start {
                                        event_name = s.event_name.clone();
                                    }
                                }
                            } else if idx <= chunks.len() {
                                if let Some(lc) = &lifecycle {
                                    if let Some(s) = &lc.on_chunk {
                                        event_name = s.event_name.clone();
                                    }
                                }
                            } else {
                                if let Some(lc) = &lifecycle {
                                    if let Some(s) = &lc.on_stop {
                                        event_name = s.event_name.clone();
                                    }
                                }
                            }

                            if let Some(name) = event_name {
                                buf.extend_from_slice(format!("event: {}\n", name).as_bytes());
                            }
                            buf.extend_from_slice(format!("data: {}\n", minified).as_bytes());
                            buf.extend_from_slice(b"\n");
                            Some((
                                Ok::<_, Infallible>(axum::body::Bytes::from(buf)),
                                (
                                    next_idx,
                                    chunks,
                                    full_response,
                                    ctx,
                                    lifecycle,
                                    registry,
                                    trickle_ms,
                                    disconnect_pct,
                                    stream_fmt,
                                    encoding,
                                    is_raw_frame,
                                ),
                            ))
                        }
                    } else {
                        // Skip this tick (e.g. if start/stop event produced no data)
                        Some((
                            Ok::<_, Infallible>(axum::body::Bytes::new()),
                            (
                                next_idx,
                                chunks,
                                full_response,
                                ctx,
                                lifecycle,
                                registry,
                                trickle_ms,
                                disconnect_pct,
                                stream_fmt,
                                encoding,
                                is_raw_frame,
                            ),
                        ))
                    }
                },
            );

            // Return Response
            let body = axum::body::Body::from_stream(stream);
            let mut response = Response::new(body);

            *response.status_mut() = stream_status;

            if provider.stream.as_ref().unwrap().encoding.as_deref() == Some("aws-event-stream") {
                response.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("application/vnd.amazon.eventstream"),
                );
            } else {
                response.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                response.headers_mut().insert(
                    axum::http::header::CACHE_CONTROL,
                    HeaderValue::from_static("no-cache"),
                );
                response.headers_mut().insert(
                    axum::http::header::CONNECTION,
                    HeaderValue::from_static("keep-alive"),
                );
            }

            return response_with_metrics(response, resolution.metrics);
        }
    }

    // No provider or stream config found
    response_with_metrics(
        (
            StatusCode::NOT_FOUND,
            "No streaming configuration found for this provider.",
        )
            .into_response(),
        resolution.metrics,
    )
}

fn minify_json(s: String) -> String {
    let trimmed = s.trim();
    if (trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']'))
    {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) {
            return serde_json::to_string(&json).unwrap_or(s);
        }
    }
    s
}

fn extract_content_value(json_str: &str) -> (serde_json::Value, bool) {
    // Try to parse as JSON
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(json_str) {
        tracing::debug!("Extracting content from JSON: {:?}", json);
        // OpenAI format: choices[0].message.content or tool_calls
        if let Some(choices) = json.get("choices").and_then(|c| c.as_array()) {
            if let Some(first_choice) = choices.get(0) {
                if let Some(message) = first_choice.get("message") {
                    // Check for tool_calls first
                    if let Some(tool_calls) = message.get("tool_calls") {
                        return (tool_calls.clone(), true);
                    }
                    if let Some(content) = message.get("content") {
                        return (content.clone(), false);
                    }
                }
            }
        }

        // OpenAI Responses API: output[].type=="message" -> content[0].text
        if let Some(output_arr) = json.get("output").and_then(|o| o.as_array()) {
            for item in output_arr {
                if item.get("type").and_then(|t| t.as_str()) == Some("function_call") {
                    return (serde_json::json!([item]), true);
                }
                if item.get("type").and_then(|t| t.as_str()) == Some("message") {
                    if let Some(content_arr) = item.get("content").and_then(|c| c.as_array()) {
                        if let Some(first_block) = content_arr.get(0) {
                            if let Some(text) = first_block.get("text") {
                                return (text.clone(), false);
                            }
                        }
                    }
                }
            }
        }
        // Bedrock Converse: output.message.content[0].text
        if let Some(output) = json.get("output") {
            if let Some(message) = output.get("message") {
                if let Some(content_arr) = message.get("content").and_then(|c| c.as_array()) {
                    if let Some(first_block) = content_arr.get(0) {
                        if let Some(text) = first_block.get("text") {
                            return (text.clone(), false);
                        }
                    }
                }
            }
        }
        // Anthropic format: content[0].text
        if let Some(content) = json.get("content").and_then(|c| c.as_array()) {
            if let Some(first_block) = content.get(0) {
                if let Some(text) = first_block.get("text") {
                    return (text.clone(), false);
                }
            }
        }
        // Gemini/Vertex format: candidates[0].content.parts[0].text
        if let Some(candidates) = json.get("candidates").and_then(|c| c.as_array()) {
            if let Some(first_candidate) = candidates.get(0) {
                if let Some(content) = first_candidate.get("content") {
                    if let Some(parts) = content.get("parts").and_then(|p| p.as_array()) {
                        if let Some(first_part) = parts.get(0) {
                            if let Some(text) = first_part.get("text") {
                                return (text.clone(), false);
                            }
                        }
                    }
                }
            }
        }
    }
    // Fallback: raw string
    (serde_json::Value::String(json_str.to_string()), false)
}

pub async fn echo_handler(
    Extension(state): Extension<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let query_params = HashMap::new();
    let resolution = match resolve_tenant_or_reject(&state, &headers, &query_params) {
        Ok(resolution) => resolution,
        Err(response) => return response,
    };

    // Apply latency simulation (consistent with mock_handler)
    apply_latency(&state.config, &headers).await;

    let mut response = Response::new(axum::body::Body::from(body));
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response_with_metrics(response, resolution.metrics)
}
