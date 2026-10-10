//! Streamer configuration MCP tools over [`StreamerConfigService`]. Inputs
//! never carry a Pushover token, so an update keeps whatever the UI set.

pub mod defs;

use omni_api::streamer_config::{
    LiveSettings, StreamerConfigCreate, StreamerConfigPatch, StreamerConfigView,
};
use omni_api::streamers::StreamerTier;
use omni_mcp_kit::{McpTool, ToolContext, ToolError, ToolMetaError, typed_tool};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

use crate::config::{ConfigError, StreamerConfigService};

fn tool_error(error: ConfigError) -> ToolError {
    if error.is_input() {
        ToolError::input(error.to_string())
    } else {
        ToolError::execute(error.to_string())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateInput {
    display_name: String,
    #[serde(default)]
    youtube: Vec<String>,
    #[serde(default)]
    twitch: Vec<String>,
    #[serde(default)]
    kick: Vec<String>,
    #[serde(default)]
    tier: StreamerTier,
    #[serde(default)]
    live_notifications: Option<bool>,
}

/// Distinguishes an explicit `null` from an absent field.
fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Option<bool>>, D::Error> {
    Option::<bool>::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateInput {
    streamer_id: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    youtube: Option<Vec<String>>,
    #[serde(default)]
    twitch: Option<Vec<String>>,
    #[serde(default)]
    kick: Option<Vec<String>>,
    #[serde(default)]
    tier: Option<StreamerTier>,
    #[serde(default, deserialize_with = "present")]
    live_notifications: Option<Option<bool>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IdInput {
    streamer_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReorderInput {
    streamer_ids: Vec<String>,
}

fn streamer(view: &StreamerConfigView) -> Value {
    json!({ "streamer": view })
}

/// The six configuration tools, in [`defs::TOOLS`] order.
pub fn tools(service: &StreamerConfigService) -> Result<Vec<McpTool>, ToolMetaError> {
    let list = {
        let service = service.clone();
        typed_tool(
            &defs::STREAMER_CONFIGS_LIST,
            move |_: Empty, _: ToolContext| {
                let service = service.clone();
                async move { service.list().await.map_err(tool_error) }
            },
        )?
    };
    let create = {
        let service = service.clone();
        typed_tool(
            &defs::STREAMER_CONFIG_CREATE,
            move |input: CreateInput, _: ToolContext| {
                let service = service.clone();
                async move {
                    let created = service
                        .create(StreamerConfigCreate {
                            display_name: input.display_name,
                            youtube: input.youtube,
                            twitch: input.twitch,
                            kick: input.kick,
                            tier: input.tier,
                            live_notifications: input.live_notifications,
                            pushover_token: None,
                        })
                        .await
                        .map_err(tool_error)?;
                    Ok::<_, ToolError>(streamer(&created))
                }
            },
        )?
    };
    let update = {
        let service = service.clone();
        typed_tool(
            &defs::STREAMER_CONFIG_UPDATE,
            move |input: UpdateInput, _: ToolContext| {
                let service = service.clone();
                async move {
                    let patch = StreamerConfigPatch {
                        display_name: input.display_name,
                        youtube: input.youtube,
                        twitch: input.twitch,
                        kick: input.kick,
                        tier: input.tier,
                        live_notifications: input.live_notifications,
                        pushover_token: None,
                    };
                    let updated = service
                        .update(&input.streamer_id, patch)
                        .await
                        .map_err(tool_error)?;
                    Ok::<_, ToolError>(streamer(&updated))
                }
            },
        )?
    };
    let delete = {
        let service = service.clone();
        typed_tool(
            &defs::STREAMER_CONFIG_DELETE,
            move |input: IdInput, _: ToolContext| {
                let service = service.clone();
                async move {
                    let deleted = service
                        .delete(&input.streamer_id)
                        .await
                        .map_err(tool_error)?;
                    Ok::<_, ToolError>(streamer(&deleted))
                }
            },
        )?
    };
    let reorder = {
        let service = service.clone();
        typed_tool(
            &defs::STREAMER_CONFIGS_REORDER,
            move |input: ReorderInput, _: ToolContext| {
                let service = service.clone();
                async move {
                    service
                        .reorder(input.streamer_ids)
                        .await
                        .map_err(tool_error)?;
                    service.list().await.map_err(tool_error)
                }
            },
        )?
    };
    let settings = {
        let service = service.clone();
        typed_tool(
            &defs::LIVESTREAM_SETTINGS_UPDATE,
            move |input: LiveSettings, _: ToolContext| {
                let service = service.clone();
                async move {
                    let settings = service.update_settings(input).await.map_err(tool_error)?;
                    Ok::<_, ToolError>(json!({ "settings": settings }))
                }
            },
        )?
    };
    Ok(vec![list, create, update, delete, reorder, settings])
}
