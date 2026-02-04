//! Transport server builder and orchestration

use anyhow::Result;
use plexus_core::plexus::Activation;
use jsonrpsee::server::ServerHandle;
use jsonrpsee::RpcModule;
use std::sync::Arc;
use tokio::task::JoinHandle;

use crate::config::{McpHttpConfig, StdioConfig, TransportConfig, WebSocketConfig};
use crate::mcp::server::serve_mcp_http;
use crate::stdio::serve_stdio;
use crate::websocket::serve_websocket;

/// Function type for converting Arc<Activation> to RpcModule
///
/// This allows each activation type to provide its own conversion logic,
/// which is critical for preserving Arc lifecycle and Weak references.
pub type RpcConverter<A> = Box<dyn FnOnce(Arc<A>) -> Result<RpcModule<()>> + Send>;

/// Transport server that can host any Activation
///
/// Supports multiple transports simultaneously:
/// - WebSocket (JSON-RPC)
/// - Stdio (line-delimited JSON-RPC, MCP-compatible)
/// - MCP HTTP (with SSE streaming)
pub struct TransportServer<A: Activation> {
    activation: Arc<A>,
    config: TransportConfig,
    rpc_converter: Option<RpcConverter<A>>,
}

impl<A: Activation> TransportServer<A> {
    /// Create a builder for configuring transports
    pub fn builder<F>(activation: Arc<A>, rpc_converter: F) -> TransportServerBuilder<A>
    where
        F: FnOnce(Arc<A>) -> Result<RpcModule<()>> + Send + 'static,
    {
        TransportServerBuilder::new(activation, rpc_converter)
    }

    /// Start all configured transports
    ///
    /// If stdio is configured, this will block on stdio (as it's the primary transport).
    /// Otherwise, it will start WebSocket/MCP servers and wait for them to complete.
    pub async fn serve(mut self) -> Result<()> {
        // Convert activation to RPC module for WebSocket/stdio
        let needs_rpc = self.config.websocket.is_some() || self.config.stdio.is_some();
        let module = if needs_rpc {
            let converter = self
                .rpc_converter
                .take()
                .ok_or_else(|| anyhow::anyhow!("RPC converter required for WebSocket/stdio"))?;
            Some(converter(self.activation.clone())?)
        } else {
            None
        };

        // Start stdio transport (blocking)
        if let Some(stdio_config) = self.config.stdio {
            let module = module.expect("RPC module should be created for stdio");
            return serve_stdio(module, stdio_config).await;
        }

        // Start WebSocket transport
        let ws_handle: Option<ServerHandle> = if let Some(ws_config) = self.config.websocket {
            let module = module.expect("RPC module should be created for WebSocket");
            Some(serve_websocket(module, ws_config).await?)
        } else {
            None
        };

        // Start MCP HTTP transport
        let mcp_handle: Option<JoinHandle<std::result::Result<(), std::io::Error>>> =
            if let Some(mcp_config) = self.config.mcp_http {
                Some(serve_mcp_http(self.activation.clone(), mcp_config).await?)
            } else {
                None
            };

        // Wait for servers to complete
        match (ws_handle, mcp_handle) {
            (Some(ws), Some(mcp)) => {
                tokio::select! {
                    _ = ws.stopped() => {
                        tracing::info!("WebSocket server stopped");
                    }
                    result = mcp => {
                        match result {
                            Ok(Ok(())) => tracing::info!("MCP server stopped"),
                            Ok(Err(e)) => tracing::error!("MCP server error: {}", e),
                            Err(e) => tracing::error!("MCP server task failed: {}", e),
                        }
                    }
                }
            }
            (Some(ws), None) => {
                ws.stopped().await;
                tracing::info!("WebSocket server stopped");
            }
            (None, Some(mcp)) => {
                let result = mcp.await;
                match result {
                    Ok(Ok(())) => tracing::info!("MCP server stopped"),
                    Ok(Err(e)) => tracing::error!("MCP server error: {}", e),
                    Err(e) => tracing::error!("MCP server task failed: {}", e),
                }
            }
            (None, None) => {
                tracing::warn!("No transports configured, nothing to serve");
            }
        }

        Ok(())
    }

}

/// Builder for configuring transport servers
pub struct TransportServerBuilder<A: Activation> {
    activation: Arc<A>,
    config: TransportConfig,
    rpc_converter: Option<RpcConverter<A>>,
}

impl<A: Activation> TransportServerBuilder<A> {
    pub fn new<F>(activation: Arc<A>, rpc_converter: F) -> Self
    where
        F: FnOnce(Arc<A>) -> Result<RpcModule<()>> + Send + 'static,
    {
        Self {
            activation,
            config: TransportConfig::default(),
            rpc_converter: Some(Box::new(rpc_converter)),
        }
    }

    /// Enable WebSocket transport on the specified port
    pub fn with_websocket(mut self, port: u16) -> Self {
        self.config.websocket = Some(WebSocketConfig::new(port));
        self
    }

    /// Enable stdio transport (MCP-compatible)
    pub fn with_stdio(mut self) -> Self {
        self.config.stdio = Some(StdioConfig::default());
        self
    }

    /// Enable MCP HTTP transport on the specified port
    pub fn with_mcp_http(mut self, port: u16) -> Self {
        self.config.mcp_http = Some(McpHttpConfig::new(port));
        self
    }

    /// Enable MCP HTTP transport with custom configuration
    pub fn with_mcp_http_config(mut self, config: McpHttpConfig) -> Self {
        self.config.mcp_http = Some(config);
        self
    }

    /// Build the transport server
    pub async fn build(self) -> Result<TransportServer<A>> {
        Ok(TransportServer {
            activation: self.activation,
            config: self.config,
            rpc_converter: self.rpc_converter,
        })
    }
}
