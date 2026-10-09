//! JSON-RPC request parsing and the modern-era response envelopes.

use serde_json::{Map, Value, json};

/// Returned in every modern result's `_meta`.
pub const SERVER_INFO_KEY: &str = "io.modelcontextprotocol/serverInfo";
pub const PROTOCOL_VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";
pub const CLIENT_CAPABILITIES_KEY: &str = "io.modelcontextprotocol/clientCapabilities";

pub fn server_info() -> Value {
    json!({"name": "Executor with Omni Events", "version": "0.1.0"})
}

/// A JSON-RPC 2.0 request with a string or number id and object (or absent) params.
#[derive(Debug, Clone, PartialEq)]
pub struct RpcRequest {
    pub id: Value,
    pub method: String,
    pub params: Option<Map<String, Value>>,
}

impl RpcRequest {
    /// Parses a request body; anything that is not a single well-formed request
    /// (notifications, responses, batches) is `None`.
    pub fn parse(raw: &[u8]) -> Option<Self> {
        let Value::Object(mut data) = serde_json::from_slice::<Value>(raw).ok()? else {
            return None;
        };
        if data.get("jsonrpc") != Some(&Value::String("2.0".into())) {
            return None;
        }
        let id = data.remove("id")?;
        if !(id.is_string() || id.is_number()) {
            return None;
        }
        let Value::String(method) = data.remove("method")? else {
            return None;
        };
        let params = match data.remove("params") {
            None => None,
            Some(Value::Object(params)) => Some(params),
            Some(_) => return None,
        };
        Some(Self { id, method, params })
    }

    /// `params._meta` when it is an object.
    pub fn meta(&self) -> Option<&Map<String, Value>> {
        self.params.as_ref()?.get("_meta")?.as_object()
    }
}

/// `{jsonrpc, id, error: {code, message, data?}}`
pub fn error_body(id: Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut error = Map::new();
    error.insert("code".into(), code.into());
    error.insert("message".into(), message.into());
    if let Some(data) = data {
        error.insert("data".into(), data);
    }
    json!({"jsonrpc": "2.0", "id": id, "error": error})
}

/// `{resultType: "complete", ...body, _meta: {...body._meta, serverInfo}}`, with
/// JS spread key order (an existing key keeps its position).
pub fn complete(body: Map<String, Value>) -> Map<String, Value> {
    let mut result = Map::new();
    result.insert("resultType".into(), "complete".into());
    let mut meta = Map::new();
    for (key, value) in body {
        if let ("_meta", Value::Object(existing)) = (key.as_str(), &value) {
            meta.extend(existing.clone());
        }
        result.insert(key, value);
    }
    meta.insert(SERVER_INFO_KEY.into(), server_info());
    result.insert("_meta".into(), Value::Object(meta));
    result
}

/// Methods whose 2026-07-28 results must carry `ttlMs` and `cacheScope` (SEP-2549).
const CACHEABLE_METHODS: [&str; 5] = [
    "tools/list",
    "prompts/list",
    "resources/list",
    "resources/templates/list",
    "resources/read",
];

/// Fills the required cache fields on a bridged legacy result the way the MCP 2
/// SDK server does: a valid value Executor sent is kept, otherwise `ttlMs: 0`
/// and `cacheScope: "private"` (results are per OAuth user). Without them the
/// MCP 2 client rejects the result.
pub fn fill_cache_fields(method: &str, body: &mut Map<String, Value>) {
    if !CACHEABLE_METHODS.contains(&method) {
        return;
    }
    let valid_ttl = body
        .get("ttlMs")
        .and_then(Value::as_u64)
        .is_some_and(|ttl| ttl < (1 << 53));
    if !valid_ttl {
        body.insert("ttlMs".into(), 0.into());
    }
    let valid_scope = matches!(
        body.get("cacheScope").and_then(Value::as_str),
        Some("public" | "private")
    );
    if !valid_scope {
        body.insert("cacheScope".into(), "private".into());
    }
}

/// A successful modern response.
pub fn result_body(id: Value, body: Map<String, Value>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": complete(body)})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_only_single_requests() {
        let ok = RpcRequest::parse(br#"{"jsonrpc":"2.0","id":7,"method":"m","params":{}}"#);
        assert_eq!(ok.unwrap().id, json!(7));
        assert!(RpcRequest::parse(br#"{"jsonrpc":"2.0","method":"m"}"#).is_none());
        assert!(RpcRequest::parse(br#"{"jsonrpc":"2.0","id":null,"method":"m"}"#).is_none());
        assert!(
            RpcRequest::parse(br#"{"jsonrpc":"2.0","id":1,"method":"m","params":[]}"#).is_none()
        );
        assert!(
            RpcRequest::parse(br#"{"jsonrpc":"2.0","id":1,"method":"m","params":null}"#).is_none()
        );
        assert!(RpcRequest::parse(br#"{"jsonrpc":"1.0","id":1,"method":"m"}"#).is_none());
        assert!(RpcRequest::parse(br#"[{"jsonrpc":"2.0","id":1,"method":"m"}]"#).is_none());
        assert!(RpcRequest::parse(b"not json").is_none());
    }

    #[test]
    fn complete_keeps_spread_order_and_merges_meta() {
        let body = json!({"tools": [], "_meta": {"x": 1}, "resultType": "input_required"});
        let rendered = Value::Object(complete(body.as_object().cloned().unwrap()));
        assert_eq!(
            serde_json::to_string(&rendered).unwrap(),
            r#"{"resultType":"input_required","tools":[],"_meta":{"x":1,"io.modelcontextprotocol/serverInfo":{"name":"Executor with Omni Events","version":"0.1.0"}}}"#
        );
        let plain = Value::Object(complete(Map::new()));
        assert_eq!(plain["resultType"], "complete");
        assert_eq!(plain["_meta"][SERVER_INFO_KEY]["version"], "0.1.0");
    }

    #[test]
    fn cacheable_results_get_private_zero_ttl_defaults() {
        let mut listed = json!({"tools": [], "ttlMs": -1})
            .as_object()
            .cloned()
            .unwrap();
        fill_cache_fields("tools/list", &mut listed);
        assert_eq!(
            Value::Object(listed),
            json!({"tools": [], "ttlMs": 0, "cacheScope": "private"})
        );
        let mut kept = json!({"contents": [], "ttlMs": 5000, "cacheScope": "public"})
            .as_object()
            .cloned()
            .unwrap();
        fill_cache_fields("resources/read", &mut kept);
        assert_eq!(kept["ttlMs"], 5000);
        assert_eq!(kept["cacheScope"], "public");
        let mut prompt = json!({"messages": []}).as_object().cloned().unwrap();
        fill_cache_fields("prompts/get", &mut prompt);
        assert!(!prompt.contains_key("ttlMs"));
    }
}
