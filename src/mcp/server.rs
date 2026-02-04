//! MCP HTTP server setup

use anyhow::Result;
use axum::{
    extract::Request,
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::any, Router,
};
use plexus_core::plexus::Activation;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use std::sync::Arc;
use tokio::task::JoinHandle;

use crate::config::McpHttpConfig;
use crate::mcp::bridge::ActivationMcpBridge;

#[cfg(feature = "sqlite-sessions")]
use crate::mcp::session::{SqliteSessionConfig, SqliteSessionManager};

/// Middleware to log all incoming HTTP requests
async fn log_request_middleware(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let headers = request.headers().clone();

    tracing::info!("▶▶▶ MCP HTTP REQUEST ▶▶▶");
    tracing::info!("  Method: {}", method);
    tracing::info!("  URI: {}", uri);
    tracing::info!("  Headers:");
    for (name, value) in headers.iter() {
        tracing::info!("    {}: {:?}", name, value);
    }

    let response = next.run(request).await;

    let status = response.status();
    tracing::info!("◀◀◀ MCP HTTP RESPONSE ◀◀◀");
    tracing::info!("  Status: {}", status);

    response
}

/// Fallback handler for unmatched routes - logs and returns debug info
async fn fallback_handler(request: Request) -> impl IntoResponse {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let headers = request.headers().clone();

    tracing::error!("╔══════════════════════════════════════════════════════════╗");
    tracing::error!("║  UNMATCHED REQUEST - NO ROUTE FOUND                      ║");
    tracing::error!("╚══════════════════════════════════════════════════════════╝");
    tracing::error!("  Method: {}", method);
    tracing::error!("  URI: {}", uri);
    tracing::error!("  Path: {}", uri.path());
    tracing::error!("  Query: {:?}", uri.query());
    tracing::error!("  Headers:");
    for (name, value) in headers.iter() {
        tracing::error!("    {}: {:?}", name, value);
    }

    let body_hint = if method == axum::http::Method::POST {
        "(body not captured in fallback - check /mcp endpoint)"
    } else {
        "(no body expected)"
    };
    tracing::error!("  Body: {}", body_hint);
    tracing::error!("");
    tracing::error!("  HINT: MCP endpoint is at /mcp");
    tracing::error!("  HINT: Make sure to send 'initialize' request first!");

    let debug_response = format!(
        r#"{{
  "error": "Route not found",
  "received": {{
    "method": "{}",
    "uri": "{}",
    "path": "{}"
  }},
  "hint": "MCP endpoint is at /mcp. Send 'initialize' request first.",
  "available_endpoints": ["/mcp", "/debug"]
}}"#,
        method,
        uri,
        uri.path()
    );

    (
        StatusCode::NOT_FOUND,
        [("content-type", "application/json")],
        debug_response,
    )
}

/// Debug endpoint that returns server info
async fn debug_handler() -> impl IntoResponse {
    tracing::info!("Debug endpoint hit");

    let info = r#"{
  "server": "hub-transport",
  "mcp_endpoint": "/mcp",
  "mcp_protocol": "MCP Streamable HTTP (2025-03-26)",
  "notes": [
    "MCP requires 'initialize' request before 'tools/list'",
    "Accept header must include 'application/json, text/event-stream'",
    "Tool names use format: namespace.method (e.g., 'echo.echo')"
  ]
}"#;

    (StatusCode::OK, [("content-type", "application/json")], info)
}

/// Serve MCP HTTP endpoint for any Activation
///
/// Returns a JoinHandle to the server task. The server will run until
/// the task is cancelled or encounters an error.
pub async fn serve_mcp_http<A: Activation>(
    activation: Arc<A>,
    config: McpHttpConfig,
) -> Result<JoinHandle<std::result::Result<(), std::io::Error>>> {
    tracing::info!("Starting MCP HTTP transport at http://{}/mcp", config.addr);

    let bridge = ActivationMcpBridge::with_server_info(
        activation,
        config.server_name.clone(),
        config.server_version.clone(),
    );

    // Create session manager based on configuration
    #[cfg(feature = "sqlite-sessions")]
    let mcp_service = match config.session_storage {
        crate::config::SessionStorage::InMemory => {
            let session_manager = LocalSessionManager::new();
            let server_config = StreamableHttpServerConfig::default();
            let bridge_clone = bridge.clone();
            StreamableHttpService::new(
                move || Ok(bridge_clone.clone()),
                session_manager.into(),
                server_config,
            )
        }
        crate::config::SessionStorage::Sqlite { path } => {
            let sqlite_config = SqliteSessionConfig {
                db_path: path,
                ..Default::default()
            };
            let session_manager = SqliteSessionManager::new(sqlite_config)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to initialize SQLite session manager: {}", e))?;
            let server_config = StreamableHttpServerConfig::default();
            let bridge_clone = bridge.clone();
            StreamableHttpService::new(
                move || Ok(bridge_clone.clone()),
                session_manager.into(),
                server_config,
            )
        }
    };

    #[cfg(not(feature = "sqlite-sessions"))]
    let mcp_service = {
        let session_manager = LocalSessionManager::default();
        let server_config = StreamableHttpServerConfig::default();
        let bridge_clone = bridge.clone();
        StreamableHttpService::new(
            move || Ok(bridge_clone.clone()),
            session_manager.into(),
            server_config,
        )
    };

    // Build axum router with MCP at /mcp, debug endpoint, and request logging
    let mcp_app = Router::new()
        .nest_service("/mcp", mcp_service)
        .route("/debug", any(debug_handler))
        .fallback(fallback_handler)
        .layer(middleware::from_fn(log_request_middleware));

    // Start MCP HTTP server
    let listener = tokio::net::TcpListener::bind(config.addr).await?;
    let handle = tokio::spawn(async move { axum::serve(listener, mcp_app).await });

    Ok(handle)
}
