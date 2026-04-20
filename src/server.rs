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
 * Core server logic. Sets up the Axum router, middleware (tracing, metrics),
 * and manages the server lifecycle.
 */

use std::sync::Arc;
use tokio::net::TcpListener;

use crate::config::AppConfig;
use crate::handlers::{
    admin_reload_handler, admin_tenant_handler, admin_tenants_handler, echo_handler, health_check,
    mock_handler, models_handler, status_handler, streaming_handler, tenant_handler,
    tenant_reload_handler, AppState,
};
use crate::tenancy::{TenantRequestMetrics, TenantStoreHandle};
// use crate::formats::load_response; // Removed

use axum::{
    extract::Request,
    middleware::{self, Next},
    response::IntoResponse,
    routing::{any, get, post},
    Extension, Router,
};
use metrics_exporter_prometheus::PrometheusHandle;
use tower_http::trace::TraceLayer;

pub async fn start_server(
    config: AppConfig,
    metrics_handle: PrometheusHandle,
    tenants: Arc<TenantStoreHandle>,
) -> Result<(), Box<dyn std::error::Error>> {
    let addr = format!("{}:{}", config.host, config.port);
    let port = config.port;

    // Bind the listener first to catch port-in-use errors early
    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("ERROR: Failed to bind to address {}: {}", addr, e);
            eprintln!(
                "       This usually means the port {} is already in use by another process.",
                port
            );
            eprintln!("       Try using a different port with --port <PORT>.");
            std::process::exit(1);
        }
    };

    // Useful when passing port 0 (which means "pick an available port").
    let local_addr = listener.local_addr().unwrap();

    // Update the config with the actual bound port so the /status endpoint reports it correctly
    let mut config = config;
    config.port = local_addr.port();

    let app = create_app(config, Some(metrics_handle), tenants).await;

    println!("🚀 VidaiMock is running at http://{}", local_addr);
    tracing::info!("Listening on {}", local_addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

pub async fn create_app(
    config: AppConfig,
    metrics_handle: Option<PrometheusHandle>,
    tenants: Arc<TenantStoreHandle>,
) -> Router {
    let state = Arc::new(AppState {
        config: Arc::new(config.clone()),
        tenants,
    });

    let mut app = Router::new()
        .route("/health", get(health_check))
        .route("/status", get(status_handler))
        .route("/admin/reload", post(admin_reload_handler))
        .route("/admin/tenants", get(admin_tenants_handler))
        .route("/admin/tenants/{id}", get(admin_tenant_handler))
        .route("/tenant", get(tenant_handler))
        .route("/tenant/reload", post(tenant_reload_handler));

    // Register endpoints
    let mut registered_paths = std::collections::HashSet::new();

    for endpoint in &config.endpoints {
        if endpoint.format == "echo" {
            app = app.route(&endpoint.path, any(echo_handler));
        } else if endpoint.path.contains("stream") {
            app = app.route(&endpoint.path, post(streaming_handler));
        } else {
            app = app.route(&endpoint.path, post(mock_handler));
        }
        registered_paths.insert(endpoint.path.clone());
    }

    // Default routes for common AI APIs (only if not already registered)
    macro_rules! register_default {
        ($path:expr, $method:ident, $handler:ident) => {
            if !registered_paths.contains($path) {
                app = app.route($path, $method($handler));
                registered_paths.insert($path.to_string());
            }
        };
    }

    register_default!("/v1/chat/completions", post, mock_handler);
    register_default!("/v1/chat/completions/stream", post, mock_handler);
    register_default!("/v1/models", get, models_handler);
    register_default!("/v1/embeddings", post, mock_handler);
    register_default!("/v1/images/generations", post, mock_handler);
    register_default!("/v1/responses", post, mock_handler);
    register_default!("/v1/moderations", post, mock_handler);

    // Error simulator
    register_default!("/error/{code}", post, mock_handler);

    register_default!("/v1/engines/{engine}/embeddings", post, mock_handler);
    register_default!("/v1beta/models/{model_action}", post, mock_handler);
    register_default!("/v1beta/models", get, models_handler);
    register_default!("/v1beta/openai/models", get, models_handler);
    register_default!("/v1beta/openai/embeddings", post, mock_handler);
    // Gemini AI Studio /v1/models paths - POST for generateContent
    register_default!("/v1/models/{model_action}", post, mock_handler);

    // Azure OpenAI paths
    register_default!(
        "/openai/deployments/{deployment}/chat/completions",
        post,
        mock_handler
    );
    register_default!(
        "/openai/deployments/{deployment}/embeddings",
        post,
        mock_handler
    );

    // Anthropic models
    if !registered_paths.contains("/v1/models/{model_action}") {
        app = app.route("/v1/models/{model_action}", get(models_handler));
    }
    register_default!("/v1/messages/stream", post, mock_handler);

    // Bedrock paths
    register_default!("/model/{model_id}/invoke", post, mock_handler);
    register_default!("/model/{model_id}/converse", post, mock_handler);
    register_default!(
        "/model/{model_id}/invoke-with-response-stream",
        post,
        mock_handler
    );
    register_default!("/model/{model_id}/converse-stream", post, mock_handler);

    // Vertex AI paths
    register_default!(
        "/v1/projects/{project}/locations/{location}/publishers/google/models/{model_action}",
        post,
        mock_handler
    );

    // Register metrics endpoint if handle is provided
    if let Some(handle) = metrics_handle {
        app = app.route("/metrics", get(move || std::future::ready(handle.render())));
    }

    app.fallback(post(mock_handler))
        .layer(Extension(state))
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn(track_metrics))
}

async fn track_metrics(req: Request, next: Next) -> impl IntoResponse {
    let start = std::time::Instant::now();
    let path = req.uri().path().to_owned();
    let method = req.method().clone();

    let response = next.run(req).await;

    let latency = start.elapsed().as_secs_f64();
    let status = response.status().as_u16().to_string();

    if let Some(metrics) = response.extensions().get::<TenantRequestMetrics>() {
        match metrics {
            TenantRequestMetrics::Accepted { tenant } => {
                metrics::counter!(
                    "http_requests_total",
                    "method" => method.to_string(),
                    "path" => path.clone(),
                    "status" => status,
                    "tenant" => tenant.clone()
                )
                .increment(1);
                metrics::histogram!(
                    "http_request_duration_seconds",
                    "method" => method.to_string(),
                    "path" => path,
                    "tenant" => tenant.clone()
                )
                .record(latency);
            }
            TenantRequestMetrics::Rejected { reason } => {
                metrics::counter!(
                    "tenant_request_rejections_total",
                    "method" => method.to_string(),
                    "path" => path.clone(),
                    "status" => status,
                    "reason" => (*reason).to_string()
                )
                .increment(1);
                metrics::histogram!(
                    "tenant_request_rejection_duration_seconds",
                    "method" => method.to_string(),
                    "path" => path,
                    "reason" => (*reason).to_string()
                )
                .record(latency);
            }
        }
    } else {
        metrics::counter!(
            "http_requests_total",
            "method" => method.to_string(),
            "path" => path.clone(),
            "status" => status
        )
        .increment(1);
        metrics::histogram!(
            "http_request_duration_seconds",
            "method" => method.to_string(),
            "path" => path
        )
        .record(latency);
    }

    response
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("Signal received, starting graceful shutdown...");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tenancy::{
        build_runtime_store, AdminAuthConfig, TenancyConfig, TenancyMode, TenantRequestMetrics,
        TenantStore, TenantStoreHandle,
    };
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use clap::Parser;
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;
    use tower::ServiceExt; // for oneshot

    fn get_test_config() -> AppConfig {
        AppConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            workers: 1,
            log_level: "debug".to_string(),
            config_dir: PathBuf::from("config"),
            tenancy: crate::tenancy::TenancyConfig::default(),
            latency: crate::config::LatencyConfig {
                mode: "benchmark".to_string(),
                base_ms: 0,
                jitter_pct: 0.0,
            },
            endpoints: vec![crate::config::EndpointConfig {
                path: "/v1/chat/completions".to_string(),
                format: "openai".to_string(),
                content_type: None,
            }],
            response_file: None,
            reload_args: None,
            chaos: crate::config::ChaosConfig {
                enabled: false,
                drop_pct: 0.0,
                malformed_pct: 0.0,
                trickle_ms: 0,
                disconnect_pct: 0.0,
            },
        }
    }

    fn get_test_store() -> Arc<TenantStore> {
        let mut registry = crate::provider::ProviderRegistry::new();
        registry
            .add_provider(crate::provider::ProviderConfig {
                name: "openai".to_string(),
                matcher: "^/v1/chat/completions$".to_string(),
                request_mapping: HashMap::new(),
                response_template: None,
                response_body: Some(
                    r#"{"id": "test-id", "object": "chat.completion", "model": "test-model"}"#
                        .to_string(),
                ),
                stream: None,
                status_code: None,
                error_template: None,
                priority: 0,
            })
            .unwrap();

        Arc::new(TenantStore::new(
            TenancyMode::Single,
            PathBuf::from("config"),
            crate::tenancy::TenancyConfig::default(),
            crate::config::LatencyConfig::default(),
            crate::config::ChaosConfig::default(),
            "x-admin-key".to_string(),
            None,
            "x-tenant".to_string(),
            Arc::new(crate::tenancy::TenantRuntime {
                label: crate::tenancy::DEFAULT_TENANT_ID.to_string(),
                template_metadata: crate::tenancy::TenantTemplateMetadata {
                    id: crate::tenancy::DEFAULT_TENANT_ID.to_string(),
                    ..crate::tenancy::TenantTemplateMetadata::default()
                },
                registry: Arc::new(registry),
                requires_key: false,
                management_auth_header: "x-tenant-admin-key".to_string(),
                management_auth_secret: None,
                latency: crate::config::LatencyConfig::default(),
                chaos: crate::config::ChaosConfig::default(),
            }),
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            std::collections::HashSet::new(),
            std::collections::HashSet::new(),
        ))
    }

    fn get_test_store_handle() -> Arc<TenantStoreHandle> {
        Arc::new(TenantStoreHandle::new(get_test_store()))
    }

    /// Loads the bundled embedded providers (OpenAI, Anthropic, Gemini, etc.)
    /// into the default tenant runtime for wire-shape regression tests.
    fn get_embedded_registry() -> Arc<TenantStoreHandle> {
        let mut registry = crate::provider::ProviderRegistry::new();
        // Pass a non-existent dir so only embedded providers load (filesystem
        // overlay empty). This mirrors the behaviour when a binary is run
        // without --config-dir on a machine without a config/ directory.
        registry
            .load_from_dir(&PathBuf::from("non_existent_dir_regression"))
            .unwrap();
        Arc::new(TenantStoreHandle::new(Arc::new(TenantStore::new(
            TenancyMode::Single,
            PathBuf::from("config"),
            crate::tenancy::TenancyConfig::default(),
            crate::config::LatencyConfig::default(),
            crate::config::ChaosConfig::default(),
            "x-admin-key".to_string(),
            None,
            "x-tenant".to_string(),
            Arc::new(crate::tenancy::TenantRuntime {
                label: crate::tenancy::DEFAULT_TENANT_ID.to_string(),
                template_metadata: crate::tenancy::TenantTemplateMetadata {
                    id: crate::tenancy::DEFAULT_TENANT_ID.to_string(),
                    ..crate::tenancy::TenantTemplateMetadata::default()
                },
                registry: Arc::new(registry),
                requires_key: false,
                management_auth_header: "x-tenant-admin-key".to_string(),
                management_auth_secret: None,
                latency: crate::config::LatencyConfig::default(),
                chaos: crate::config::ChaosConfig::default(),
            }),
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            std::collections::HashSet::new(),
            std::collections::HashSet::new(),
        ))))
    }

    /// Collects an Axum streaming response body into a single Vec<u8>.
    async fn drain_body(resp: axum::http::Response<Body>) -> Vec<u8> {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        bytes.to_vec()
    }

    #[tokio::test]
    async fn test_health_check() {
        let config = get_test_config();
        let app = create_app(config, None, get_test_store_handle()).await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_mock_endpoint() {
        let config = get_test_config();
        let app = create_app(config, None, get_test_store_handle()).await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(body["id"], "test-id");
    }

    #[tokio::test]
    async fn test_echo_endpoint() {
        let mut config = get_test_config();
        config.endpoints = vec![crate::config::EndpointConfig {
            path: "/echo".to_string(),
            format: "echo".to_string(),
            content_type: None,
        }];

        let app = create_app(config, None, get_test_store_handle()).await;
        let body_content = r#"{"test": "echo"}"#;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/echo")
                    .body(Body::from(body_content))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&bytes[..], body_content.as_bytes());
    }

    #[tokio::test]
    async fn test_multiprovider_endpoints() {
        let config = get_test_config();

        // Setup registry with multiple providers
        let mut registry = crate::provider::ProviderRegistry::new();

        // OpenAI
        registry
            .add_provider(crate::provider::ProviderConfig {
                name: "openai".to_string(),
                matcher: "^/v1/chat/completions$".to_string(),
                request_mapping: HashMap::new(),
                response_template: None,
                response_body: Some(r#"{"object": "chat.completion"}"#.to_string()),
                stream: None,
                status_code: None,
                error_template: None,
                priority: 0,
            })
            .unwrap();

        // Anthropic
        registry
            .add_provider(crate::provider::ProviderConfig {
                name: "anthropic".to_string(),
                matcher: "^/v1/messages$".to_string(),
                request_mapping: HashMap::new(),
                response_template: None,
                response_body: Some(r#"{"type": "message"}"#.to_string()),
                stream: None,
                status_code: None,
                error_template: None,
                priority: 0,
            })
            .unwrap();

        // Gemini
        registry
            .add_provider(crate::provider::ProviderConfig {
                name: "gemini".to_string(),
                matcher: "^/gemini$".to_string(),
                request_mapping: HashMap::new(),
                response_template: None,
                response_body: Some(r#"{"candidates": []}"#.to_string()),
                stream: None,
                status_code: None,
                error_template: None,
                priority: 0,
            })
            .unwrap();

        let app = create_app(
            config,
            None,
            Arc::new(TenantStoreHandle::new(Arc::new(TenantStore::new(
                TenancyMode::Single,
                PathBuf::from("config"),
                crate::tenancy::TenancyConfig::default(),
                crate::config::LatencyConfig::default(),
                crate::config::ChaosConfig::default(),
                "x-admin-key".to_string(),
                None,
                "x-tenant".to_string(),
                Arc::new(crate::tenancy::TenantRuntime {
                    label: crate::tenancy::DEFAULT_TENANT_ID.to_string(),
                    template_metadata: crate::tenancy::TenantTemplateMetadata {
                        id: crate::tenancy::DEFAULT_TENANT_ID.to_string(),
                        ..crate::tenancy::TenantTemplateMetadata::default()
                    },
                    registry: Arc::new(registry),
                    requires_key: false,
                    management_auth_header: "x-tenant-admin-key".to_string(),
                    management_auth_secret: None,
                    latency: crate::config::LatencyConfig::default(),
                    chaos: crate::config::ChaosConfig::default(),
                }),
                HashMap::new(),
                HashMap::new(),
                HashMap::new(),
                std::collections::HashSet::new(),
                std::collections::HashSet::new(),
            )))),
        )
        .await;

        // Test OpenAI
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(body_str.contains("chat.completion"));

        // Test Anthropic
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/messages")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(body_str.contains("type\": \"message"));

        // Test Gemini
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/gemini")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(body_str.contains("candidates"));
    }

    #[tokio::test]
    async fn test_status_endpoint() {
        let config = get_test_config();
        let app = create_app(config, None, get_test_store_handle()).await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(body["status"], "ok");
        assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(body["port"], 0);
        assert_eq!(body.as_object().unwrap().len(), 3);
        assert!(body.get("tenancy").is_none());
        assert!(body.get("latency").is_none());
        assert!(body.get("endpoints").is_none());
    }

    fn write_provider(path: &std::path::Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            format!(
                "name: \"openai\"\nmatcher: \"^/v1/chat/completions$\"\nresponse_body: '{}'\npriority: 100\n",
                body.replace('\'', "''")
            ),
        )
        .unwrap();
    }

    fn write_streaming_provider(path: &std::path::Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            format!(
                "name: \"openai\"\nmatcher: \"^/v1/chat/completions$\"\nresponse_body: '{}'\nstream:\n  enabled: true\npriority: 100\n",
                body.replace('\'', "''")
            ),
        )
        .unwrap();
    }

    fn write_tenant_metadata(base_dir: &std::path::Path, tenant_id: &str, body: &str) {
        let metadata_path = base_dir.join("tenants").join(tenant_id).join("tenant.toml");
        fs::create_dir_all(metadata_path.parent().unwrap()).unwrap();
        fs::write(metadata_path, body).unwrap();
    }

    fn multi_tenant_config(base_dir: &std::path::Path) -> AppConfig {
        write_tenant_metadata(
            base_dir,
            "acme",
            r#"
id = "acme"
"#,
        );
        write_tenant_metadata(
            base_dir,
            "globex",
            r#"
id = "globex"
"#,
        );

        AppConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            workers: 1,
            log_level: "debug".to_string(),
            config_dir: PathBuf::from("config"),
            tenancy: TenancyConfig {
                mode: TenancyMode::Multi,
                tenants_dir: base_dir.join("tenants"),
                tenant_header: "x-tenant".to_string(),
                admin_auth: AdminAuthConfig::default(),
            },
            latency: crate::config::LatencyConfig::default(),
            chaos: crate::config::ChaosConfig::default(),
            endpoints: vec![crate::config::EndpointConfig {
                path: "/v1/chat/completions".to_string(),
                format: "openai".to_string(),
                content_type: None,
            }],
            response_file: None,
            reload_args: None,
        }
    }

    fn managed_multi_tenant_config(base_dir: &std::path::Path) -> AppConfig {
        let secret_dir = base_dir.join("secrets");
        fs::create_dir_all(&secret_dir).unwrap();
        let acme_key_path = secret_dir.join("acme.key");
        let acme_admin_key_path = secret_dir.join("acme-admin.key");
        fs::write(&acme_key_path, "secret-acme").unwrap();
        fs::write(&acme_admin_key_path, "tenant-admin-acme").unwrap();
        write_tenant_metadata(
            base_dir,
            "acme",
            &format!(
                r#"
id = "acme"
display_name = "Acme Corp"

[labels]
tier = "gold"
region = "eu-west"

[[keys]]
source = "header"
name = "x-api-key"
value_file = "{}"

[management_auth]
header = "x-tenant-admin-key"
value_file = "{}"
"#,
                toml_path(&acme_key_path),
                toml_path(&acme_admin_key_path)
            ),
        );
        write_tenant_metadata(
            base_dir,
            "globex",
            r#"
id = "globex"
display_name = "Globex"

[labels]
tier = "silver"
region = "us-east"

[[keys]]
source = "header"
name = "x-api-key"
value = "secret-globex"

[management_auth]
header = "x-tenant-admin-key"
value = "tenant-admin-globex"
"#,
        );

        AppConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            workers: 1,
            log_level: "debug".to_string(),
            config_dir: PathBuf::from("config"),
            tenancy: TenancyConfig {
                mode: TenancyMode::Multi,
                tenants_dir: base_dir.join("tenants"),
                tenant_header: "x-tenant".to_string(),
                admin_auth: AdminAuthConfig {
                    header: "x-admin-key".to_string(),
                    value: "global-admin-secret".to_string(),
                    value_file: None,
                    value_env: None,
                },
            },
            latency: crate::config::LatencyConfig::default(),
            chaos: crate::config::ChaosConfig::default(),
            endpoints: vec![crate::config::EndpointConfig {
                path: "/v1/chat/completions".to_string(),
                format: "openai".to_string(),
                content_type: None,
            }],
            response_file: None,
            reload_args: None,
        }
    }

    fn unmanaged_admin_multi_tenant_config(base_dir: &std::path::Path) -> AppConfig {
        let mut config = managed_multi_tenant_config(base_dir);
        config.tenancy.admin_auth.value.clear();
        config
    }

    fn single_mode_management_config(base_dir: &std::path::Path) -> AppConfig {
        AppConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            workers: 1,
            log_level: "debug".to_string(),
            config_dir: base_dir.join("config"),
            tenancy: TenancyConfig {
                mode: TenancyMode::Single,
                tenants_dir: base_dir.join("tenants"),
                tenant_header: "x-tenant".to_string(),
                admin_auth: AdminAuthConfig {
                    header: "x-admin-key".to_string(),
                    value: "global-admin-secret".to_string(),
                    value_file: None,
                    value_env: None,
                },
            },
            latency: crate::config::LatencyConfig::default(),
            chaos: crate::config::ChaosConfig::default(),
            endpoints: vec![crate::config::EndpointConfig {
                path: "/v1/chat/completions".to_string(),
                format: "openai".to_string(),
                content_type: None,
            }],
            response_file: None,
            reload_args: None,
        }
    }

    fn management_test_base(name: &str) -> PathBuf {
        let temp_base = std::env::current_dir()
            .unwrap()
            .join(format!("target/{}", name));
        if temp_base.exists() {
            fs::remove_dir_all(&temp_base).unwrap();
        }
        temp_base
    }

    async fn response_text(response: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn toml_path(value: &std::path::Path) -> String {
        value.display().to_string().replace('\\', "\\\\")
    }

    fn write_file_backed_management_config(
        base_dir: &std::path::Path,
        admin_key: &str,
        acme_key: &str,
        duplicate_acme: bool,
    ) -> PathBuf {
        fs::create_dir_all(base_dir).unwrap();
        let config_path = base_dir.join("mock-server.toml");
        fs::write(
            &config_path,
            format!(
                r#"
port = 8100
workers = 1
log_level = "debug"
config_dir = "{config_dir}"

[tenancy]
mode = "multi"
tenants_dir = "{tenants_dir}"
tenant_header = "x-tenant"

[tenancy.admin_auth]
header = "x-admin-key"
value = "{admin_key}"
"#,
                config_dir = toml_path(&base_dir.join("config")),
                tenants_dir = toml_path(&base_dir.join("tenants")),
                admin_key = admin_key,
            ),
        )
        .unwrap();

        write_tenant_metadata(
            base_dir,
            "acme",
            &format!(
                r#"
id = "acme"

[[keys]]
source = "header"
name = "x-api-key"
value = "{}"
"#,
                acme_key
            ),
        );
        write_tenant_metadata(
            base_dir,
            "globex",
            r#"
id = "globex"

[[keys]]
source = "header"
name = "x-api-key"
value = "secret-globex"
"#,
        );

        if duplicate_acme {
            write_tenant_metadata(
                base_dir,
                "globex-copy",
                r#"
id = "acme"

[[keys]]
source = "header"
name = "x-api-key"
value = "secret-globex"
"#,
            );
        } else {
            let duplicate_dir = base_dir.join("tenants/globex-copy");
            if duplicate_dir.exists() {
                fs::remove_dir_all(duplicate_dir).unwrap();
            }
        }

        config_path
    }

    fn load_file_backed_management_config(config_path: &std::path::Path) -> AppConfig {
        let args = crate::config::Cli::parse_from(&[
            "mock-server",
            "--config",
            config_path.to_str().unwrap(),
        ]);
        AppConfig::build_config(args).unwrap()
    }

    #[tokio::test]
    async fn test_same_path_different_tenants_use_different_runtimes() {
        let temp_base = std::env::current_dir()
            .unwrap()
            .join("target/test_multi_tenant_runtimes");
        if temp_base.exists() {
            fs::remove_dir_all(&temp_base).unwrap();
        }

        write_provider(
            &temp_base.join("tenants/default/providers/openai.yaml"),
            r#"{"tenant":"default"}"#,
        );
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme"}"#,
        );
        write_provider(
            &temp_base.join("tenants/globex/providers/openai.yaml"),
            r#"{"tenant":"globex"}"#,
        );

        let config = multi_tenant_config(&temp_base);
        let store = Arc::new(TenantStoreHandle::new(
            build_runtime_store(&config).unwrap(),
        ));
        let app = create_app(config, None, store).await;

        let default_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let default_bytes = axum::body::to_bytes(default_response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(String::from_utf8(default_bytes.to_vec())
            .unwrap()
            .contains("\"default\""));

        let acme_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-tenant", "acme")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let acme_bytes = axum::body::to_bytes(acme_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let acme_body = String::from_utf8(acme_bytes.to_vec()).unwrap();
        assert!(acme_body.contains("\"acme\""));
        assert!(!acme_body.contains("\"globex\""));

        let globex_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-tenant", "globex")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let globex_bytes = axum::body::to_bytes(globex_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let globex_body = String::from_utf8(globex_bytes.to_vec()).unwrap();
        assert!(globex_body.contains("\"globex\""));
        assert!(!globex_body.contains("\"acme\""));

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_models_endpoint_is_tenant_isolated() {
        let temp_base = management_test_base("test_models_endpoint_tenant_isolation");
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme"}"#,
        );
        write_provider(
            &temp_base.join("tenants/globex/providers/openai.yaml"),
            r#"{"tenant":"globex"}"#,
        );

        fs::write(
            temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"
name: "acme-openai"
matcher: "^/v1/chat/completions$"
response_body: '{"tenant":"acme"}'
priority: 100
"#,
        )
        .unwrap();
        fs::write(
            temp_base.join("tenants/globex/providers/openai.yaml"),
            r#"
name: "globex-openai"
matcher: "^/v1/chat/completions$"
response_body: '{"tenant":"globex"}'
priority: 100
"#,
        )
        .unwrap();

        let config = multi_tenant_config(&temp_base);
        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        let acme_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/models")
                    .header("x-tenant", "acme")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(acme_response.status(), StatusCode::OK);
        let acme_body: serde_json::Value =
            serde_json::from_str(&response_text(acme_response).await).unwrap();
        let acme_models = acme_body["data"].to_string();
        assert!(acme_models.contains("acme-openai"));
        assert!(!acme_models.contains("globex-openai"));

        let globex_response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/models")
                    .header("x-tenant", "globex")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(globex_response.status(), StatusCode::OK);
        let globex_body: serde_json::Value =
            serde_json::from_str(&response_text(globex_response).await).unwrap();
        let globex_models = globex_body["data"].to_string();
        assert!(globex_models.contains("globex-openai"));
        assert!(!globex_models.contains("acme-openai"));

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_models_endpoint_resolves_tenant_from_query_key() {
        let temp_base = management_test_base("test_models_endpoint_query_tenant_resolution");
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme"}"#,
        );
        write_provider(
            &temp_base.join("tenants/globex/providers/openai.yaml"),
            r#"{"tenant":"globex"}"#,
        );

        fs::write(
            temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"
name: "acme-openai"
matcher: "^/v1/chat/completions$"
response_body: '{"tenant":"acme"}'
priority: 100
"#,
        )
        .unwrap();
        fs::write(
            temp_base.join("tenants/globex/providers/openai.yaml"),
            r#"
name: "globex-openai"
matcher: "^/v1/chat/completions$"
response_body: '{"tenant":"globex"}'
priority: 100
"#,
        )
        .unwrap();

        let config = AppConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            workers: 1,
            log_level: "debug".to_string(),
            config_dir: PathBuf::from("config"),
            tenancy: TenancyConfig {
                mode: TenancyMode::Multi,
                tenants_dir: temp_base.join("tenants"),
                tenant_header: "x-tenant".to_string(),
                admin_auth: AdminAuthConfig::default(),
            },
            latency: crate::config::LatencyConfig::default(),
            chaos: crate::config::ChaosConfig::default(),
            endpoints: vec![crate::config::EndpointConfig {
                path: "/v1/chat/completions".to_string(),
                format: "openai".to_string(),
                content_type: None,
            }],
            response_file: None,
            reload_args: None,
        };
        write_tenant_metadata(
            &temp_base,
            "acme",
            r#"
id = "acme"

[[keys]]
source = "query"
name = "api_key"
value = "secret-acme"
"#,
        );
        write_tenant_metadata(
            &temp_base,
            "globex",
            r#"
id = "globex"

[[keys]]
source = "query"
name = "api_key"
value = "secret-globex"
"#,
        );
        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        let acme_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/models?api_key=secret-acme")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(acme_response.status(), StatusCode::OK);
        let acme_body: serde_json::Value =
            serde_json::from_str(&response_text(acme_response).await).unwrap();
        let acme_models = acme_body["data"].to_string();
        assert!(acme_models.contains("acme-openai"));
        assert!(!acme_models.contains("globex-openai"));

        let globex_response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/models?api_key=secret-globex")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(globex_response.status(), StatusCode::OK);
        let globex_body: serde_json::Value =
            serde_json::from_str(&response_text(globex_response).await).unwrap();
        let globex_models = globex_body["data"].to_string();
        assert!(globex_models.contains("globex-openai"));
        assert!(!globex_models.contains("acme-openai"));

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_multi_mode_tenant_policies_change_latency_and_chaos_defaults() {
        let temp_base = management_test_base("test_multi_mode_tenant_policy_defaults");
        write_provider(
            &temp_base.join("tenants/default/providers/openai.yaml"),
            r#"{"tenant":"default"}"#,
        );
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme"}"#,
        );
        write_provider(
            &temp_base.join("tenants/globex/providers/openai.yaml"),
            r#"{"tenant":"globex"}"#,
        );
        let mut config = multi_tenant_config(&temp_base);
        config.latency.base_ms = 0;
        config.latency.jitter_pct = 0.0;
        config.chaos.enabled = true;
        config.chaos.drop_pct = 0.0;
        write_tenant_metadata(
            &temp_base,
            "acme",
            r#"
id = "acme"

[latency]
base_ms = 70
jitter_pct = 0.0
"#,
        );
        write_tenant_metadata(
            &temp_base,
            "globex",
            r#"
id = "globex"

[chaos]
drop_pct = 100.0
"#,
        );
        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        let acme_start = std::time::Instant::now();
        let acme_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-tenant", "acme")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let acme_elapsed = acme_start.elapsed();
        assert_eq!(acme_response.status(), StatusCode::OK);
        assert!(
            acme_elapsed >= std::time::Duration::from_millis(45),
            "expected acme tenant latency override to apply, got {:?}",
            acme_elapsed
        );
        assert!(response_text(acme_response).await.contains("\"acme\""));

        let globex_response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-tenant", "globex")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(globex_response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_default_tenant_fallback_uses_default_tenant_policy() {
        let temp_base = management_test_base("test_default_tenant_policy_fallback");
        write_provider(
            &temp_base.join("tenants/default/providers/openai.yaml"),
            r#"{"tenant":"default"}"#,
        );
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme"}"#,
        );
        let mut config = multi_tenant_config(&temp_base);
        config.chaos.enabled = true;
        config.chaos.drop_pct = 0.0;
        write_tenant_metadata(
            &temp_base,
            "default",
            r#"
id = "default"

[chaos]
drop_pct = 100.0
"#,
        );
        write_tenant_metadata(
            &temp_base,
            "acme",
            r#"
id = "acme"
"#,
        );
        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        let default_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(default_response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let acme_response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-tenant", "acme")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(acme_response.status(), StatusCode::OK);
        assert!(response_text(acme_response).await.contains("\"acme\""));

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_request_headers_override_tenant_policy_per_request() {
        let temp_base = management_test_base("test_request_headers_override_tenant_policy");
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme"}"#,
        );
        let mut config = multi_tenant_config(&temp_base);
        config.chaos.enabled = true;
        config.chaos.drop_pct = 0.0;
        write_tenant_metadata(
            &temp_base,
            "acme",
            r#"
id = "acme"

[latency]
base_ms = 120
jitter_pct = 0.0

[chaos]
drop_pct = 0.0
"#,
        );
        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        let baseline_start = std::time::Instant::now();
        let baseline_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-tenant", "acme")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let baseline_elapsed = baseline_start.elapsed();
        assert_eq!(baseline_response.status(), StatusCode::OK);
        assert!(
            baseline_elapsed >= std::time::Duration::from_millis(90),
            "expected tenant latency default to apply, got {:?}",
            baseline_elapsed
        );

        let override_start = std::time::Instant::now();
        let override_response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-tenant", "acme")
                    .header("x-vidai-latency", "0")
                    .header("x-vidai-chaos-drop", "100")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let override_elapsed = override_start.elapsed();
        assert_eq!(
            override_response.status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert!(
            override_elapsed + std::time::Duration::from_millis(50) < baseline_elapsed,
            "expected request headers to beat tenant defaults, baseline {:?}, override {:?}",
            baseline_elapsed,
            override_elapsed
        );

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_single_mode_latency_and_chaos_still_use_global_defaults() {
        let temp_base = management_test_base("test_single_mode_global_latency_and_chaos");
        write_provider(
            &temp_base.join("config/providers/openai.yaml"),
            r#"{"tenant":"single"}"#,
        );

        let config = AppConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            workers: 1,
            log_level: "debug".to_string(),
            config_dir: temp_base.join("config"),
            tenancy: TenancyConfig {
                mode: TenancyMode::Single,
                tenants_dir: temp_base.join("tenants"),
                tenant_header: "x-tenant".to_string(),
                admin_auth: AdminAuthConfig::default(),
            },
            latency: crate::config::LatencyConfig {
                mode: "benchmark".to_string(),
                base_ms: 70,
                jitter_pct: 0.0,
            },
            chaos: crate::config::ChaosConfig {
                enabled: true,
                malformed_pct: 0.0,
                drop_pct: 100.0,
                trickle_ms: 0,
                disconnect_pct: 0.0,
            },
            endpoints: vec![crate::config::EndpointConfig {
                path: "/v1/chat/completions".to_string(),
                format: "openai".to_string(),
                content_type: None,
            }],
            response_file: None,
            reload_args: None,
        };
        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        let start = std::time::Instant::now();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let elapsed = start.elapsed();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(
            elapsed >= std::time::Duration::from_millis(45),
            "expected single mode to keep global latency defaults, got {:?}",
            elapsed
        );

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_normal_render_path_includes_resolved_tenant_metadata() {
        let temp_base = management_test_base("test_normal_render_tenant_metadata");
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant_id":"{{ tenant.id }}","display_name":"{{ tenant.display_name }}","tier":"{{ tenant.labels.tier }}","has_management_auth":"{{ tenant.management_auth is defined }}"}"#,
        );

        let config = managed_multi_tenant_config(&temp_base);
        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-api-key", "secret-acme")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_text(response).await;
        assert!(body.contains("\"tenant_id\":\"acme\""));
        assert!(body.contains("\"display_name\":\"Acme Corp\""));
        assert!(body.contains("\"tier\":\"gold\""));
        assert!(body.contains("\"has_management_auth\":\"false\""));

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_streaming_render_path_includes_resolved_tenant_metadata() {
        let temp_base = management_test_base("test_streaming_render_tenant_metadata");
        write_streaming_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"choices":[{"message":{"content":"{{ tenant.id }}|{{ tenant.display_name }}|{{ tenant.labels.region }}"}}]}"#,
        );

        let config = managed_multi_tenant_config(&temp_base);
        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-api-key", "secret-acme")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"stream":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_text(response).await;
        assert!(body.contains("acme"));
        assert!(body.contains("Acme"));
        assert!(body.contains("Corp"));
        assert!(body.contains("eu-west"));

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_tenant_resolution_failures_are_generic_externally_but_structured_internally() {
        let temp_base = management_test_base("test_tenant_rejections_are_generic");
        write_provider(
            &temp_base.join("tenants/default/providers/openai.yaml"),
            r#"{"tenant":"default"}"#,
        );
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme"}"#,
        );
        write_provider(
            &temp_base.join("tenants/globex/providers/openai.yaml"),
            r#"{"tenant":"globex"}"#,
        );

        let config = managed_multi_tenant_config(&temp_base);
        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        let scenarios = vec![
            (
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-tenant", "missing")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
                "unknown_tenant",
            ),
            (
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-api-key", "unknown")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
                "unknown_key",
            ),
            (
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-tenant", "acme")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
                "missing_key",
            ),
            (
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-tenant", "globex")
                    .header("x-api-key", "secret-acme")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
                "header_key_conflict",
            ),
        ];

        for (request, expected_reason) in scenarios {
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            let metrics = response
                .extensions()
                .get::<TenantRequestMetrics>()
                .cloned()
                .expect("rejected tenant response should include rejection metrics");
            let body = response_text(response).await;
            assert_eq!(body, "Tenant authentication failed.");
            match metrics {
                TenantRequestMetrics::Rejected { reason } => assert_eq!(reason, expected_reason),
                TenantRequestMetrics::Accepted { tenant } => {
                    panic!(
                        "expected rejection metrics, got accepted tenant label {}",
                        tenant
                    )
                }
            }
        }

        let accepted = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-api-key", "secret-acme")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(accepted.status(), StatusCode::OK);
        let accepted_metrics = accepted
            .extensions()
            .get::<TenantRequestMetrics>()
            .cloned()
            .expect("accepted tenant response should include tenant metrics");
        match accepted_metrics {
            TenantRequestMetrics::Accepted { tenant } => assert_eq!(tenant, "acme"),
            TenantRequestMetrics::Rejected { reason } => {
                panic!(
                    "expected accepted tenant metrics, got rejection reason {}",
                    reason
                )
            }
        }

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_global_admin_can_list_tenants() {
        let temp_base = management_test_base("test_admin_lists_tenants");
        write_provider(
            &temp_base.join("tenants/default/providers/openai.yaml"),
            r#"{"tenant":"default"}"#,
        );
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme"}"#,
        );
        write_provider(
            &temp_base.join("tenants/globex/providers/openai.yaml"),
            r#"{"tenant":"globex"}"#,
        );

        let config = managed_multi_tenant_config(&temp_base);
        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/admin/tenants")
                    .header("x-admin-key", "global-admin-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_str(&response_text(response).await).unwrap();
        assert_eq!(body["mode"], "multi");
        assert_eq!(body["tenants"].as_array().unwrap().len(), 3);
        assert!(body["tenants"].to_string().contains("\"default\""));
        assert!(body["tenants"].to_string().contains("\"acme\""));
        assert!(body["tenants"].to_string().contains("\"globex\""));

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_admin_endpoints_fail_when_admin_auth_is_unset() {
        let temp_base = management_test_base("test_admin_requires_configured_auth");
        write_provider(
            &temp_base.join("tenants/default/providers/openai.yaml"),
            r#"{"tenant":"default"}"#,
        );
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme"}"#,
        );
        write_provider(
            &temp_base.join("tenants/globex/providers/openai.yaml"),
            r#"{"tenant":"globex"}"#,
        );

        let config = unmanaged_admin_multi_tenant_config(&temp_base);
        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        let list_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/admin/tenants")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list_response.status(), StatusCode::UNAUTHORIZED);

        let inspect_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/admin/tenants/acme")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(inspect_response.status(), StatusCode::UNAUTHORIZED);

        let reload_response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/reload")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reload_response.status(), StatusCode::UNAUTHORIZED);

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_admin_endpoints_accept_authorization_bearer_secret() {
        let temp_base = management_test_base("test_admin_accepts_bearer_secret");
        write_provider(
            &temp_base.join("tenants/default/providers/openai.yaml"),
            r#"{"tenant":"default"}"#,
        );
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme"}"#,
        );
        write_provider(
            &temp_base.join("tenants/globex/providers/openai.yaml"),
            r#"{"tenant":"globex"}"#,
        );

        let mut config = managed_multi_tenant_config(&temp_base);
        config.tenancy.admin_auth.header = "authorization".to_string();
        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/admin/tenants")
                    .header("authorization", "Bearer global-admin-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_global_admin_can_inspect_one_tenant() {
        let temp_base = management_test_base("test_admin_inspects_tenant");
        write_provider(
            &temp_base.join("tenants/default/providers/openai.yaml"),
            r#"{"tenant":"default"}"#,
        );
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme"}"#,
        );
        write_provider(
            &temp_base.join("tenants/globex/providers/openai.yaml"),
            r#"{"tenant":"globex"}"#,
        );

        let config = managed_multi_tenant_config(&temp_base);
        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/admin/tenants/acme")
                    .header("x-admin-key", "global-admin-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_str(&response_text(response).await).unwrap();
        assert_eq!(body["id"], "acme");
        assert_eq!(body["is_default"], false);
        assert_eq!(body["requires_key"], true);

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_global_admin_can_trigger_reload() {
        let temp_base = management_test_base("test_admin_reload");
        write_provider(
            &temp_base.join("tenants/default/providers/openai.yaml"),
            r#"{"tenant":"default"}"#,
        );
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme-before"}"#,
        );
        write_provider(
            &temp_base.join("tenants/globex/providers/openai.yaml"),
            r#"{"tenant":"globex"}"#,
        );

        let config_path = write_file_backed_management_config(
            &temp_base,
            "global-admin-secret",
            "secret-acme",
            false,
        );
        let config = load_file_backed_management_config(&config_path);
        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        let before = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-api-key", "secret-acme")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response_text(before).await.contains("acme-before"));

        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme-after"}"#,
        );

        let reload = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/reload")
                    .header("x-admin-key", "global-admin-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reload.status(), StatusCode::OK);
        assert!(response_text(reload).await.contains("acme"));

        let after = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-api-key", "secret-acme")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response_text(after).await.contains("acme-after"));

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_admin_reload_rereads_config_file_changes() {
        let temp_base = management_test_base("test_admin_reload_rereads_config_file");
        write_provider(
            &temp_base.join("tenants/default/providers/openai.yaml"),
            r#"{"tenant":"default"}"#,
        );
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme"}"#,
        );
        write_provider(
            &temp_base.join("tenants/globex/providers/openai.yaml"),
            r#"{"tenant":"globex"}"#,
        );

        let config_path = write_file_backed_management_config(
            &temp_base,
            "global-admin-secret",
            "secret-acme",
            false,
        );
        let config = load_file_backed_management_config(&config_path);
        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        write_file_backed_management_config(
            &temp_base,
            "global-admin-secret",
            "secret-acme-rotated",
            false,
        );

        let reload = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/reload")
                    .header("x-admin-key", "global-admin-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reload.status(), StatusCode::OK);

        let old_key_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-api-key", "secret-acme")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(old_key_response.status(), StatusCode::UNAUTHORIZED);

        let new_key_response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-api-key", "secret-acme-rotated")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(new_key_response.status(), StatusCode::OK);
        assert!(response_text(new_key_response).await.contains("acme"));

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_failed_config_file_reload_keeps_previous_runtime_active() {
        let temp_base = management_test_base("test_failed_config_reload_keeps_runtime");
        write_provider(
            &temp_base.join("tenants/default/providers/openai.yaml"),
            r#"{"tenant":"default"}"#,
        );
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme"}"#,
        );
        write_provider(
            &temp_base.join("tenants/globex/providers/openai.yaml"),
            r#"{"tenant":"globex"}"#,
        );

        let config_path = write_file_backed_management_config(
            &temp_base,
            "global-admin-secret",
            "secret-acme",
            false,
        );
        let config = load_file_backed_management_config(&config_path);
        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        write_file_backed_management_config(&temp_base, "global-admin-secret", "secret-acme", true);

        let reload = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/reload")
                    .header("x-admin-key", "global-admin-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reload.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let after_failed_reload = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-api-key", "secret-acme")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(after_failed_reload.status(), StatusCode::OK);
        assert!(response_text(after_failed_reload).await.contains("acme"));

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_admin_reload_rejects_restart_required_config_changes() {
        let temp_base = management_test_base("test_admin_reload_rejects_restart_required_changes");
        write_provider(
            &temp_base.join("tenants/default/providers/openai.yaml"),
            r#"{"tenant":"default"}"#,
        );
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme-before"}"#,
        );
        write_provider(
            &temp_base.join("tenants/globex/providers/openai.yaml"),
            r#"{"tenant":"globex"}"#,
        );

        let config_path = write_file_backed_management_config(
            &temp_base,
            "global-admin-secret",
            "secret-acme",
            false,
        );
        let config = load_file_backed_management_config(&config_path);
        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme-after"}"#,
        );
        fs::write(
            &config_path,
            format!(
                r#"
port = 8100
workers = 1
log_level = "debug"
config_dir = "{config_dir}"

[latency]
mode = "realistic"
base_ms = 25
jitter_pct = 0.0

[tenancy]
mode = "multi"
tenants_dir = "{tenants_dir}"
tenant_header = "x-tenant"

[tenancy.admin_auth]
header = "x-admin-key"
value = "global-admin-secret"
"#,
                config_dir = toml_path(&temp_base.join("config")),
                tenants_dir = toml_path(&temp_base.join("tenants")),
            ),
        )
        .unwrap();

        let reload = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/reload")
                    .header("x-admin-key", "global-admin-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reload.status(), StatusCode::CONFLICT);
        assert!(response_text(reload).await.contains("latency"));

        let after_failed_reload = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-api-key", "secret-acme")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(after_failed_reload.status(), StatusCode::OK);
        assert!(response_text(after_failed_reload)
            .await
            .contains("acme-before"));

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_tenant_admin_can_inspect_own_tenant() {
        let temp_base = management_test_base("test_tenant_inspects_own");
        write_provider(
            &temp_base.join("tenants/default/providers/openai.yaml"),
            r#"{"tenant":"default"}"#,
        );
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme"}"#,
        );
        write_provider(
            &temp_base.join("tenants/globex/providers/openai.yaml"),
            r#"{"tenant":"globex"}"#,
        );

        let config = managed_multi_tenant_config(&temp_base);
        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/tenant")
                    .header("x-tenant-admin-key", "tenant-admin-acme")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_str(&response_text(response).await).unwrap();
        assert_eq!(body["id"], "acme");

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_tenant_request_key_can_call_mock_endpoint_but_cannot_reload_tenant() {
        let temp_base = management_test_base("test_request_key_cannot_manage_tenant");
        write_provider(
            &temp_base.join("tenants/default/providers/openai.yaml"),
            r#"{"tenant":"default"}"#,
        );
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme"}"#,
        );

        let config = managed_multi_tenant_config(&temp_base);
        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        let mock_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-api-key", "secret-acme")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(mock_response.status(), StatusCode::OK);

        let tenant_reload = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tenant/reload")
                    .header("x-api-key", "secret-acme")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(tenant_reload.status(), StatusCode::UNAUTHORIZED);

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_tenant_admin_can_reload_own_tenant() {
        let temp_base = management_test_base("test_tenant_reload_own");
        write_provider(
            &temp_base.join("tenants/default/providers/openai.yaml"),
            r#"{"tenant":"default"}"#,
        );
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme-before"}"#,
        );
        write_provider(
            &temp_base.join("tenants/globex/providers/openai.yaml"),
            r#"{"tenant":"globex"}"#,
        );

        let config = managed_multi_tenant_config(&temp_base);
        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme-after"}"#,
        );

        let reload = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tenant/reload")
                    .header("x-tenant-admin-key", "tenant-admin-acme")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reload.status(), StatusCode::OK);
        assert!(response_text(reload).await.contains("acme"));

        let acme_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-api-key", "secret-acme")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response_text(acme_response).await.contains("acme-after"));

        let globex_response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-api-key", "secret-globex")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response_text(globex_response).await.contains("globex"));

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_failed_tenant_reload_keeps_previous_runtime_active() {
        let temp_base = management_test_base("test_failed_tenant_reload_keeps_runtime");
        write_provider(
            &temp_base.join("tenants/default/providers/openai.yaml"),
            r#"{"tenant":"default"}"#,
        );
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme-before"}"#,
        );
        write_provider(
            &temp_base.join("tenants/globex/providers/openai.yaml"),
            r#"{"tenant":"globex"}"#,
        );

        let config = managed_multi_tenant_config(&temp_base);
        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        let before = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-api-key", "secret-acme")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response_text(before).await.contains("acme-before"));

        fs::remove_file(temp_base.join("secrets/acme.key")).unwrap();

        let reload = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tenant/reload")
                    .header("x-tenant-admin-key", "tenant-admin-acme")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reload.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let after_failed_reload = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-api-key", "secret-acme")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response_text(after_failed_reload)
            .await
            .contains("acme-before"));

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_tenant_reload_collision_fails_and_keeps_previous_runtime_and_auth_state() {
        let temp_base = management_test_base("test_tenant_reload_collision_keeps_runtime");
        write_provider(
            &temp_base.join("tenants/default/providers/openai.yaml"),
            r#"{"tenant":"default"}"#,
        );
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme-before"}"#,
        );
        write_provider(
            &temp_base.join("tenants/globex/providers/openai.yaml"),
            r#"{"tenant":"globex"}"#,
        );

        let config = managed_multi_tenant_config(&temp_base);
        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        let before = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-api-key", "secret-acme")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response_text(before).await.contains("acme-before"));

        fs::write(temp_base.join("secrets/acme.key"), "secret-globex").unwrap();
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme-after"}"#,
        );

        let reload = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tenant/reload")
                    .header("x-tenant-admin-key", "tenant-admin-acme")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reload.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let old_key_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-api-key", "secret-acme")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(old_key_response.status(), StatusCode::OK);
        assert!(response_text(old_key_response)
            .await
            .contains("acme-before"));

        let globex_response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-api-key", "secret-globex")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(globex_response.status(), StatusCode::OK);
        assert!(response_text(globex_response).await.contains("globex"));

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_tenant_reload_management_auth_conflict_keeps_previous_runtime_and_admin_auth() {
        let temp_base = management_test_base("test_tenant_reload_management_auth_conflict");
        write_provider(
            &temp_base.join("tenants/default/providers/openai.yaml"),
            r#"{"tenant":"default"}"#,
        );
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme-before"}"#,
        );
        write_provider(
            &temp_base.join("tenants/globex/providers/openai.yaml"),
            r#"{"tenant":"globex"}"#,
        );

        let config = managed_multi_tenant_config(&temp_base);
        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme-after"}"#,
        );
        write_tenant_metadata(
            &temp_base,
            "acme",
            r#"
id = "acme"

[[keys]]
source = "header"
name = "x-api-key"
value = "secret-acme"

[management_auth]
header = "x-tenant-admin-key"
value = "tenant-admin-globex"
"#,
        );

        let reload = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tenant/reload")
                    .header("x-tenant-admin-key", "tenant-admin-acme")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reload.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let inspect = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/tenant")
                    .header("x-tenant-admin-key", "tenant-admin-acme")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(inspect.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_str(&response_text(inspect).await).unwrap();
        assert_eq!(body["id"], "acme");

        let acme_response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-api-key", "secret-acme")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(acme_response.status(), StatusCode::OK);
        assert!(response_text(acme_response).await.contains("acme-before"));

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_tenant_reload_succeeds_when_other_tenant_secret_source_is_broken() {
        let temp_base = management_test_base("test_tenant_reload_ignores_other_tenant_secret");
        write_provider(
            &temp_base.join("tenants/default/providers/openai.yaml"),
            r#"{"tenant":"default"}"#,
        );
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme-before"}"#,
        );
        write_provider(
            &temp_base.join("tenants/globex/providers/openai.yaml"),
            r#"{"tenant":"globex"}"#,
        );

        let config = managed_multi_tenant_config(&temp_base);
        let globex_key_path = temp_base.join("secrets/globex.key");
        fs::write(&globex_key_path, "secret-globex").unwrap();
        write_tenant_metadata(
            &temp_base,
            "globex",
            &format!(
                r#"
id = "globex"

[[keys]]
source = "header"
name = "x-api-key"
value_file = "{}"
"#,
                toml_path(&globex_key_path)
            ),
        );

        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme-after"}"#,
        );
        fs::remove_file(&globex_key_path).unwrap();

        let reload = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tenant/reload")
                    .header("x-tenant-admin-key", "tenant-admin-acme")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reload.status(), StatusCode::OK);

        let acme_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-api-key", "secret-acme")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response_text(acme_response).await.contains("acme-after"));

        let globex_response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-api-key", "secret-globex")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(globex_response.status(), StatusCode::OK);
        assert!(response_text(globex_response).await.contains("globex"));

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_tenant_reload_succeeds_when_admin_auth_secret_source_is_broken() {
        let temp_base = management_test_base("test_tenant_reload_ignores_admin_auth_secret");
        write_provider(
            &temp_base.join("tenants/default/providers/openai.yaml"),
            r#"{"tenant":"default"}"#,
        );
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme-before"}"#,
        );
        write_provider(
            &temp_base.join("tenants/globex/providers/openai.yaml"),
            r#"{"tenant":"globex"}"#,
        );

        let mut config = managed_multi_tenant_config(&temp_base);
        let admin_key_path = temp_base.join("secrets/admin.key");
        fs::write(&admin_key_path, "global-admin-secret").unwrap();
        config.tenancy.admin_auth.value.clear();
        config.tenancy.admin_auth.value_file = Some(admin_key_path.clone());

        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme-after"}"#,
        );
        fs::remove_file(&admin_key_path).unwrap();

        let reload = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tenant/reload")
                    .header("x-tenant-admin-key", "tenant-admin-acme")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reload.status(), StatusCode::OK);

        let acme_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-api-key", "secret-acme")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response_text(acme_response).await.contains("acme-after"));

        let admin_response = app
            .oneshot(
                Request::builder()
                    .uri("/admin/tenants")
                    .header("x-admin-key", "global-admin-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(admin_response.status(), StatusCode::OK);

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_tenant_secret_rotation_is_picked_up_by_tenant_reload() {
        let temp_base = management_test_base("test_tenant_secret_rotation_reload");
        write_provider(
            &temp_base.join("tenants/default/providers/openai.yaml"),
            r#"{"tenant":"default"}"#,
        );
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme-before"}"#,
        );
        write_provider(
            &temp_base.join("tenants/globex/providers/openai.yaml"),
            r#"{"tenant":"globex"}"#,
        );

        let config = managed_multi_tenant_config(&temp_base);
        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        fs::write(temp_base.join("secrets/acme.key"), "secret-acme-rotated").unwrap();
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme-after"}"#,
        );

        let reload = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tenant/reload")
                    .header("x-tenant-admin-key", "tenant-admin-acme")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reload.status(), StatusCode::OK);

        let old_key_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-api-key", "secret-acme")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(old_key_response.status(), StatusCode::UNAUTHORIZED);

        let new_key_response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("x-api-key", "secret-acme-rotated")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response_text(new_key_response).await.contains("acme-after"));

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_tenant_admin_cannot_target_another_tenant() {
        let temp_base = management_test_base("test_tenant_cannot_target_other");
        write_provider(
            &temp_base.join("tenants/default/providers/openai.yaml"),
            r#"{"tenant":"default"}"#,
        );
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme"}"#,
        );
        write_provider(
            &temp_base.join("tenants/globex/providers/openai.yaml"),
            r#"{"tenant":"globex"}"#,
        );

        let config = managed_multi_tenant_config(&temp_base);
        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        let inspect = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/tenant")
                    .header("x-tenant", "globex")
                    .header("x-tenant-admin-key", "tenant-admin-acme")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(inspect.status(), StatusCode::UNAUTHORIZED);

        let reload = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tenant/reload")
                    .header("x-tenant", "globex")
                    .header("x-tenant-admin-key", "tenant-admin-acme")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reload.status(), StatusCode::UNAUTHORIZED);

        let admin_attempt = app
            .oneshot(
                Request::builder()
                    .uri("/admin/tenants/globex")
                    .header("x-tenant-admin-key", "tenant-admin-acme")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(admin_attempt.status(), StatusCode::UNAUTHORIZED);

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_management_responses_do_not_expose_secret_bearing_fields() {
        let temp_base = management_test_base("test_management_response_sanitization");
        write_provider(
            &temp_base.join("tenants/default/providers/openai.yaml"),
            r#"{"tenant":"default"}"#,
        );
        write_provider(
            &temp_base.join("tenants/acme/providers/openai.yaml"),
            r#"{"tenant":"acme"}"#,
        );
        write_provider(
            &temp_base.join("tenants/globex/providers/openai.yaml"),
            r#"{"tenant":"globex"}"#,
        );

        let config = managed_multi_tenant_config(&temp_base);
        let request_secret_path = temp_base.join("secrets/acme.key");
        let management_secret_path = temp_base.join("secrets/acme-admin.key");
        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/admin/tenants/acme")
                    .header("x-admin-key", "global-admin-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_text(response).await;
        assert!(!body.contains("global-admin-secret"));
        assert!(!body.contains("secret-acme"));
        assert!(!body.contains("tenant-admin-acme"));
        assert!(!body.contains("tenant-admin-globex"));
        assert!(!body.contains(&request_secret_path.display().to_string()));
        assert!(!body.contains(&management_secret_path.display().to_string()));
        assert!(!body.contains("value_file"));
        assert!(!body.contains("value_env"));
        assert!(!body.contains("\"value\""));

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_single_mode_tenant_management_requires_admin_auth() {
        let temp_base = management_test_base("test_single_mode_management");
        write_provider(
            &temp_base.join("config/providers/openai.yaml"),
            r#"{"tenant":"single"}"#,
        );

        let config = single_mode_management_config(&temp_base);
        let app = create_app(
            config.clone(),
            None,
            Arc::new(TenantStoreHandle::new(
                build_runtime_store(&config).unwrap(),
            )),
        )
        .await;

        let tenant_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/tenant")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(tenant_response.status(), StatusCode::UNAUTHORIZED);

        let tenant_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/tenant")
                    .header("x-admin-key", "global-admin-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(tenant_response.status(), StatusCode::OK);
        let tenant_body: serde_json::Value =
            serde_json::from_str(&response_text(tenant_response).await).unwrap();
        assert_eq!(tenant_body["id"], "default");

        let tenant_reload_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tenant/reload")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(tenant_reload_response.status(), StatusCode::UNAUTHORIZED);

        let tenant_reload_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/tenant/reload")
                    .header("x-admin-key", "global-admin-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(tenant_reload_response.status(), StatusCode::OK);

        let admin_response = app
            .oneshot(
                Request::builder()
                    .uri("/admin/tenants")
                    .header("x-admin-key", "global-admin-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(admin_response.status(), StatusCode::OK);
        let admin_body: serde_json::Value =
            serde_json::from_str(&response_text(admin_response).await).unwrap();
        assert_eq!(admin_body["mode"], "single");
        assert_eq!(admin_body["tenants"].as_array().unwrap().len(), 1);
        assert_eq!(admin_body["tenants"][0]["id"], "default");

        fs::remove_dir_all(temp_base).unwrap();
    }

    // ─────────────────────────────────────────────────────────────────────────
    // SDK Compat regression suite (BUGS-VIDAIMOCK.md: VM-001 .. VM-008)
    // Each test locks a specific wire-shape contract so regressions that would
    // reintroduce an SDK-level parse failure fail in CI.
    // ─────────────────────────────────────────────────────────────────────────

    /// VM-001: OpenAI streaming must emit a terminal chunk carrying
    /// finish_reason = "stop" before the usage/[DONE] frames.
    /// Real openai-python treats that chunk as the stream's terminal signal.
    #[tokio::test]
    async fn test_vm001_openai_stream_emits_finish_reason() {
        let app = create_app(get_test_config(), None, get_embedded_registry()).await;
        let body = r#"{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}],"max_tokens":5,"stream":true}"#;

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = drain_body(resp).await;
        let text = String::from_utf8(bytes).unwrap();

        assert!(
            text.contains(r#""finish_reason":"stop""#),
            "stream must include a chunk with finish_reason=stop before [DONE]; got:\n{}",
            text
        );
        assert!(
            text.ends_with("data: [DONE]\n\n"),
            "stream must end with 'data: [DONE]\\n\\n'; got tail:\n{}",
            &text[text.len().saturating_sub(100)..]
        );
    }

    /// VM-002: every SSE event must end with a blank line (\n\n), including
    /// the usage chunk that precedes [DONE] when stream_options.include_usage
    /// is set. openai-python's SSE parser fails with JSONDecodeError otherwise.
    #[tokio::test]
    async fn test_vm002_openai_stream_usage_chunk_has_blank_line_terminator() {
        let app = create_app(get_test_config(), None, get_embedded_registry()).await;
        let body = r#"{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}],"max_tokens":5,"stream":true,"stream_options":{"include_usage":true}}"#;

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let text = String::from_utf8(drain_body(resp).await).unwrap();

        // The usage chunk is the one with "usage":{"prompt_tokens":... .
        // It must be followed by "\n\ndata: [DONE]" — not just "\ndata:".
        assert!(
            text.contains("\"usage\":{"),
            "stream must include a usage chunk when include_usage=true"
        );
        let usage_idx = text.find("\"usage\":{").unwrap();
        let tail = &text[usage_idx..];
        // Find the end of the usage JSON object, then assert \n\n follows.
        let done_idx = tail
            .find("data: [DONE]")
            .expect("expected [DONE] frame after usage chunk");
        let between = &tail[..done_idx];
        assert!(
            between.ends_with("\n\n"),
            "usage chunk must be terminated with blank line before [DONE]; got bytes: {:?}",
            between
                .as_bytes()
                .iter()
                .rev()
                .take(6)
                .rev()
                .collect::<Vec<_>>()
        );
    }

    /// VM-005: ?chaos_status=500 query param produces an OpenAI-shaped JSON
    /// error envelope (provider error_template), not plain text or the
    /// success body. Composes with the URL so gateways can register one
    /// "broken" endpoint and another "healthy" endpoint against the same mock.
    #[tokio::test]
    async fn test_vm005_chaos_status_query_returns_provider_shape_error() {
        let app = create_app(get_test_config(), None, get_embedded_registry()).await;
        let body = r#"{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}]}"#;

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions?chaos_status=503")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let content_type = resp.headers().get("content-type").cloned();
        let text = String::from_utf8(drain_body(resp).await).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text)
            .expect("chaos response must be valid JSON (not plain text)");

        assert_eq!(content_type.unwrap().to_str().unwrap(), "application/json");
        assert!(
            json.get("error").is_some(),
            "OpenAI error envelope must have top-level 'error' key; got: {}",
            text
        );
    }

    /// VM-005 (streaming): ?chaos_status=500 on a streaming request must
    /// return a non-streaming HTTP error (real providers don't send SSE
    /// errors). Assert status + JSON body.
    #[tokio::test]
    async fn test_vm005_chaos_status_on_streaming_returns_non_streaming_error() {
        let app = create_app(get_test_config(), None, get_embedded_registry()).await;
        let body =
            r#"{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}],"stream":true}"#;

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions?chaos_status=500")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        // Body must be JSON, not SSE (no "data:" prefixes).
        let text = String::from_utf8(drain_body(resp).await).unwrap();
        assert!(
            !text.starts_with("data:"),
            "chaos on streaming must return non-streaming JSON body; got:\n{}",
            text
        );
        let _json: serde_json::Value =
            serde_json::from_str(&text).expect("streaming chaos response body must be valid JSON");
    }

    /// VM-005: X-Vidai-Chaos-Drop=100 must return a JSON error envelope,
    /// not plain text. Previously returned "Simulated Internal Server Error"
    /// as text/plain which no SDK can parse.
    #[tokio::test]
    async fn test_vm005_chaos_drop_header_returns_json_error_envelope() {
        let app = create_app(get_test_config(), None, get_embedded_registry()).await;
        let body = r#"{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}]}"#;

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .header("x-vidai-chaos-drop", "100")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let content_type = resp
            .headers()
            .get("content-type")
            .cloned()
            .expect("missing content-type header");
        assert_eq!(
            content_type.to_str().unwrap(),
            "application/json",
            "chaos response must carry application/json content-type"
        );

        let text = String::from_utf8(drain_body(resp).await).unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&text).expect("chaos response must be valid JSON");
        assert!(
            json.get("error").is_some(),
            "OpenAI error envelope must have top-level 'error' key"
        );
    }

    /// VM-006: Anthropic streaming events must each be separated by a blank
    /// line (`\n\n`) so SDK SSE parsers treat them as distinct events.
    /// The symptom was "events out of order" but the root cause was frames
    /// merging because blank lines were stripped by the raw-frame renderer.
    #[tokio::test]
    async fn test_vm006_anthropic_stream_events_have_blank_line_separators() {
        let app = create_app(get_test_config(), None, get_embedded_registry()).await;
        let body = r#"{"model":"claude","max_tokens":30,"stream":true,"messages":[{"role":"user","content":"hi"}]}"#;

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/messages")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let text = String::from_utf8(drain_body(resp).await).unwrap();

        // First event must be message_start.
        let first_event_line = text
            .lines()
            .find(|l| l.starts_with("event: "))
            .expect("no event: lines found in Anthropic stream");
        assert_eq!(
            first_event_line, "event: message_start",
            "Anthropic streaming must start with message_start; got: {}",
            first_event_line
        );

        // Every inter-event transition must be separated by `\n\n`.
        // We check this by scanning for every occurrence of "\nevent:" and
        // asserting the byte before the leading \n is another \n (i.e. a
        // blank line precedes each event after the first).
        // Skip the very first `event:` at the start of the stream.
        let bytes = text.as_bytes();
        let mut pos = 0usize;
        let mut events_seen = 0usize;
        while let Some(idx) = text[pos..].find("\nevent: ") {
            let absolute = pos + idx;
            // absolute is the \n before "event:". For the 2nd event onward,
            // bytes[absolute - 1] must also be \n (i.e. a blank line).
            if events_seen >= 1 {
                assert!(
                    absolute >= 1 && bytes[absolute - 1] == b'\n',
                    "event at byte {} not preceded by blank line; context: {:?}",
                    absolute,
                    &text[absolute.saturating_sub(20)..(absolute + 20).min(text.len())]
                );
            }
            events_seen += 1;
            pos = absolute + 1;
        }
        assert!(
            events_seen >= 7,
            "expected at least 7 Anthropic event types; found {}",
            events_seen
        );
    }

    /// VM-006 (order): even with framing fixed, the event order itself
    /// must be strict: message_start first, then content_block_start before
    /// any content_block_delta.
    #[tokio::test]
    async fn test_vm006_anthropic_stream_event_order() {
        let app = create_app(get_test_config(), None, get_embedded_registry()).await;
        let body = r#"{"model":"claude","max_tokens":30,"stream":true,"messages":[{"role":"user","content":"hi"}]}"#;

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/messages")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        let text = String::from_utf8(drain_body(resp).await).unwrap();
        let events: Vec<&str> = text.lines().filter(|l| l.starts_with("event: ")).collect();

        // Find indices of the three ordering-critical events.
        let pos_start = events
            .iter()
            .position(|e| *e == "event: message_start")
            .expect("missing message_start");
        let pos_block_start = events
            .iter()
            .position(|e| *e == "event: content_block_start")
            .expect("missing content_block_start");
        let pos_first_delta = events
            .iter()
            .position(|e| *e == "event: content_block_delta")
            .expect("missing content_block_delta");
        let pos_stop = events
            .iter()
            .position(|e| *e == "event: message_stop")
            .expect("missing message_stop");

        assert!(
            pos_start < pos_block_start,
            "message_start must precede content_block_start"
        );
        assert!(
            pos_block_start < pos_first_delta,
            "content_block_start must precede content_block_delta"
        );
        assert!(
            pos_first_delta < pos_stop,
            "content_block_delta must precede message_stop"
        );
    }

    /// VM-007: Anthropic `/v1/messages` without `max_tokens` must return
    /// HTTP 400 with an Anthropic-shaped `{"type":"error","error":{...}}`
    /// envelope, matching real Anthropic validation.
    #[tokio::test]
    async fn test_vm007_anthropic_missing_max_tokens_returns_400() {
        let app = create_app(get_test_config(), None, get_embedded_registry()).await;
        let body = r#"{"model":"claude","messages":[{"role":"user","content":"hi"}]}"#;

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/messages")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "missing max_tokens must return HTTP 400"
        );

        let text = String::from_utf8(drain_body(resp).await).unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&text).expect("error body must be valid JSON");
        assert_eq!(
            json.get("type").and_then(|v| v.as_str()),
            Some("error"),
            "Anthropic error envelope must have type=error; got: {}",
            text
        );
        assert_eq!(
            json["error"]["type"].as_str(),
            Some("invalid_request_error"),
            "Anthropic validation errors must carry type=invalid_request_error"
        );
        // Message must name the missing field (real Anthropic: "max_tokens: field required").
        let message = json["error"]["message"].as_str().unwrap_or("");
        assert!(
            message.contains("max_tokens"),
            "error message must name the missing field; got: {}",
            message
        );
    }

    /// VM-007: Request WITH max_tokens continues to succeed (regression guard —
    /// the validation must be targeted, not blanket).
    #[tokio::test]
    async fn test_vm007_anthropic_with_max_tokens_returns_200() {
        let app = create_app(get_test_config(), None, get_embedded_registry()).await;
        let body =
            r#"{"model":"claude","max_tokens":30,"messages":[{"role":"user","content":"hi"}]}"#;

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/messages")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// VM-008: Gemini streamGenerateContent with alt=sse must NOT emit a
    /// `data: [DONE]` sentinel. Real Gemini terminates on connection close;
    /// a [DONE] frame causes google-genai SDK to raise UnknownApiResponseError.
    #[tokio::test]
    async fn test_vm008_gemini_stream_no_done_sentinel() {
        let app = create_app(get_test_config(), None, get_embedded_registry()).await;
        let body = r#"{"contents":[{"role":"user","parts":[{"text":"hi"}]}]}"#;

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let text = String::from_utf8(drain_body(resp).await).unwrap();

        assert!(
            !text.contains("[DONE]"),
            "Gemini stream must not emit [DONE] sentinel; got:\n{}",
            text
        );
        // Must still contain at least one real data: frame.
        assert!(
            text.contains("data: {"),
            "Gemini stream should contain at least one JSON data frame"
        );
    }

    /// VM-008 follow-up: Gemini streaming chunk shape contract.
    /// Real Gemini puts finishReason + usageMetadata ONLY on the final chunk;
    /// intermediate chunks have finishReason=null and no usageMetadata.
    /// Regression guard against over-emission (which would make google-genai
    /// SDK treat every chunk as a terminal signal and discard the rest).
    #[tokio::test]
    async fn test_vm008_gemini_stream_finishreason_only_on_final_chunk() {
        let app = create_app(get_test_config(), None, get_embedded_registry()).await;
        let body = r#"{"contents":[{"role":"user","parts":[{"text":"hi"}]}]}"#;

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        let text = String::from_utf8(drain_body(resp).await).unwrap();

        // Collect JSON payloads from every `data:` frame.
        let frames: Vec<serde_json::Value> = text
            .lines()
            .filter_map(|l| l.strip_prefix("data: "))
            .filter(|j| j.starts_with('{'))
            .filter_map(|j| serde_json::from_str(j).ok())
            .collect();

        assert!(
            frames.len() >= 2,
            "expected at least 2 frames (>=1 intermediate + 1 final); got {}",
            frames.len()
        );

        // Final chunk must carry finishReason=STOP and usageMetadata.
        let final_frame = frames.last().unwrap();
        let final_finish = final_frame["candidates"][0]["finishReason"].as_str();
        assert_eq!(
            final_finish,
            Some("STOP"),
            "final chunk must carry finishReason=STOP; got frame: {}",
            final_frame
        );
        assert!(
            final_frame.get("usageMetadata").is_some(),
            "final chunk must carry usageMetadata; got: {}",
            final_frame
        );

        // Every intermediate chunk must carry finishReason=null and must NOT
        // carry usageMetadata. This is the specific shape google-genai SDK
        // requires to continue iterating through the stream.
        for (i, frame) in frames.iter().take(frames.len() - 1).enumerate() {
            let finish = &frame["candidates"][0]["finishReason"];
            assert!(
                finish.is_null(),
                "intermediate chunk {} must have finishReason=null; got {}; frame: {}",
                i,
                finish,
                frame
            );
            assert!(
                frame.get("usageMetadata").is_none(),
                "intermediate chunk {} must NOT carry usageMetadata; got frame: {}",
                i,
                frame
            );
        }
    }

    /// Regression: OpenAI non-streaming chat continues to return 200 with
    /// a valid chat.completion shape even after the error_template + chaos
    /// rewiring.
    #[tokio::test]
    async fn test_openai_non_streaming_still_works() {
        let app = create_app(get_test_config(), None, get_embedded_registry()).await;
        let body = r#"{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}]}"#;
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let text = String::from_utf8(drain_body(resp).await).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json["object"], "chat.completion");
        assert!(json["choices"][0]["message"].is_object());
    }

    /// Regression: X-Mock-Status header retains its original behaviour —
    /// forces an HTTP status while keeping the success body shape (the
    /// contract documented for BFF-level passthrough tests).
    #[tokio::test]
    async fn test_xmockstatus_header_still_works() {
        let app = create_app(get_test_config(), None, get_embedded_registry()).await;
        let body = r#"{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}]}"#;
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .header("x-mock-status", "429")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        // Body is rendered from error_template (since status is an error).
        let text = String::from_utf8(drain_body(resp).await).unwrap();
        let _: serde_json::Value = serde_json::from_str(&text).expect("body must be valid JSON");
    }

    // ─── VM-009: Agentic tool-loop termination (end-to-end) ──────────────
    //
    // These integration tests complement the unit tests in provider.rs by
    // exercising the full HTTP->template->response path. They guarantee that
    // the heuristic is wired into each provider's bundled chat template and
    // that the emitted response actually changes shape when a tool result
    // sits in the history.

    /// VM-009 / OpenAI: with `tools` but a preceding `role:tool` message, the
    /// response must be plain text with finish_reason=stop — NOT tool_calls.
    #[tokio::test]
    async fn test_vm009_openai_tool_loop_terminates_on_tool_result() {
        let app = create_app(get_test_config(), None, get_embedded_registry()).await;
        let body = r#"{
            "model": "gpt-4o-mini",
            "tools": [{"type": "function", "function": {"name": "get_weather", "parameters": {}}}],
            "messages": [
                {"role": "user", "content": "What is the weather in London?"},
                {"role": "assistant", "tool_calls": [
                    {"id": "call_1", "type": "function",
                     "function": {"name": "get_weather", "arguments": "{\"city\":\"London\"}"}}
                ]},
                {"role": "tool", "tool_call_id": "call_1", "content": "15°C cloudy"}
            ]
        }"#;
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let text = String::from_utf8(drain_body(resp).await).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        let choice = &json["choices"][0];
        let msg = &choice["message"];

        assert_eq!(
            choice["finish_reason"], "stop",
            "finish_reason must be 'stop' after tool result, not 'tool_calls'; got:\n{}",
            text
        );
        assert!(
            msg.get("tool_calls").is_none() || msg["tool_calls"].is_null(),
            "response must NOT include tool_calls after a tool result; got:\n{}",
            text
        );
        assert!(
            msg["content"].as_str().is_some(),
            "response must include text content after a tool result; got:\n{}",
            text
        );
    }

    /// VM-009 / OpenAI (regression guard): with `tools` and NO tool result in
    /// the history, the response must still be tool_calls. We must only
    /// terminate when appropriate.
    #[tokio::test]
    async fn test_vm009_openai_tools_without_result_still_returns_tool_calls() {
        let app = create_app(get_test_config(), None, get_embedded_registry()).await;
        let body = r#"{
            "model": "gpt-4o-mini",
            "tools": [{"type": "function", "function": {"name": "get_weather", "parameters": {}}}],
            "messages": [{"role": "user", "content": "Weather?"}]
        }"#;
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let text = String::from_utf8(drain_body(resp).await).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json["choices"][0]["finish_reason"], "tool_calls");
        assert!(json["choices"][0]["message"]["tool_calls"].is_array());
    }

    /// VM-009 / Anthropic: tool_result content block in user message
    /// terminates the loop — stop_reason must be end_turn and content
    /// must be text, not tool_use.
    #[tokio::test]
    async fn test_vm009_anthropic_tool_loop_terminates_on_tool_result() {
        let app = create_app(get_test_config(), None, get_embedded_registry()).await;
        let body = r#"{
            "model": "claude",
            "max_tokens": 200,
            "tools": [{"name": "get_weather", "description": "x", "input_schema": {"type":"object"}}],
            "messages": [
                {"role": "user", "content": "Weather in London?"},
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "t1", "name": "get_weather", "input": {"city": "London"}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "t1", "content": "15°C cloudy"}
                ]}
            ]
        }"#;
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/messages")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let text = String::from_utf8(drain_body(resp).await).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();

        assert_eq!(
            json["stop_reason"], "end_turn",
            "stop_reason must be 'end_turn' after tool_result, not 'tool_use'; got:\n{}",
            text
        );
        let first_block_type = json["content"][0]["type"].as_str();
        assert_eq!(
            first_block_type,
            Some("text"),
            "first content block must be 'text' after tool_result, not 'tool_use'; got:\n{}",
            text
        );
    }

    /// VM-009 / Anthropic (regression guard): with `tools` but only the
    /// initial user message, the response must still be a tool_use block.
    #[tokio::test]
    async fn test_vm009_anthropic_tools_without_result_still_returns_tool_use() {
        let app = create_app(get_test_config(), None, get_embedded_registry()).await;
        let body = r#"{
            "model": "claude",
            "max_tokens": 200,
            "tools": [{"name": "get_weather", "description": "x", "input_schema": {"type":"object"}}],
            "messages": [{"role": "user", "content": "Weather?"}]
        }"#;
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/messages")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let text = String::from_utf8(drain_body(resp).await).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json["stop_reason"], "tool_use");
        assert_eq!(json["content"][0]["type"], "tool_use");
    }

    /// VM-009 / Gemini: functionResponse part in user contents terminates
    /// the loop — first part must be text, not functionCall. finishMessage
    /// must NOT say "Model generated function call(s)."
    #[tokio::test]
    async fn test_vm009_gemini_tool_loop_terminates_on_function_response() {
        let app = create_app(get_test_config(), None, get_embedded_registry()).await;
        let body = r#"{
            "contents": [
                {"role": "user", "parts": [{"text": "Weather in London?"}]},
                {"role": "model", "parts": [{"functionCall": {"name": "get_weather", "args": {"city":"London"}}}]},
                {"role": "user", "parts": [{"functionResponse": {"name": "get_weather", "response": {"temp": 15}}}]}
            ],
            "tools": [{"functionDeclarations": [{"name": "get_weather", "parameters": {}}]}]
        }"#;
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1beta/models/gemini-2.5-flash:generateContent")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let text = String::from_utf8(drain_body(resp).await).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        let first_part = &json["candidates"][0]["content"]["parts"][0];

        assert!(
            first_part.get("text").is_some(),
            "first part must carry text after functionResponse; got:\n{}",
            text
        );
        assert!(
            first_part.get("functionCall").is_none(),
            "first part must NOT be functionCall after functionResponse; got:\n{}",
            text
        );
        // finishMessage is only emitted on the function-call branch; absence here
        // confirms the loop-terminating branch rendered.
        assert!(
            json["candidates"][0].get("finishMessage").is_none(),
            "finishMessage must be absent on the text-response branch; got:\n{}",
            text
        );
    }

    /// VM-009 / Gemini (regression guard): with tools and no functionResponse,
    /// still returns functionCall.
    #[tokio::test]
    async fn test_vm009_gemini_tools_without_response_still_returns_function_call() {
        let app = create_app(get_test_config(), None, get_embedded_registry()).await;
        let body = r#"{
            "contents": [{"role": "user", "parts": [{"text": "Weather?"}]}],
            "tools": [{"functionDeclarations": [{"name": "get_weather", "parameters": {}}]}]
        }"#;
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1beta/models/gemini-2.5-flash:generateContent")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let text = String::from_utf8(drain_body(resp).await).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(json["candidates"][0]["content"]["parts"][0]
            .get("functionCall")
            .is_some());
    }

    // ─── VM-010: Tool-call responses must echo the caller's tool name ──────
    //
    // Previously, the OpenAI chat template hardcoded "get_weather"/"Paris"
    // regardless of what tool the caller declared. Anthropic and Gemini
    // already echoed correctly; we lock that in as regression guards too.

    /// VM-010 / OpenAI non-streaming: declared tool name must appear in response.
    #[tokio::test]
    async fn test_vm010_openai_echoes_caller_tool_name() {
        let app = create_app(get_test_config(), None, get_embedded_registry()).await;
        let body = r#"{
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "hi"}],
            "tools": [{"type": "function", "function": {"name": "canary_tool_a", "parameters": {}}}]
        }"#;
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&String::from_utf8(drain_body(resp).await).unwrap()).unwrap();
        let tc = &json["choices"][0]["message"]["tool_calls"][0];
        assert_eq!(
            tc["function"]["name"], "canary_tool_a",
            "OpenAI response must echo caller's tool name; got: {}",
            tc
        );
        assert_eq!(
            tc["function"]["arguments"], "{}",
            "OpenAI args should default to empty object; got: {}",
            tc
        );
    }

    /// VM-010 / OpenAI streaming: same — the streaming tool_calls frame
    /// must echo the caller's name, not hardcoded.
    #[tokio::test]
    async fn test_vm010_openai_streaming_echoes_caller_tool_name() {
        let app = create_app(get_test_config(), None, get_embedded_registry()).await;
        let body = r#"{
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "hi"}],
            "stream": true,
            "tools": [{"type": "function", "function": {"name": "canary_tool_s", "parameters": {}}}]
        }"#;
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let text = String::from_utf8(drain_body(resp).await).unwrap();
        // Find the tool_calls frame and assert it carries the caller's name.
        assert!(
            text.contains("\"name\":\"canary_tool_s\""),
            "OpenAI streaming tool_calls must echo caller's name; got:\n{}",
            text
        );
        assert!(
            !text.contains("\"name\":\"get_weather\""),
            "OpenAI streaming must not emit the hardcoded demo name; got:\n{}",
            text
        );
    }

    /// VM-010 / Anthropic non-streaming (regression guard).
    #[tokio::test]
    async fn test_vm010_anthropic_echoes_caller_tool_name() {
        let app = create_app(get_test_config(), None, get_embedded_registry()).await;
        let body = r#"{
            "model": "claude", "max_tokens": 200,
            "messages": [{"role": "user", "content": "hi"}],
            "tools": [{"name": "canary_tool_b", "description": "x", "input_schema": {"type": "object"}}]
        }"#;
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/messages")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&String::from_utf8(drain_body(resp).await).unwrap()).unwrap();
        assert_eq!(json["content"][0]["type"], "tool_use");
        assert_eq!(json["content"][0]["name"], "canary_tool_b");
    }

    /// VM-010 / Gemini non-streaming (regression guard).
    #[tokio::test]
    async fn test_vm010_gemini_echoes_caller_tool_name() {
        let app = create_app(get_test_config(), None, get_embedded_registry()).await;
        let body = r#"{
            "contents": [{"role": "user", "parts": [{"text": "hi"}]}],
            "tools": [{"functionDeclarations": [{"name": "canary_tool_c", "parameters": {"type":"OBJECT"}}]}]
        }"#;
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1beta/models/gemini-2.5-flash:generateContent")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&String::from_utf8(drain_body(resp).await).unwrap()).unwrap();
        let fc = &json["candidates"][0]["content"]["parts"][0]["functionCall"];
        assert_eq!(fc["name"], "canary_tool_c");
    }

    // ─── VM-011: Streaming-with-tools wire-format linter ───────────────────
    //
    // Every `data:` frame in a streaming SSE response must be a single line
    // of valid JSON. Before the VM-011 fix, Gemini + Anthropic emitted
    // pretty-printed multi-line JSON that bled outside the SSE framing
    // (Gemini's SDK errored; Anthropic's silently mis-parsed). Locks in the
    // single-line contract across all three providers.
    //
    // Reusable: easy to extend when new providers add streaming-with-tools.

    /// Assert every `data: {...}` line in an SSE body is a single-line
    /// JSON object that parses cleanly and contains no embedded newlines.
    fn assert_data_frames_are_single_line_json(body: &str, label: &str) {
        let mut frame_count = 0;
        for line in body.lines() {
            let Some(payload) = line.strip_prefix("data: ") else {
                continue;
            };
            if payload.is_empty() || payload == "[DONE]" {
                // Empty data: is a malformed frame (the original Gemini bug).
                panic!("{label}: empty 'data:' line found — indicates broken multi-line frame");
            }
            // Must parse as JSON.
            serde_json::from_str::<serde_json::Value>(payload)
                .unwrap_or_else(|e| panic!(
                    "{label}: data frame must be valid JSON; parse error: {e}\nframe: {payload}\nfull body:\n{body}"
                ));
            // Must contain no embedded newlines (single-line per SSE spec).
            assert!(
                !payload.contains('\n'),
                "{label}: data frame must be single-line; got embedded \\n in:\n{payload}"
            );
            frame_count += 1;
        }
        assert!(
            frame_count >= 1,
            "{label}: expected at least one data: frame; got 0. body:\n{body}"
        );
    }

    /// VM-011 / Gemini: streaming with tools must not emit multi-line JSON.
    #[tokio::test]
    async fn test_vm011_gemini_streaming_with_tools_single_line_frames() {
        let app = create_app(get_test_config(), None, get_embedded_registry()).await;
        let body = r#"{
            "contents": [{"role": "user", "parts": [{"text": "hi"}]}],
            "tools": [{"functionDeclarations": [{"name": "canary_t", "parameters": {"type":"OBJECT"}}]}]
        }"#;
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let text = String::from_utf8(drain_body(resp).await).unwrap();
        assert_data_frames_are_single_line_json(&text, "gemini stream+tools");

        // Also assert the functionCall echoes the caller's name (combined
        // VM-010 + VM-011 check for the streaming path).
        assert!(
            text.contains("\"name\":\"canary_t\""),
            "Gemini streaming tool_call must echo caller's name; got:\n{}",
            text
        );
    }

    /// VM-011 / Anthropic: streaming with tools must emit typed tool_use
    /// events, not word-chunked JSON text deltas. Before the fix, the stream
    /// parsed as "content_block_delta with text_delta" containing fragments
    /// of the tool_use JSON body.
    #[tokio::test]
    async fn test_vm011_anthropic_streaming_with_tools_emits_tool_use_block() {
        let app = create_app(get_test_config(), None, get_embedded_registry()).await;
        let body = r#"{
            "model": "claude", "max_tokens": 50, "stream": true,
            "messages": [{"role": "user", "content": "hi"}],
            "tools": [{"name": "canary_t", "description": "x", "input_schema": {"type": "object"}}]
        }"#;
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/messages")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let text = String::from_utf8(drain_body(resp).await).unwrap();

        // content_block_start's content_block must be tool_use, not text.
        assert!(
            text.contains("\"type\":\"tool_use\""),
            "Anthropic streaming with tools must emit tool_use content block; got:\n{}",
            text
        );
        assert!(
            text.contains("\"name\":\"canary_t\""),
            "tool_use block must echo caller's name; got:\n{}",
            text
        );
        // Deltas must be input_json_delta, not text_delta fragments of JSON body.
        assert!(
            text.contains("\"type\":\"input_json_delta\""),
            "Anthropic streaming tool deltas must be input_json_delta; got:\n{}",
            text
        );
        // stop_reason in message_delta must be tool_use.
        assert!(
            text.contains("\"stop_reason\":\"tool_use\""),
            "message_delta must carry stop_reason=tool_use; got:\n{}",
            text
        );
    }

    /// VM-011 / OpenAI (regression guard): streaming-with-tools single-line
    /// framing was already correct before the VM-011 fix; assert it stays that
    /// way after the shared content extractor changes.
    #[tokio::test]
    async fn test_vm011_openai_streaming_with_tools_single_line_frames() {
        let app = create_app(get_test_config(), None, get_embedded_registry()).await;
        let body = r#"{
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "hi"}],
            "stream": true,
            "tools": [{"type": "function", "function": {"name": "canary_t", "parameters": {}}}]
        }"#;
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let text = String::from_utf8(drain_body(resp).await).unwrap();
        // OpenAI's stream uses "data: ..." + "data: [DONE]". Skip the [DONE]
        // sentinel which is a valid OpenAI convention but not valid JSON.
        let body_without_done: String = text
            .lines()
            .filter(|l| *l != "data: [DONE]")
            .collect::<Vec<_>>()
            .join("\n");
        assert_data_frames_are_single_line_json(&body_without_done, "openai stream+tools");
    }
}
