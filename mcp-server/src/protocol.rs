use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Latest MCP protocol revision implemented by this server.
pub(crate) const LATEST_PROTOCOL_VERSION: &str = "2025-11-25";

/// All protocol revisions this server can serve. The `tools`-only capability
/// surface implemented here (initialize/tools/list/tools/call) is stable
/// across these revisions.
pub(crate) const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18"];

/// Echo the client's requested protocol version when supported, otherwise
/// fall back to the server's latest. Per the MCP lifecycle spec, a client
/// requesting an unsupported version is expected to disconnect.
pub(crate) fn negotiate_protocol_version(requested: Option<&str>) -> &'static str {
    requested
        .and_then(|version| {
            SUPPORTED_PROTOCOL_VERSIONS
                .iter()
                .find(|&&supported| supported == version)
        })
        .copied()
        .unwrap_or(LATEST_PROTOCOL_VERSION)
}

/// Strip control characters and bound length before writing client-supplied
/// strings (e.g. `clientInfo`) to logs.
pub(crate) fn sanitize_log_field(value: &str) -> String {
    const MAX_LEN: usize = 200;
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_LEN)
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Value,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    pub(crate) fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    pub(crate) fn error(id: Value, code: i64, message: String) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError { code, message }),
        }
    }
}

pub(crate) fn serialize_response(response: JsonRpcResponse) -> String {
    serde_json::to_string(&response).unwrap_or_else(|err| {
        json!({
            "jsonrpc": "2.0",
            "id": Value::Null,
            "error": {
                "code": -32603,
                "message": format!("failed to serialize response: {err}")
            }
        })
        .to_string()
    })
}
