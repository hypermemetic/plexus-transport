//! WebSocket transport - JSON-RPC over WebSocket

use anyhow::Result;
use jsonrpsee::server::{Server, ServerHandle};
use jsonrpsee::RpcModule;

use crate::config::WebSocketConfig;

/// Serve RPC module over WebSocket
///
/// Starts a WebSocket server that accepts JSON-RPC requests.
/// Returns a handle that can be used to stop the server.
pub async fn serve_websocket(
    module: RpcModule<()>,
    config: WebSocketConfig,
) -> Result<ServerHandle> {
    tracing::info!("Starting WebSocket transport at ws://{}", config.addr);

    let server = Server::builder().build(config.addr).await?;
    let handle = server.start(module);

    Ok(handle)
}
