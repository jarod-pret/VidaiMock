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
    // Legacy support logic removed as we fully transition to providers for content types too

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
        build_runtime_store, AdminAuthConfig, TenancyConfig, TenancyMode, TenantKeySource,
        TenantStore, TenantStoreHandle,
    };
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
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
                priority: 0,
            })
            .unwrap();

        Arc::new(TenantStore::new(
            TenancyMode::Single,
            "x-tenant".to_string(),
            Arc::new(crate::tenancy::TenantRuntime {
                label: crate::tenancy::DEFAULT_TENANT_ID.to_string(),
                registry: Arc::new(registry),
                requires_key: false,
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
        // Ensure config knows where presets are
        // This line seems to be part of a larger change or a typo in the instruction.
        // If `presets_dir` is meant to be defined, it's missing from the context.
        // Assuming the intent was to set config_dir if a presets_dir was available,
        // but without `presets_dir` definition, this line would cause a compile error.
        // For now, I will insert the line as provided, but it will likely require further context.
        // config.config_dir = presets_dir.clone(); // This line is commented out as `presets_dir` is undefined.
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

                priority: 0,
            })
            .unwrap();

        let app = create_app(
            config,
            None,
            Arc::new(TenantStoreHandle::new(Arc::new(TenantStore::new(
                TenancyMode::Single,
                "x-tenant".to_string(),
                Arc::new(crate::tenancy::TenantRuntime {
                    label: crate::tenancy::DEFAULT_TENANT_ID.to_string(),
                    registry: Arc::new(registry),
                    requires_key: false,
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

        assert_eq!(body["port"], 0);
        assert_eq!(body["latency"]["mode"], "benchmark");
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

    fn multi_tenant_config(base_dir: &std::path::Path) -> AppConfig {
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
                tenants: vec![
                    crate::tenancy::TenantConfig {
                        id: "acme".to_string(),
                        keys: Vec::new(),
                    },
                    crate::tenancy::TenantConfig {
                        id: "globex".to_string(),
                        keys: Vec::new(),
                    },
                ],
            },
            latency: crate::config::LatencyConfig::default(),
            chaos: crate::config::ChaosConfig::default(),
            endpoints: vec![crate::config::EndpointConfig {
                path: "/v1/chat/completions".to_string(),
                format: "openai".to_string(),
                content_type: None,
            }],
            response_file: None,
        }
    }

    fn managed_multi_tenant_config(base_dir: &std::path::Path) -> AppConfig {
        let secret_dir = base_dir.join("secrets");
        fs::create_dir_all(&secret_dir).unwrap();
        let acme_key_path = secret_dir.join("acme.key");
        fs::write(&acme_key_path, "secret-acme").unwrap();

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
                tenants: vec![
                    crate::tenancy::TenantConfig {
                        id: "acme".to_string(),
                        keys: vec![crate::tenancy::TenantKeyConfig {
                            source: TenantKeySource::Header,
                            name: "x-api-key".to_string(),
                            value: String::new(),
                            value_file: Some(acme_key_path),
                            value_env: None,
                        }],
                    },
                    crate::tenancy::TenantConfig {
                        id: "globex".to_string(),
                        keys: vec![crate::tenancy::TenantKeyConfig {
                            source: TenantKeySource::Header,
                            name: "x-api-key".to_string(),
                            value: "secret-globex".to_string(),
                            value_file: None,
                            value_env: None,
                        }],
                    },
                ],
            },
            latency: crate::config::LatencyConfig::default(),
            chaos: crate::config::ChaosConfig::default(),
            endpoints: vec![crate::config::EndpointConfig {
                path: "/v1/chat/completions".to_string(),
                format: "openai".to_string(),
                content_type: None,
            }],
            response_file: None,
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
                tenants: Vec::new(),
            },
            latency: crate::config::LatencyConfig::default(),
            chaos: crate::config::ChaosConfig::default(),
            endpoints: vec![crate::config::EndpointConfig {
                path: "/v1/chat/completions".to_string(),
                format: "openai".to_string(),
                content_type: None,
            }],
            response_file: None,
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
                    .header("x-api-key", "secret-acme")
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
                    .header("x-api-key", "secret-acme")
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
                    .header("x-api-key", "secret-acme")
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
                    .header("x-api-key", "secret-acme")
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
                    .header("x-api-key", "secret-acme")
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
                    .header("x-api-key", "secret-acme")
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
                    .header("x-api-key", "secret-acme")
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
        let secret_path = temp_base.join("secrets/acme.key");
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
        assert!(!body.contains(&secret_path.display().to_string()));
        assert!(!body.contains("value_file"));
        assert!(!body.contains("value_env"));
        assert!(!body.contains("\"value\""));

        fs::remove_dir_all(temp_base).unwrap();
    }

    #[tokio::test]
    async fn test_single_mode_management_endpoints_still_work() {
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
        assert_eq!(tenant_response.status(), StatusCode::OK);
        let tenant_body: serde_json::Value =
            serde_json::from_str(&response_text(tenant_response).await).unwrap();
        assert_eq!(tenant_body["id"], "default");

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
}
