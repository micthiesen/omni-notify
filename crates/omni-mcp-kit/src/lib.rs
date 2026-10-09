//! MCP tool registration (ARCHITECTURE.md section 3.10).
//!
//! Tool metadata (name, title, description, schemas, annotations, policy) is
//! loaded from golden JSON generated from the TS sources ([`golden`]), never
//! hand-written. [`typed_tool`] validates inputs and outputs against the golden
//! schemas; [`registry::ToolRegistry`] formats results like `src/mcp/tool.ts` and
//! serves the tools through rmcp ([`registry::RegistryServer`]).

use std::future::Future;
use std::sync::Arc;

use futures::future::BoxFuture;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

pub mod golden;
pub mod registry;

/// Recommended Executor policy (`docs/mcp-policy.json`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorPolicy {
    Allow,
    RequireApproval,
    Block,
}

/// MCP tool annotations (all four hints are always present).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Annotations {
    pub read_only_hint: bool,
    pub destructive_hint: bool,
    pub idempotent_hint: bool,
    pub open_world_hint: bool,
}

/// `ToolPolicy`: side effects, cost and the recommended Executor policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolPolicy {
    pub side_effects: Vec<String>,
    pub cost: String,
    pub recommended_policy: ExecutorPolicy,
}

/// Golden metadata for one tool.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolMeta {
    pub name: String,
    pub title: String,
    pub description: String,
    pub input_schema: Arc<Map<String, Value>>,
    pub output_schema: Arc<Map<String, Value>>,
    pub annotations: Annotations,
    pub policy: ToolPolicy,
}

#[derive(Debug, thiserror::Error)]
pub enum ToolMetaError {
    #[error("no golden metadata for MCP tool {0:?}")]
    UnknownTool(String),
    #[error("invalid golden MCP metadata: {0}")]
    InvalidGolden(String),
    #[error("invalid golden schema for MCP tool {tool:?}: {reason}")]
    InvalidSchema { tool: String, reason: String },
}

/// Metadata for `name` from `crates/omni-mcp-kit/golden/tools-list.json` and
/// `golden/mcp-policy.json` (embedded at build time).
pub fn golden_meta(name: &str) -> Result<&'static ToolMeta, ToolMetaError> {
    golden::golden_tools()?
        .get(name)
        .map(|(meta, _)| meta)
        .ok_or_else(|| ToolMetaError::UnknownTool(name.to_owned()))
}

/// A compiled JSON-schema validator (draft 2020-12, formats asserted like zod).
///
/// `pattern` keywords follow ECMAScript regex semantics as zod's do: `\d` and `\w`
/// are ASCII-only and `\s` is JS whitespace (U+FEFF but not U+0085). `integer`
/// accepts integral floats such as `2.0` (JS has one number type).
pub struct SchemaValidator {
    validator: jsonschema::Validator,
}

impl SchemaValidator {
    pub fn new(tool: &str, schema: &Map<String, Value>) -> Result<Self, ToolMetaError> {
        let validator = jsonschema::options()
            .should_validate_formats(true)
            .build(&Value::Object(schema.clone()))
            .map_err(|e| ToolMetaError::InvalidSchema {
                tool: tool.to_owned(),
                reason: e.to_string(),
            })?;
        Ok(Self { validator })
    }

    /// `Ok` or every violation as `"<path>: <message>"`, joined by `; `.
    pub fn check(&self, value: &Value) -> Result<(), String> {
        let errors: Vec<String> = self
            .validator
            .iter_errors(value)
            .map(|e| {
                let path = e.instance_path().to_string();
                if path.is_empty() {
                    e.to_string()
                } else {
                    format!("{path}: {e}")
                }
            })
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

/// Per-call context.
#[derive(Clone, Debug)]
pub struct ToolContext {
    pub call_id: String,
    pub cancel: CancellationToken,
}

/// An MCP content block, serialized verbatim (`{"type":"text",...}`, resources, images).
pub type Content = Map<String, Value>;

/// A tool result: structured content, optionally with a custom content list
/// (TS `formatResult`).
#[derive(Clone, Debug, PartialEq)]
pub enum ToolOutput {
    Structured(Map<String, Value>),
    Custom {
        structured: Map<String, Value>,
        content: Vec<Content>,
    },
}

/// Where a tool call failed (`McpToolError.phase`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolPhase {
    Input,
    Execute,
    Output,
}

/// A failed tool call; `message` is the innermost cause (`toolErrorMessage`).
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
#[error("{message}")]
pub struct ToolError {
    pub phase: ToolPhase,
    pub message: String,
}

impl ToolError {
    pub fn input(message: impl Into<String>) -> Self {
        Self {
            phase: ToolPhase::Input,
            message: message.into(),
        }
    }

    pub fn execute(message: impl Into<String>) -> Self {
        Self {
            phase: ToolPhase::Execute,
            message: message.into(),
        }
    }

    /// An execute-phase error from the innermost cause of `error`.
    pub fn execute_from(error: &dyn std::error::Error) -> Self {
        Self::execute(omni_core::error::chain_message(error))
    }

    pub fn output(message: impl Into<String>) -> Self {
        Self {
            phase: ToolPhase::Output,
            message: message.into(),
        }
    }
}

pub trait ToolHandler: Send + Sync {
    fn call<'a>(
        &'a self,
        input: Value,
        cx: ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>>;
}

/// A registered tool: golden metadata plus its handler.
#[derive(Clone)]
pub struct McpTool {
    pub meta: &'static ToolMeta,
    pub handler: Arc<dyn ToolHandler>,
}

struct TypedHandler<I, O, F> {
    f: F,
    input: Option<SchemaValidator>,
    output: Option<SchemaValidator>,
    _types: std::marker::PhantomData<fn(I) -> O>,
}

impl<I, O, F, Fut> ToolHandler for TypedHandler<I, O, F>
where
    I: DeserializeOwned + Send + 'static,
    O: Serialize + Send + 'static,
    F: Fn(I, ToolContext) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<O, ToolError>> + Send + 'static,
{
    fn call<'a>(
        &'a self,
        input: Value,
        cx: ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        // JS has one number type: `2.0` decodes wherever an integer is expected.
        let input = omni_core::js::normalize_numbers(input);
        let checked = match &self.input {
            Some(validator) => validator.check(&input).map_err(ToolError::input),
            None => Ok(()),
        };
        let decoded = checked.and_then(|()| {
            serde_json::from_value::<I>(input).map_err(|e| ToolError::input(e.to_string()))
        });
        let fut = decoded.map(|input| (self.f)(input, cx));
        Box::pin(async move {
            let output = fut?.await?;
            let value =
                serde_json::to_value(output).map_err(|e| ToolError::output(e.to_string()))?;
            // Whole-number doubles serialize as JS prints them (`12`, not `12.0`).
            let value = omni_core::js::normalize_numbers(value);
            if let Some(validator) = &self.output {
                validator.check(&value).map_err(ToolError::output)?;
            }
            match value {
                Value::Object(map) => Ok(ToolOutput::Structured(map)),
                _ => Err(ToolError::output("tool output must be a JSON object")),
            }
        })
    }
}

/// Builds a tool whose input is validated against the golden input schema, then
/// decoded with serde (explicit defaults stand in for zod defaults), and whose output
/// must serialize to a JSON object that satisfies the golden output schema (TS
/// `defineTool`). Integral floats in the input decode into integer fields, and
/// whole-number `f64` output fields serialize without a fraction, as in JS. Fails
/// when `name` has no golden metadata.
pub fn typed_tool<I, O, F, Fut>(name: &str, f: F) -> Result<McpTool, ToolMetaError>
where
    I: DeserializeOwned + Send + 'static,
    O: Serialize + Send + 'static,
    F: Fn(I, ToolContext) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<O, ToolError>> + Send + 'static,
{
    let meta = golden_meta(name)?;
    Ok(McpTool {
        meta,
        handler: Arc::new(TypedHandler {
            f,
            input: Some(SchemaValidator::new(name, &meta.input_schema)?),
            output: Some(SchemaValidator::new(name, &meta.output_schema)?),
            _types: std::marker::PhantomData,
        }),
    })
}

/// A tool with a hand-written handler (custom content via [`ToolOutput::Custom`]);
/// the handler validates its own input. Fails when `name` has no golden metadata.
pub fn raw_tool(name: &str, handler: Arc<dyn ToolHandler>) -> Result<McpTool, ToolMetaError> {
    Ok(McpTool {
        meta: golden_meta(name)?,
        handler,
    })
}

/// `{items, nextCursor | null, total}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<usize>,
    pub total: usize,
}

/// `paginate()`: `items[cursor..cursor+limit]`, `nextCursor` when more remain.
pub fn paginate<T: Serialize>(items: Vec<T>, cursor: usize, limit: usize) -> Page<T> {
    let total = items.len();
    let page: Vec<T> = items.into_iter().skip(cursor).take(limit).collect();
    let next = cursor.saturating_add(page.len());
    Page {
        items: page,
        next_cursor: (next < total).then_some(next),
        total,
    }
}

/// `truncate()`: first `max` UTF-16 units and whether anything was cut.
pub fn truncate_utf16(s: &str, max: usize) -> (String, bool) {
    (
        omni_core::js::utf16_slice(s, 0, max).into_owned(),
        omni_core::js::utf16_len(s) > max,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pagination_matches_ts() {
        let page = paginate(vec![1, 2, 3, 4, 5], 1, 2);
        assert_eq!(
            page,
            Page {
                items: vec![2, 3],
                next_cursor: Some(3),
                total: 5
            }
        );
        let last = paginate(vec![1, 2, 3], 2, 5);
        assert_eq!(
            last,
            Page {
                items: vec![3],
                next_cursor: None,
                total: 3
            }
        );
        let beyond = paginate(vec![1], 9, 5);
        assert_eq!(
            beyond,
            Page {
                items: vec![],
                next_cursor: None,
                total: 1
            }
        );
    }

    #[test]
    fn truncation_counts_utf16() {
        assert_eq!(truncate_utf16("héllo", 3), ("hél".to_owned(), true));
        assert_eq!(truncate_utf16("hi", 3), ("hi".to_owned(), false));
    }

    #[tokio::test]
    async fn typed_handler_decodes_and_encodes() {
        #[derive(Deserialize)]
        struct In {
            n: u32,
        }
        #[derive(Serialize)]
        struct Out {
            doubled: u32,
        }
        let handler = TypedHandler {
            f: |input: In, _cx: ToolContext| async move {
                Ok::<_, ToolError>(Out {
                    doubled: input.n * 2,
                })
            },
            input: None,
            output: None,
            _types: std::marker::PhantomData,
        };
        let cx = || ToolContext {
            call_id: "1".to_owned(),
            cancel: CancellationToken::new(),
        };
        let out = handler.call(serde_json::json!({"n": 2}), cx()).await;
        assert_eq!(
            out,
            Ok(ToolOutput::Structured(
                serde_json::json!({"doubled": 4})
                    .as_object()
                    .cloned()
                    .unwrap_or_default()
            ))
        );
        let bad = handler.call(serde_json::json!({"n": "x"}), cx()).await;
        assert!(matches!(
            bad,
            Err(ToolError {
                phase: ToolPhase::Input,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn numbers_cross_the_handler_as_js_numbers() {
        #[derive(Deserialize)]
        struct In {
            n: u32,
        }
        #[derive(Serialize)]
        struct Out {
            half: f64,
            whole: f64,
        }
        let schema = serde_json::json!({
            "type": "object",
            "properties": {"n": {"type": "integer", "minimum": 0}},
            "required": ["n"],
        });
        let handler = TypedHandler {
            f: |input: In, _cx: ToolContext| async move {
                Ok::<_, ToolError>(Out {
                    half: f64::from(input.n) / 2.0,
                    whole: f64::from(input.n),
                })
            },
            input: SchemaValidator::new(
                "t",
                schema.as_object().cloned().as_ref().unwrap_or(&Map::new()),
            )
            .ok(),
            output: None,
            _types: std::marker::PhantomData,
        };
        let cx = ToolContext {
            call_id: "1".to_owned(),
            cancel: CancellationToken::new(),
        };
        let out = handler.call(serde_json::json!({"n": 3.0}), cx).await;
        let Ok(ToolOutput::Structured(map)) = out else {
            panic!("unexpected {out:?}");
        };
        assert_eq!(
            serde_json::to_string(&map).unwrap_or_default(),
            r#"{"half":1.5,"whole":3}"#
        );
    }

    #[test]
    fn patterns_use_ecmascript_classes() {
        let schema = serde_json::json!({"type": "object", "properties": {
            "d": {"type": "string", "pattern": "^\\d+$"},
            "s": {"type": "string", "pattern": "^\\s+$"},
            "set": {"type": "string", "pattern": "^[\\d\\s]+$"},
        }});
        let validator = SchemaValidator::new("t", schema.as_object().unwrap_or(&Map::new()));
        let Ok(validator) = validator else {
            panic!("schema compiles");
        };
        let check = |key: &str, value: &str| validator.check(&serde_json::json!({key: value}));
        assert!(check("d", "12").is_ok());
        assert!(check("d", "\u{0661}\u{0662}").is_err());
        assert!(check("s", "\u{FEFF}\u{A0}").is_ok());
        assert!(check("s", "\u{85}").is_err());
        assert!(check("set", "1 2").is_ok());
        assert!(check("set", "\u{0663}").is_err());
    }
}
