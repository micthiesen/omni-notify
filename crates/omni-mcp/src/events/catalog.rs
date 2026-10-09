//! The MCP Events catalog.
//!
//! Each event declares its arguments, payload schema and the rule that matches
//! a payload to a subscription. Arguments are string-valued filters kept in a
//! canonical form; subscription identity depends on [`canonical_event_arguments`].

use indexmap::IndexMap;
use serde_json::{Map, Value, json};

pub const EMAIL_RECEIVED: &str = "email.received";
pub const CLAUDE_TURN_FINISHED: &str = "claude.session.turn_finished";

/// Validated subscription arguments (insertion ordered).
pub type EventArguments = IndexMap<String, String>;

/// Which event a definition describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    EmailReceived,
    ClaudeTurnFinished,
}

/// One catalog entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventDefinition {
    pub kind: EventKind,
    pub name: &'static str,
    pub description: &'static str,
}

const EMAIL_DESCRIPTION: &str = "A new iCloud message arrived in the selected Inbox or Archive mailbox. Read full mail with email tools using messageId.";
const CLAUDE_DESCRIPTION: &str = "A Claude Code session on the Claude Code host finished its turn or stopped. Omni checks the host about every 15 seconds while a subscription is active. Read the result with claude_session_get (includeResult) using sessionId.";

pub const EVENT_DEFINITIONS: [EventDefinition; 2] = [
    EventDefinition {
        kind: EventKind::EmailReceived,
        name: EMAIL_RECEIVED,
        description: EMAIL_DESCRIPTION,
    },
    EventDefinition {
        kind: EventKind::ClaudeTurnFinished,
        name: CLAUDE_TURN_FINISHED,
        description: CLAUDE_DESCRIPTION,
    },
];

pub fn event_definition(name: &str) -> Option<&'static EventDefinition> {
    EVENT_DEFINITIONS.iter().find(|d| d.name == name)
}

/// `^[A-Za-z0-9][A-Za-z0-9._-]{0,99}$`.
fn is_project_name(value: &str) -> bool {
    let mut chars = value.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && value.len() <= 100
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

impl EventDefinition {
    /// The arguments in canonical form, or `None` when they are invalid.
    pub fn parse_arguments(&self, raw: &Value) -> Option<EventArguments> {
        let args = raw.as_object()?;
        match self.kind {
            EventKind::EmailReceived => {
                if args.len() != 1 {
                    return None;
                }
                let folder = args.get("folder")?.as_str()?;
                matches!(folder, "inbox" | "archive")
                    .then(|| IndexMap::from([("folder".to_owned(), folder.to_owned())]))
            }
            EventKind::ClaudeTurnFinished => {
                if args.keys().any(|key| key != "project") {
                    return None;
                }
                match args.get("project") {
                    None => Some(IndexMap::new()),
                    Some(Value::String(project)) if is_project_name(project) => {
                        Some(IndexMap::from([("project".to_owned(), project.clone())]))
                    }
                    Some(_) => None,
                }
            }
        }
    }

    /// Whether an event with this payload belongs to a subscription's arguments.
    pub fn matches(&self, args: &EventArguments, data: &Map<String, Value>) -> bool {
        match self.kind {
            EventKind::EmailReceived => {
                data.get("folder").and_then(Value::as_str) == args.get("folder").map(String::as_str)
            }
            EventKind::ClaudeTurnFinished => match args.get("project") {
                None => true,
                Some(project) if project.is_empty() => true,
                Some(project) => data.get("project").and_then(Value::as_str) == Some(project),
            },
        }
    }

    pub fn input_schema(&self) -> Value {
        match self.kind {
            EventKind::EmailReceived => json!({
                "type": "object",
                "properties": {"folder": {"type": "string", "enum": ["inbox", "archive"]}},
                "required": ["folder"],
                "additionalProperties": false,
            }),
            EventKind::ClaudeTurnFinished => json!({
                "type": "object",
                "properties": {
                    "project": {
                        "type": "string",
                        "description": "Only sessions in this project (see claude_link_status)",
                    },
                },
                "additionalProperties": false,
            }),
        }
    }

    pub fn payload_schema(&self) -> Value {
        match self.kind {
            EventKind::EmailReceived => json!({
                "type": "object",
                "properties": {
                    "messageId": {"type": "string"},
                    "folder": {"type": "string", "enum": ["inbox", "archive"]},
                    "uidValidity": {"type": "string"},
                    "uid": {"type": "integer"},
                },
                "required": ["messageId", "folder", "uidValidity", "uid"],
                "additionalProperties": false,
            }),
            EventKind::ClaudeTurnFinished => json!({
                "type": "object",
                "properties": {
                    "sessionId": {"type": "string"},
                    "id": {"type": ["string", "null"]},
                    "project": {"type": ["string", "null"]},
                    "status": {"type": "string"},
                    "revision": {"type": "integer"},
                },
                "required": ["sessionId", "id", "project", "status", "revision"],
                "additionalProperties": false,
            }),
        }
    }
}

/// The `events/list` catalog.
pub fn event_catalog() -> Value {
    Value::Array(
        EVENT_DEFINITIONS
            .iter()
            .map(|definition| {
                json!({
                    "name": definition.name,
                    "description": definition.description,
                    "delivery": ["webhook"],
                    "inputSchema": definition.input_schema(),
                    "payloadSchema": definition.payload_schema(),
                })
            })
            .collect(),
    )
}

/// Stable JSON with keys sorted by `localeCompare`; subscription identity depends on it.
pub fn canonical_event_arguments(args: &EventArguments) -> String {
    let mut entries: Vec<(&String, &String)> = args.iter().collect();
    entries.sort_by(|a, b| omni_core::js::locale_compare(a.0, b.0));
    let object: Map<String, Value> = entries
        .into_iter()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect();
    omni_core::js::json_stringify(&Value::Object(object))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_arguments_need_exactly_one_folder() {
        let email = &EVENT_DEFINITIONS[0];
        assert!(email.parse_arguments(&json!({"folder": "inbox"})).is_some());
        assert!(email.parse_arguments(&json!({"folder": "sent"})).is_none());
        assert!(email.parse_arguments(&json!({})).is_none());
        assert!(
            email
                .parse_arguments(&json!({"folder": "inbox", "x": 1}))
                .is_none()
        );
    }

    #[test]
    fn claude_arguments_take_an_optional_safe_project() {
        let claude = &EVENT_DEFINITIONS[1];
        assert_eq!(claude.parse_arguments(&json!({})), Some(IndexMap::new()));
        assert!(
            claude
                .parse_arguments(&json!({"project": "../etc"}))
                .is_none()
        );
        assert!(
            claude
                .parse_arguments(&json!({"project": "omni-notify"}))
                .is_some()
        );
        assert!(claude.parse_arguments(&json!({"other": "x"})).is_none());
    }

    #[test]
    fn canonical_arguments_sort_keys() {
        let args = IndexMap::from([
            ("project".to_owned(), "b".to_owned()),
            ("folder".to_owned(), "a".to_owned()),
        ]);
        assert_eq!(
            canonical_event_arguments(&args),
            r#"{"folder":"a","project":"b"}"#
        );
    }
}
