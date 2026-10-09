//! Hister tools. Results are untrusted
//! archived evidence; only `set_browser_page_label` writes, and it verifies.

pub mod defs;

use std::sync::Arc;

use omni_core::js::trim;
use omni_mcp_kit::{McpTool, ToolContext, ToolError, ToolMetaError, typed_tool};
use serde::Deserialize;

use crate::hister::{BrowseInput, HisterError, HisterService, PageInput, SearchInput};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchToolInput {
    query: String,
    #[serde(default = "ten")]
    limit: u32,
    cursor: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    #[serde(default)]
    semantic: bool,
}

fn ten() -> u32 {
    10
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowseToolInput {
    filter: Option<String>,
    date_from: Option<u64>,
    date_to: Option<u64>,
    cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageToolInput {
    url: String,
    document_id: Option<String>,
    #[serde(default)]
    offset: u64,
    #[serde(default = "twenty_thousand")]
    max_chars: u64,
}

fn twenty_thousand() -> u64 {
    20_000
}

#[derive(Deserialize)]
struct LabelToolInput {
    url: String,
    label: String,
}

fn hister_error(error: HisterError) -> ToolError {
    ToolError::execute(error.to_string())
}

fn require(hister: Option<&Arc<HisterService>>) -> Result<&Arc<HisterService>, ToolError> {
    hister.ok_or_else(|| {
        hister_error(HisterError::new(
            "Hister",
            "not configured; set HISTER_ACCESS_TOKEN",
        ))
    })
}

/// Builds the four browser-history tools; calls fail when Hister is not configured.
pub fn tools(hister: Option<Arc<HisterService>>) -> Result<Vec<McpTool>, ToolMetaError> {
    let h = hister.clone();
    let search = typed_tool(
        &defs::SEARCH_BROWSER_HISTORY,
        move |input: SearchToolInput, _cx: ToolContext| {
            let hister = h.clone();
            async move {
                let query = trim(&input.query).to_owned();
                if query.is_empty() {
                    return Err(ToolError::input(
                        "query: String must contain at least 1 character(s)",
                    ));
                }
                if let (Some(from), Some(to)) = (&input.date_from, &input.date_to)
                    && from > to
                {
                    return Err(ToolError::input("dateFrom must not follow dateTo"));
                }
                let hister = require(hister.as_ref())?;
                let result = hister
                    .search(&SearchInput {
                        query,
                        limit: Some(input.limit),
                        cursor: input.cursor,
                        date_from: input.date_from,
                        date_to: input.date_to,
                        semantic: Some(input.semantic),
                    })
                    .await
                    .map_err(hister_error)?;
                Ok::<_, ToolError>(result)
            }
        },
    )?;
    let h = hister.clone();
    let browse = typed_tool(
        &defs::BROWSE_BROWSER_HISTORY,
        move |input: BrowseToolInput, _cx: ToolContext| {
            let hister = h.clone();
            async move {
                if let (Some(from), Some(to)) = (input.date_from, input.date_to)
                    && from >= to
                {
                    return Err(ToolError::input("dateFrom must precede dateTo"));
                }
                let hister = require(hister.as_ref())?;
                let result = hister
                    .browse(&BrowseInput {
                        filter: input.filter.map(|f| trim(&f).to_owned()),
                        date_from: input.date_from,
                        date_to: input.date_to,
                        cursor: input.cursor,
                    })
                    .await
                    .map_err(hister_error)?;
                Ok::<_, ToolError>(result)
            }
        },
    )?;
    let h = hister.clone();
    let page = typed_tool(
        &defs::GET_BROWSER_PAGE,
        move |input: PageToolInput, _cx: ToolContext| {
            let hister = h.clone();
            async move {
                let hister = require(hister.as_ref())?;
                let result = hister
                    .get_page(&PageInput {
                        url: input.url,
                        document_id: input.document_id,
                        offset: Some(input.offset),
                        max_chars: Some(input.max_chars),
                    })
                    .await
                    .map_err(hister_error)?;
                Ok::<_, ToolError>(result)
            }
        },
    )?;
    let label = typed_tool(
        &defs::SET_BROWSER_PAGE_LABEL,
        move |input: LabelToolInput, _cx: ToolContext| {
            let hister = hister.clone();
            async move {
                let hister = require(hister.as_ref())?;
                let label = trim(&input.label).to_owned();
                let result = hister
                    .set_label(&input.url, &label)
                    .await
                    .map_err(hister_error)?;
                Ok::<_, ToolError>(result)
            }
        },
    )?;
    Ok(vec![search, browse, page, label])
}
