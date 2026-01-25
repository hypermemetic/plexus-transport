//! Generic MCP server bridge for any Activation
//!
//! This module implements the MCP protocol using the rmcp crate,
//! bridging MCP tool calls to activation methods.

use std::sync::Arc;

use futures::StreamExt;
use hub_core::plexus::{types::PlexusStreamItem, Activation, PlexusError, PluginSchema};
use rmcp::{
    model::*,
    service::{RequestContext, RoleServer},
    ErrorData as McpError, ServerHandler,
};
use serde_json::json;

// =============================================================================
// Schema Transformation
// =============================================================================

/// Convert activation schemas to rmcp Tool format
///
/// MCP requires all tool inputSchema to have "type": "object" at root.
/// schemars may produce schemas without this (e.g., for unit types).
fn schemas_to_rmcp_tools(schemas: Vec<PluginSchema>) -> Vec<Tool> {
    schemas
        .into_iter()
        .flat_map(|activation| {
            let namespace = activation.namespace.clone();
            activation.methods.into_iter().map(move |method| {
                let name = format!("{}.{}", namespace, method.name);
                let description = method.description.clone();

                // Convert schemars::Schema to JSON, ensure "type": "object" exists
                let input_schema = method
                    .params
                    .and_then(|s| serde_json::to_value(s).ok())
                    .and_then(|v| v.as_object().cloned())
                    .map(|mut obj| {
                        // MCP requires "type": "object" at schema root
                        if !obj.contains_key("type") {
                            obj.insert("type".to_string(), json!("object"));
                        }
                        Arc::new(obj)
                    })
                    .unwrap_or_else(|| {
                        // Empty params = empty object schema
                        Arc::new(serde_json::Map::from_iter([(
                            "type".to_string(),
                            json!("object"),
                        )]))
                    });

                Tool::new(name, description, input_schema)
            })
        })
        .collect()
}

// =============================================================================
// Error Mapping
// =============================================================================

/// Convert PlexusError to McpError
fn plexus_to_mcp_error(e: PlexusError) -> McpError {
    match e {
        PlexusError::ActivationNotFound(name) => {
            McpError::invalid_params(format!("Unknown activation: {}", name), None)
        }
        PlexusError::MethodNotFound { activation, method } => McpError::invalid_params(
            format!("Unknown method: {}.{}", activation, method),
            None,
        ),
        PlexusError::InvalidParams(reason) => McpError::invalid_params(reason, None),
        PlexusError::ExecutionError(error) => McpError::internal_error(error, None),
        PlexusError::HandleNotSupported(activation) => McpError::invalid_params(
            format!("Handle resolution not supported: {}", activation),
            None,
        ),
    }
}

// =============================================================================
// Generic Activation MCP Bridge
// =============================================================================

/// MCP handler that bridges to any Activation
///
/// Generic over any type implementing the Activation trait from hub-core.
/// This allows hosting single plugins, Plexus hubs, or nested hubs with
/// the same MCP transport infrastructure.
pub struct ActivationMcpBridge<A: Activation> {
    activation: Arc<A>,
}

impl<A: Activation> ActivationMcpBridge<A> {
    pub fn new(activation: Arc<A>) -> Self {
        Self { activation }
    }
}

impl<A: Activation> Clone for ActivationMcpBridge<A> {
    fn clone(&self) -> Self {
        Self {
            activation: self.activation.clone(),
        }
    }
}

impl<A: Activation> ServerHandler for ActivationMcpBridge<A> {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::LATEST,
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .enable_logging()
                .build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(format!(
                "{} - MCP server powered by hub-transport",
                self.activation.description()
            )),
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        // Get schema from the activation
        let schema = self.activation.plugin_schema();
        let tools = schemas_to_rmcp_tools(vec![schema]);

        tracing::debug!("Listing {} tools", tools.len());

        Ok(ListToolsResult {
            tools,
            next_cursor: None,
            meta: None,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let method_name = &request.name;
        let arguments = request
            .arguments
            .map(serde_json::Value::Object)
            .unwrap_or(json!({}));

        tracing::debug!("Calling tool: {} with args: {:?}", method_name, arguments);

        // Get progress token if provided
        let progress_token = ctx.meta.get_progress_token();

        // Logger name: namespace.method (e.g., bash.execute)
        let logger = method_name.to_string();

        // Extract method name (remove namespace prefix if present)
        let method = if method_name.contains('.') {
            method_name.split('.').nth(1).unwrap_or(method_name)
        } else {
            method_name
        };

        // Call activation and get stream
        let stream = self
            .activation
            .call(method, arguments)
            .await
            .map_err(plexus_to_mcp_error)?;

        // Stream events via notifications AND buffer for final result
        let mut had_error = false;
        let mut buffered_data: Vec<serde_json::Value> = Vec::new();
        let mut error_messages: Vec<String> = Vec::new();

        tokio::pin!(stream);
        while let Some(item) = stream.next().await {
            // Check cancellation on each iteration
            if ctx.ct.is_cancelled() {
                return Err(McpError::internal_error("Cancelled", None));
            }

            match &item {
                PlexusStreamItem::Progress {
                    message,
                    percentage,
                    ..
                } => {
                    // Only send progress if client provided token
                    if let Some(ref token) = progress_token {
                        let _ = ctx
                            .peer
                            .notify_progress(ProgressNotificationParam {
                                progress_token: token.clone(),
                                progress: percentage.unwrap_or(0.0) as f64,
                                total: None,
                                message: Some(message.clone()),
                            })
                            .await;
                    }
                }

                PlexusStreamItem::Data {
                    content,
                    content_type,
                    ..
                } => {
                    // Buffer data for final result
                    buffered_data.push(content.clone());

                    // Also stream via notifications for real-time consumers
                    let _ = ctx
                        .peer
                        .notify_logging_message(LoggingMessageNotificationParam {
                            level: LoggingLevel::Info,
                            logger: Some(logger.clone()),
                            data: json!({
                                "type": "data",
                                "content_type": content_type,
                                "data": content,
                            }),
                        })
                        .await;
                }

                PlexusStreamItem::Error {
                    message,
                    recoverable,
                    ..
                } => {
                    // Buffer errors for final result
                    error_messages.push(message.clone());

                    let _ = ctx
                        .peer
                        .notify_logging_message(LoggingMessageNotificationParam {
                            level: LoggingLevel::Error,
                            logger: Some(logger.clone()),
                            data: json!({
                                "type": "error",
                                "error": message,
                                "recoverable": recoverable,
                            }),
                        })
                        .await;

                    if !recoverable {
                        had_error = true;
                    }
                }

                PlexusStreamItem::Done { .. } => {
                    break;
                }
            }
        }

        // Return buffered data in the final result
        if had_error {
            let error_content = if error_messages.is_empty() {
                "Stream completed with errors".to_string()
            } else {
                error_messages.join("\n")
            };
            Ok(CallToolResult::error(vec![Content::text(error_content)]))
        } else {
            // Convert buffered data to content
            let text_content = if buffered_data.is_empty() {
                "(no output)".to_string()
            } else if buffered_data.len() == 1 {
                // Single value - return as text if string, otherwise JSON
                match &buffered_data[0] {
                    serde_json::Value::String(s) => s.clone(),
                    other => serde_json::to_string_pretty(other).unwrap_or_default(),
                }
            } else {
                // Multiple values - join strings or return as JSON array
                let all_strings = buffered_data.iter().all(|v| v.is_string());
                if all_strings {
                    buffered_data
                        .iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join("")
                } else {
                    serde_json::to_string_pretty(&buffered_data).unwrap_or_default()
                }
            };

            Ok(CallToolResult::success(vec![Content::text(
                text_content,
            )]))
        }
    }
}
