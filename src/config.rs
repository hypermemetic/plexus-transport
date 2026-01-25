//! Configuration types for transport servers

use std::net::SocketAddr;

#[cfg(feature = "sqlite-sessions")]
use std::path::PathBuf;

/// Complete transport configuration
#[derive(Debug, Clone)]
pub struct TransportConfig {
    pub websocket: Option<WebSocketConfig>,
    pub stdio: Option<StdioConfig>,
    pub mcp_http: Option<McpHttpConfig>,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            websocket: None,
            stdio: None,
            mcp_http: None,
        }
    }
}

/// WebSocket server configuration
#[derive(Debug, Clone)]
pub struct WebSocketConfig {
    pub addr: SocketAddr,
}

impl WebSocketConfig {
    pub fn new(port: u16) -> Self {
        Self {
            addr: format!("127.0.0.1:{}", port)
                .parse()
                .expect("Valid socket address"),
        }
    }
}

/// Stdio (line-delimited JSON-RPC) configuration
#[derive(Debug, Clone)]
pub struct StdioConfig {
    /// Buffer size for subscription notifications
    pub subscription_buffer_size: usize,
}

impl Default for StdioConfig {
    fn default() -> Self {
        Self {
            subscription_buffer_size: 1024,
        }
    }
}

/// MCP HTTP server configuration
#[derive(Debug, Clone)]
pub struct McpHttpConfig {
    pub addr: SocketAddr,
    pub session_storage: SessionStorage,
}

impl McpHttpConfig {
    pub fn new(port: u16) -> Self {
        Self {
            addr: format!("127.0.0.1:{}", port)
                .parse()
                .expect("Valid socket address"),
            session_storage: SessionStorage::default(),
        }
    }

    #[cfg(feature = "sqlite-sessions")]
    pub fn with_sqlite(mut self, path: PathBuf) -> Self {
        self.session_storage = SessionStorage::Sqlite { path };
        self
    }
}

/// Session storage backend for MCP
#[derive(Debug, Clone)]
pub enum SessionStorage {
    /// In-memory sessions (lost on restart, simpler)
    InMemory,
    /// SQLite persistent sessions (survive restarts)
    #[cfg(feature = "sqlite-sessions")]
    Sqlite { path: PathBuf },
}

impl Default for SessionStorage {
    fn default() -> Self {
        Self::InMemory
    }
}
