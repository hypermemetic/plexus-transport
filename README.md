# hub-transport

Generic transport layer for hosting any `Activation` (from `hub-core`) with multiple protocol backends.

## Overview

`hub-transport` extracts the transport infrastructure from Substrate into a reusable library. It allows you to host **any** activation—single plugins, Plexus hubs, or nested hubs—with WebSocket, stdio, and MCP HTTP transports using a clean builder API.

**Key insight**: Plexus is just an Activation with routing. There's nothing transport-special about it. This library works generically with `impl Activation`.

## Features

- **Generic over `Activation` trait** - Works with any type implementing `hub_core::plexus::Activation`
- **Multiple transports simultaneously**:
  - WebSocket JSON-RPC server
  - Stdio line-delimited JSON-RPC (MCP-compatible)
  - MCP HTTP with SSE streaming
- **Builder pattern API** - Clean, composable configuration
- **Arc lifecycle preservation** - Callback-based RPC conversion keeps Weak references valid
- **In-memory sessions by default** - SQLite persistence opt-in via feature flag

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
hub-transport = { path = "../hub-transport" }

# Optional: SQLite session persistence
hub-transport = { path = "../hub-transport", features = ["sqlite-sessions"] }
```

## Usage

### Hosting a Plexus Hub

```rust
use hub_transport::TransportServer;
use substrate::build_plexus;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Build Plexus hub
    let plexus = build_plexus().await;

    // Provide RPC converter (preserves Arc for Weak references)
    let rpc_converter = |arc: Arc<substrate::Plexus>| {
        substrate::Plexus::arc_into_rpc_module(arc)
            .map_err(|e| anyhow::anyhow!("{}", e))
    };

    // Configure and start transports
    TransportServer::builder(plexus, rpc_converter)
        .with_websocket(4444)
        .with_mcp_http(4445)
        .build().await?
        .serve().await?;

    Ok(())
}
```

### Hosting a Single Plugin

```rust
use hub_transport::TransportServer;
use jsexec::JsExec;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create single plugin activation
    let jsexec = Arc::new(JsExec::new(Default::default()));

    // Provide RPC converter
    let rpc_converter = |arc: Arc<JsExec>| {
        Ok(arc.into_rpc_methods())
    };

    // Host with stdio transport (MCP-compatible)
    TransportServer::builder(jsexec, rpc_converter)
        .with_stdio()
        .build().await?
        .serve().await?;

    Ok(())
}
```

### Stdio Mode (MCP-Compatible)

Perfect for Claude Desktop integration:

```rust
TransportServer::builder(activation, rpc_converter)
    .with_stdio()  // Blocks on stdin, line-delimited JSON-RPC
    .build().await?
    .serve().await?;
```

### Multiple Transports

Run WebSocket and MCP HTTP simultaneously:

```rust
TransportServer::builder(activation, rpc_converter)
    .with_websocket(8888)      // JSON-RPC WebSocket
    .with_mcp_http(8889)        // MCP HTTP with SSE
    .build().await?
    .serve().await?;
```

### SQLite Session Persistence (Optional)

```rust
use hub_transport::{TransportServer, McpHttpConfig, SessionStorage};
use std::path::PathBuf;

let mcp_config = McpHttpConfig::new(8889)
    .with_sqlite(PathBuf::from("sessions.db"));

TransportServer::builder(activation, rpc_converter)
    .with_mcp_http_config(mcp_config)
    .build().await?
    .serve().await?;
```

## Architecture

### Core Components

**`TransportServer<A: Activation>`**
Main server that orchestrates multiple transports. Generic over any Activation type.

**`TransportServerBuilder<A>`**
Builder for configuring which transports to enable. Supports method chaining.

**`ActivationMcpBridge<A>`**
Generic MCP protocol handler that bridges `tools/call` requests to `Activation::call()` methods. Works with any activation type.

### Transport Modules

**`stdio`** - Line-delimited JSON-RPC over stdin/stdout (MCP-compatible)
**`websocket`** - JSON-RPC WebSocket server
**`mcp`** - MCP HTTP with SSE streaming and session management

### RPC Conversion Pattern

The callback-based converter handles the `into_rpc_methods()` consumption issue:

```rust
// User provides: Arc<A> -> Result<RpcModule<()>>
let rpc_converter = |arc: Arc<MyActivation>| {
    // Can call activation-specific conversion logic
    MyActivation::arc_into_rpc_module(arc)
};
```

This preserves the Arc lifecycle, keeping Weak references valid throughout the server's lifetime.

## Configuration Types

### `TransportConfig`
Container for all transport configurations.

### `WebSocketConfig`
```rust
pub struct WebSocketConfig {
    pub addr: SocketAddr,
}
```

### `StdioConfig`
```rust
pub struct StdioConfig {
    pub subscription_buffer_size: usize,  // Default: 1024
}
```

### `McpHttpConfig`
```rust
pub struct McpHttpConfig {
    pub addr: SocketAddr,
    pub session_storage: SessionStorage,
}
```

### `SessionStorage`
```rust
pub enum SessionStorage {
    InMemory,  // Default: simple, no persistence
    Sqlite { path: PathBuf },  // Optional: survives restarts
}
```

## API Reference

### `TransportServer::builder(activation, rpc_converter)`
Create a new transport server builder.

**Parameters:**
- `activation: Arc<A>` - The activation to host
- `rpc_converter: FnOnce(Arc<A>) -> Result<RpcModule<()>>` - Converter function

**Returns:** `TransportServerBuilder<A>`

### `TransportServerBuilder` Methods

#### `.with_websocket(port: u16) -> Self`
Enable WebSocket JSON-RPC transport.

#### `.with_stdio() -> Self`
Enable stdio transport (line-delimited JSON-RPC, MCP-compatible).

#### `.with_mcp_http(port: u16) -> Self`
Enable MCP HTTP transport with default configuration.

#### `.with_mcp_http_config(config: McpHttpConfig) -> Self`
Enable MCP HTTP transport with custom configuration.

#### `.build() -> Result<TransportServer<A>>`
Build the configured transport server.

### `TransportServer` Methods

#### `.serve() -> Result<()>`
Start all configured transports and block until shutdown.

- If stdio is configured: blocks on stdin
- Otherwise: starts WebSocket/MCP servers and waits for completion

## Examples

See `examples/` directory:
- `jsexec_server.rs` - Hosting single JsExec plugin
- `full_plexus.rs` - Hosting complete Plexus hub

## Design Goals

1. **Activation-agnostic** - No special handling for Plexus vs single plugins
2. **Arc lifecycle safety** - Preserve references for cross-activation calls
3. **Clean API** - Builder pattern, composable configuration
4. **MCP-first** - Full MCP protocol support with streaming
5. **Production-ready** - Optional SQLite persistence, proper error handling

## Comparison with Substrate

**Before (substrate/src/main.rs):**
```rust
// ~200 lines of transport setup code
// Hardcoded to Plexus
// Duplicated across projects
```

**After (with hub-transport):**
```rust
// ~20 lines using builder API
// Works with any Activation
// Reusable across projects
```

## License

AGPL-3.0-only

## Contributing

This library is part of the Substrate/Plexus ecosystem. See the main Substrate repository for contribution guidelines.
