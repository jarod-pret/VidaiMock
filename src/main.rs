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

use mimalloc::MiMalloc;
use crate::config::AppConfig;
use crate::server::start_server;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;
use metrics_exporter_prometheus::PrometheusBuilder;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

mod config;
// mod formats; // Removed
mod handlers;
mod replacer;
mod server;
mod provider;
mod aws_event_stream; // Added for Bedrock streaming



fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = AppConfig::load()?;
    let workers = config.workers;

    // Initialize Prometheus Metrics
    let builder = PrometheusBuilder::new();
    let handle = match builder.install_recorder() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("ERROR: Failed to initialize Prometheus metrics: {}", e);
            eprintln!("       This may happen if the metrics port is already in use.");
            eprintln!("       Try stopping other VidaiMock instances or check port conflicts.");
            std::process::exit(1);
        }
    };

    // Initialize Logging
    let log_level = match config.log_level.to_lowercase().as_str() {
        "debug" => Level::DEBUG,
        "warn" => Level::WARN,
        "error" => Level::ERROR,
        "off" => Level::ERROR,
        _ => Level::INFO,
    };

    let subscriber = FmtSubscriber::builder()
        .with_max_level(log_level)
        .finish();

    if let Err(e) = tracing::subscriber::set_global_default(subscriber) {
        eprintln!("ERROR: Failed to initialize logging: {}", e);
        std::process::exit(1);
    }

    if config.log_level != "off" {
        tracing::info!("VidaiMock Initialization (Workers: {}, Latency: {}ms, Mode: {})", 
            workers, config.latency.base_ms, config.latency.mode);

        // Diagnostic: List embedded assets
        for file in crate::provider::Asset::iter() {
            tracing::debug!("Embedded Asset: {}", file);
        }
        
        let endpoints: Vec<String> = config.endpoints.iter().map(|e| e.path.clone()).collect();
        info!(endpoints = ?endpoints, "Registered Endpoints");
    }

    let registry = crate::provider::init_registry(&config.config_dir);

    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers as usize)
        .enable_all()
        .build()?
        .block_on(start_server(config, handle, registry))
}
