//! Wire DTOs for every REST/SSE contract.
//!
//! `serde` + `serde_json` only; must compile for `wasm32-unknown-unknown`.
//! Serde rules: `rename_all = "camelCase"`; TS `T | null` is `Option<T>` and is
//! always serialized; TS `field?: T` is
//! `#[serde(default, skip_serializing_if = "Option::is_none")]`; string unions
//! are enums with explicit `rename`; never `deny_unknown_fields`.
//!
//! Each module file is owned by exactly one work package (listed per module).

/// WP00: error body, pagination, path builders.
pub mod common;
/// WP00: `/api/costs`.
pub mod costs;
/// WP00: task runs, run logs and the run-log SSE frames.
pub mod runs;
/// WP00: `/api/tasks`.
pub mod tasks;

/// WP14: data manager.
pub mod data;
/// WP14: dashboard snapshot (`/api/snapshot`, `/api/events`).
pub mod snapshot;

/// WP04: iOS live controls.
pub mod ios;
/// WP04: streamers, metrics, sessions, trigger channels.
pub mod streamers;

/// WP05: livestream intelligence details and feedback.
pub mod intelligence;

/// WP02: email activity, logs, rules, feedback.
pub mod email;

/// WP06: PressPods episodes, jobs, details.
pub mod presspods;

/// WP07: podcast recommendations and taste profile.
pub mod podcasts;

/// WP08: media recommendations, taste profile, on-deck items.
pub mod media;

/// WP11: briefings.
pub mod briefings;
/// WP11: workspaces.
pub mod workspaces;

/// WP12: Claude activity, sessions and transcripts.
pub mod claude;
/// WP12: MCP activity.
pub mod mcp_activity;

/// WP10: Reminders public status.
pub mod reminders;

/// WP13: pets.
pub mod pets;
