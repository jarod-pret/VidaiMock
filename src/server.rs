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
use crate::handlers::{AppState, health_check, mock_handler, echo_handler, status_handler, models_handler, streaming_handler};
// use crate::formats::load_response; // Removed

use metrics_exporter_prometheus::PrometheusHandle;
use tower_http::trace::TraceLayer;
use axum::{
    extract::Request,
    middleware::{self, Next},
    response::IntoResponse,
    routing::{get, post, any},
    Router,
    Extension,
};

pub async fn start_server(config: AppConfig, metrics_handle: PrometheusHandle, registry: Arc<crate::provider::ProviderRegistry>) -> Result<(), Box<dyn std::error::Error>> {
    let addr = format!("{}:{}", config.host, config.port);
    let port = config.port;
    
    // Bind the listener first to catch port-in-use errors early
    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("ERROR: Failed to bind to address {}: {}", addr, e);
            eprintln!("       This usually means the port {} is already in use by another process.", port);
            eprintln!("       Try using a different port with --port <PORT>.");
            std::process::exit(1);
        }
    };

    let app = create_app(config, Some(metrics_handle), registry).await;

    println!("🚀 VidaiMock is running at http://{}", addr);
    tracing::info!("Listening on {}", addr);
    
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

pub async fn create_app(config: AppConfig, metrics_handle: Option<PrometheusHandle>, registry: Arc<crate::provider::ProviderRegistry>) -> Router {
    // Legacy support logic removed as we fully transition to providers for content types too
    
    let state = Arc::new(AppState {
        config: Arc::new(config.clone()),
        registry,
    });

    let mut app = Router::new()
        .route("/health", get(health_check))
        .route("/status", get(status_handler));

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
    register_default!("/v1/engines/{engine}/embeddings", post, mock_handler);
    register_default!("/v1beta/models/{model_action}", post, mock_handler);
    // Gemini AI Studio /v1/models paths - POST for generateContent
    register_default!("/v1/models/{model_action}", post, mock_handler);
    
    // Azure OpenAI paths
    register_default!("/openai/deployments/{deployment}/chat/completions", post, mock_handler);
    register_default!("/openai/deployments/{deployment}/embeddings", post, mock_handler);

    // Anthropic models
    if !registered_paths.contains("/v1/models/{model_action}") {
         app = app.route("/v1/models/{model_action}", get(models_handler));
    }
    register_default!("/v1/messages/stream", post, mock_handler);

    // Bedrock paths
    register_default!("/model/{model_id}/invoke", post, mock_handler);
    register_default!("/model/{model_id}/converse", post, mock_handler);
    register_default!("/model/{model_id}/invoke-with-response-stream", post, mock_handler);
    register_default!("/model/{model_id}/converse-stream", post, mock_handler);

    // Vertex AI paths
    register_default!("/v1/projects/{project}/locations/{location}/publishers/google/models/{model_action}", post, mock_handler);

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

    metrics::counter!("http_requests_total", "method" => method.to_string(), "path" => path.clone(), "status" => status).increment(1);
    metrics::histogram!("http_request_duration_seconds", "method" => method.to_string(), "path" => path).record(latency);

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
    use axum::http::{Request, StatusCode};
    use axum::body::Body;
    use tower::ServiceExt; // for oneshot
    use std::path::PathBuf;
    use std::collections::HashMap;

    fn get_test_config() -> AppConfig {
        AppConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            workers: 1,
            log_level: "debug".to_string(),
            config_dir: PathBuf::from("config"), 
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

    fn get_test_registry() -> Arc<crate::provider::ProviderRegistry> {
        let mut registry = crate::provider::ProviderRegistry::new();
        // Add a default OpenAI provider for tests
        let config = crate::provider::ProviderConfig {
            name: "openai".to_string(),
            matcher: "^/v1/chat/completions$".to_string(),
            request_mapping: HashMap::new(),
            response_template: None,
            response_body: Some(r#"{"id": "test-id", "object": "chat.completion", "model": "test-model"}"#.to_string()),
            stream: None,
        };
        registry.add_provider(config).unwrap();
        Arc::new(registry)
    }

    #[tokio::test]
    async fn test_health_check() {
        let config = get_test_config();
        let registry = Arc::new(crate::provider::ProviderRegistry::new());
        let app = create_app(config, None, registry).await;

        let response = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_mock_endpoint() {
        let config = get_test_config();
        let app = create_app(config, None, get_test_registry()).await;

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
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
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
        
        let app = create_app(config, None, get_test_registry()).await;
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
        
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&bytes[..], body_content.as_bytes());
    }

    #[tokio::test]
    async fn test_multiprovider_endpoints() {
        let config = get_test_config();
        
        // Setup registry with multiple providers
        let mut registry = crate::provider::ProviderRegistry::new();
        
        // OpenAI
        registry.add_provider(crate::provider::ProviderConfig {
            name: "openai".to_string(),
            matcher: "^/v1/chat/completions$".to_string(),
            request_mapping: HashMap::new(),
            response_template: None,
            response_body: Some(r#"{"object": "chat.completion"}"#.to_string()),
            stream: None,
        }).unwrap();

        // Anthropic
        registry.add_provider(crate::provider::ProviderConfig {
            name: "anthropic".to_string(),
            matcher: "^/v1/messages$".to_string(),
            request_mapping: HashMap::new(),
            response_template: None,
            response_body: Some(r#"{"type": "message"}"#.to_string()),
            stream: None,
        }).unwrap();

        // Gemini
        registry.add_provider(crate::provider::ProviderConfig {
            name: "gemini".to_string(),
            matcher: "^/gemini$".to_string(),
            request_mapping: HashMap::new(),
            response_template: None,
            response_body: Some(r#"{"candidates": []}"#.to_string()),
            stream: None,
        }).unwrap();

        let app = create_app(config, None, Arc::new(registry)).await;

        // Test OpenAI
        let response = app.clone()
            .oneshot(Request::builder().method("POST").uri("/v1/chat/completions").header("content-type", "application/json").body(Body::from("{}")).unwrap())
            .await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body_str = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(body_str.contains("chat.completion"));

        // Test Anthropic
        let response = app.clone()
            .oneshot(Request::builder().method("POST").uri("/v1/messages").header("content-type", "application/json").body(Body::from("{}")).unwrap())
            .await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body_str = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(body_str.contains("type\": \"message"));

        // Test Gemini
        let response = app.clone()
            .oneshot(Request::builder().method("POST").uri("/gemini").header("content-type", "application/json").body(Body::from("{}")).unwrap())
            .await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body_str = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(body_str.contains("candidates"));
    }

    #[tokio::test]
    async fn test_status_endpoint() {
        let config = get_test_config();
        let app = create_app(config, None, get_test_registry()).await;

        let response = app
            .oneshot(Request::builder().uri("/status").body(Body::empty()).unwrap())
            .await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        
        assert_eq!(body["port"], 0);
        assert_eq!(body["latency"]["mode"], "benchmark");
    }
}
