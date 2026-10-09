//! `parcels_list` and `parcels_get`: bounded reads of the cached Parcel
//! deliveries. They never call Parcel.

pub mod defs;

use omni_api::parcels::{ParcelDelivery, ParcelDeliveryStatus, ParcelEvent, ParcelSource};
use omni_core::js::to_iso_string;
use omni_mcp_kit::{McpTool, ToolContext, ToolError, ToolMetaError, typed_tool};
use omni_store::Store;
use serde::{Deserialize, Serialize};

use crate::deliveries::{self, state, view};

#[derive(Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Filter {
    #[default]
    Active,
    All,
}

#[derive(Deserialize)]
struct ListInput {
    #[serde(default)]
    filter: Filter,
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default = "default_events")]
    events: usize,
}

fn default_limit() -> usize {
    20
}

fn default_events() -> usize {
    3
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetInput {
    tracking_number: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CacheInfo {
    configured: bool,
    fetched_at: Option<String>,
    next_read_after: Option<String>,
    backoff_until: Option<String>,
    last_error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Source {
    email_id: String,
    activity_id: String,
    submitted_at: String,
}

impl From<ParcelSource> for Source {
    fn from(source: ParcelSource) -> Self {
        Self {
            email_id: source.email_id,
            activity_id: source.activity_id,
            submitted_at: to_iso_string(source.submitted_at),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Delivery {
    tracking_number: String,
    carrier_code: String,
    carrier_name: Option<String>,
    description: String,
    status: ParcelDeliveryStatus,
    status_label: &'static str,
    active: bool,
    expected: Option<String>,
    expected_end: Option<String>,
    extra_information: Option<String>,
    events: Vec<ParcelEvent>,
    event_count: u32,
    source: Option<Source>,
}

impl Delivery {
    fn new(delivery: ParcelDelivery, max_events: usize) -> Self {
        Self {
            tracking_number: delivery.tracking_number,
            carrier_code: delivery.carrier_code,
            carrier_name: delivery.carrier_name,
            description: delivery.description,
            status: delivery.status,
            status_label: delivery.status.label(),
            active: delivery.active,
            expected: delivery.expected,
            expected_end: delivery.expected_end,
            extra_information: delivery.extra_information,
            events: delivery.events.into_iter().take(max_events).collect(),
            event_count: delivery.event_count,
            source: delivery.source.map(Source::from),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ListOutput {
    cache: CacheInfo,
    active_count: u32,
    total: usize,
    deliveries: Vec<Delivery>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GetOutput {
    cache: CacheInfo,
    delivery: Option<Delivery>,
    submitted: Option<Source>,
}

fn cache_info(response: &omni_api::parcels::ParcelsResponse) -> CacheInfo {
    CacheInfo {
        configured: response.configured,
        fetched_at: response.fetched_at.map(to_iso_string),
        next_read_after: response.next_read_after.map(to_iso_string),
        backoff_until: response.backoff_until.map(to_iso_string),
        last_error: response.last_error.clone(),
    }
}

async fn list(store: Store, configured: bool, input: ListInput) -> Result<ListOutput, ToolError> {
    let response = deliveries::load_response(&store, configured)
        .await
        .map_err(|e| ToolError::execute_from(&e))?;
    let cache = cache_info(&response);
    let matching: Vec<ParcelDelivery> = response
        .deliveries
        .into_iter()
        .filter(|d| input.filter == Filter::All || d.active)
        .collect();
    Ok(ListOutput {
        cache,
        active_count: response.active_count,
        total: matching.len(),
        deliveries: matching
            .into_iter()
            .take(input.limit)
            .map(|d| Delivery::new(d, input.events))
            .collect(),
    })
}

async fn get(store: Store, configured: bool, input: GetInput) -> Result<GetOutput, ToolError> {
    let (snapshot, read_state, submissions) = state::load(&store)
        .await
        .map_err(|e| ToolError::execute_from(&e))?;
    let response = view::response(
        configured,
        snapshot.as_ref(),
        read_state.as_ref(),
        &submissions,
    );
    Ok(GetOutput {
        cache: cache_info(&response),
        delivery: view::find(&response, &input.tracking_number)
            .cloned()
            .map(|d| Delivery::new(d, state::MAX_EVENTS)),
        submitted: view::submitted_source(&submissions, &input.tracking_number).map(Source::from),
    })
}

/// Builds `parcels_list` and `parcels_get`; `configured` is whether
/// `PARCEL_API_KEY` is set (the tools still answer from the cache).
pub fn tools(store: Store, configured: bool) -> Result<Vec<McpTool>, ToolMetaError> {
    let list_store = store.clone();
    let list_tool = typed_tool(
        &defs::PARCELS_LIST,
        move |input: ListInput, _cx: ToolContext| {
            let store = list_store.clone();
            async move { list(store, configured, input).await }
        },
    )?;
    let get_tool = typed_tool(
        &defs::PARCELS_GET,
        move |input: GetInput, _cx: ToolContext| {
            let store = store.clone();
            async move { get(store, configured, input).await }
        },
    )?;
    Ok(vec![list_tool, get_tool])
}
