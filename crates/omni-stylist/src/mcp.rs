//! Stylist MCP tools: bounded adapters over [`Stylist`] under the existing MCP
//! bearer authentication. Contracts are in [`defs`].

pub mod defs;

use std::future::Future;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use futures::future::BoxFuture;
use omni_mcp_kit::{
    Content, McpTool, SchemaValidator, ToolContext, ToolDef, ToolError, ToolHandler, ToolMetaError,
    ToolOutput, raw_tool, typed_tool,
};
use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use crate::api::{ExportInput, GetEvidenceInput};
use crate::archive::{MAX_INLINE_DOCUMENT_BYTES, Reason};
use crate::error::StylistError;
use crate::service::Stylist;

fn execute(error: StylistError) -> ToolError {
    ToolError::execute(error.tool_text())
}

fn tool<I, O, F, Fut>(
    def: &'static ToolDef<I, O>,
    stylist: &Arc<Stylist>,
    f: F,
) -> Result<McpTool, ToolMetaError>
where
    I: JsonSchema + DeserializeOwned + Send + 'static,
    O: JsonSchema + Serialize + Send + 'static,
    F: Fn(Arc<Stylist>, I) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<O, StylistError>> + Send + 'static,
{
    let stylist = stylist.clone();
    typed_tool(def, move |input: I, _cx: ToolContext| {
        let fut = f(stylist.clone(), input);
        async move { fut.await.map_err(execute) }
    })
}

/// A tool whose result carries an embedded resource.
struct ResourceTool<I, F> {
    input: SchemaValidator,
    output: SchemaValidator,
    run: F,
    _input: std::marker::PhantomData<fn(I)>,
}

/// What a resource tool returns: structured content and an optional resource.
struct WithResource {
    structured: Map<String, Value>,
    resource: Option<(String, String, String)>,
}

impl<I, F, Fut> ToolHandler for ResourceTool<I, F>
where
    I: DeserializeOwned + Send + 'static,
    F: Fn(I) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<WithResource, StylistError>> + Send + 'static,
{
    fn call<'a>(
        &'a self,
        input: Value,
        _cx: ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let input = omni_core::js::normalize_numbers(input);
            self.input.check(&input).map_err(ToolError::input)?;
            let decoded: I =
                serde_json::from_value(input).map_err(|e| ToolError::input(e.to_string()))?;
            let result = (self.run)(decoded).await.map_err(execute)?;
            let structured =
                match omni_core::js::normalize_numbers(Value::Object(result.structured)) {
                    Value::Object(map) => map,
                    _ => Map::new(),
                };
            self.output
                .check(&Value::Object(structured.clone()))
                .map_err(ToolError::output)?;
            let mut summary = structured.clone();
            summary.shift_remove("blob");
            let mut text: Content = Map::new();
            text.insert("type".into(), json!("text"));
            text.insert(
                "text".into(),
                json!(omni_core::js::json_stringify(&Value::Object(summary))),
            );
            let mut content = vec![text];
            if let Some((uri, mime, blob)) = result.resource {
                let mut resource: Content = Map::new();
                resource.insert("type".into(), json!("resource"));
                resource.insert(
                    "resource".into(),
                    json!({"uri": uri, "mimeType": mime, "blob": blob}),
                );
                content.push(resource);
            }
            Ok(ToolOutput::Custom {
                structured,
                content,
            })
        })
    }
}

fn resource_tool<I, O, F, Fut>(
    def: &'static ToolDef<I, O>,
    run: F,
) -> Result<McpTool, ToolMetaError>
where
    I: JsonSchema + DeserializeOwned + Send + 'static,
    O: JsonSchema,
    F: Fn(I) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<WithResource, StylistError>> + Send + 'static,
{
    use omni_mcp_kit::ToolDefinition as _;
    let meta = def.meta()?;
    raw_tool(
        def,
        Arc::new(ResourceTool {
            input: SchemaValidator::new(&meta.name, &meta.input_schema)?,
            output: SchemaValidator::new(&meta.name, &meta.output_schema)?,
            run,
            _input: std::marker::PhantomData,
        }),
    )
}

fn object<T: Serialize>(value: &T) -> Result<Map<String, Value>, StylistError> {
    match serde_json::to_value(value) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(StylistError::Corrupt(
            "tool output must be an object".to_owned(),
        )),
        Err(e) => Err(StylistError::Corrupt(e.to_string())),
    }
}

/// Every stylist tool over `stylist`.
pub fn tools(stylist: Arc<Stylist>) -> Result<Vec<McpTool>, ToolMetaError> {
    let evidence_stylist = stylist.clone();
    let export_stylist = stylist.clone();
    Ok(vec![
        tool(
            &defs::STYLIST_CONTEXT_GET,
            &stylist,
            |s, input| async move { s.context(input).await },
        )?,
        tool(
            &defs::STYLIST_RECORDS_QUERY,
            &stylist,
            |s, input| async move { s.query(input).await },
        )?,
        tool(
            &defs::STYLIST_MEASUREMENTS_COMPARE,
            &stylist,
            |s, input| async move { s.compare(input).await },
        )?,
        tool(
            &defs::STYLIST_RECORD_SAVE,
            &stylist,
            |s, input| async move { s.save_record(input).await },
        )?,
        tool(
            &defs::STYLIST_EVENT_APPEND,
            &stylist,
            |s, input| async move { s.append_event(input).await },
        )?,
        tool(
            &defs::STYLIST_EVIDENCE_STORE,
            &stylist,
            |s, input| async move { s.store_evidence(input).await },
        )?,
        resource_tool(
            &defs::STYLIST_EVIDENCE_GET,
            move |input: GetEvidenceInput| {
                let s = evidence_stylist.clone();
                async move {
                    let mut output = s.get_evidence(input).await?;
                    // The bytes travel once, as the embedded resource.
                    let resource = output.blob.take().map(|blob| {
                        (
                            format!("omni-stylist-evidence:{}", output.evidence.id),
                            output.evidence.mime_type.clone(),
                            blob,
                        )
                    });
                    Ok(WithResource {
                        structured: object(&output)?,
                        resource,
                    })
                }
            },
        )?,
        resource_tool(&defs::STYLIST_EXPORT, move |input: ExportInput| {
            let s = export_stylist.clone();
            async move {
                let exported = s
                    .export(
                        &input.namespace,
                        input.include_bytes.unwrap_or(true),
                        Reason::Export,
                    )
                    .await?;
                let mut output = exported.output;
                let resource = match exported.document {
                    Some(bytes)
                        if input.return_document.unwrap_or(false)
                            && bytes.len() <= MAX_INLINE_DOCUMENT_BYTES =>
                    {
                        output.document_attached = true;
                        Some((
                            format!("omni-stylist-archive:{}", output.manifest.archive_id),
                            "application/json".to_owned(),
                            STANDARD.encode(bytes),
                        ))
                    }
                    _ => None,
                };
                Ok(WithResource {
                    structured: object(&output)?,
                    resource,
                })
            }
        })?,
        tool(
            &defs::STYLIST_ARCHIVES_LIST,
            &stylist,
            |s, input| async move {
                let limit = usize::try_from(input.limit.unwrap_or(25).clamp(1, 100)).unwrap_or(25);
                s.list_archives(input.namespace.as_ref(), limit).await
            },
        )?,
        tool(&defs::STYLIST_RESTORE, &stylist, |s, input| async move {
            s.restore(
                crate::api::ArchiveSource::Id(input.archive_id),
                &input.namespace,
                false,
            )
            .await
        })?,
        tool(
            &defs::STYLIST_FIXTURE_PURGE,
            &stylist,
            |s, input| async move {
                s.purge_fixture(&input.namespace, &input.confirm_namespace)
                    .await
            },
        )?,
    ])
}
