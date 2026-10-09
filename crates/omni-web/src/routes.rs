//! Path matching, in route precedence order.

use omni_web_pages::FeedbackKind;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    Home,
    /// The full streamer roster.
    Live,
    Media,
    MediaDetail(String),
    Podcasts,
    PodcastDetail(String),
    Feedback(FeedbackKind, String),
    Pods,
    PodsDetail(String),
    Streamer(String),
    StreamerIntelligence(String),
    /// Ids are passed to the page as signals; the page stays mounted.
    Workspaces,
    Briefings,
    Emails,
    Data,
    Costs,
    Operations,
    Reminders,
    Pets,
    Mcp,
    Claude,
    NotFound,
}

/// Trailing-slash normalization and the `/recommendations` → `/media` alias
/// (old Pushover notifications and bookmarks link there).
pub fn normalize_path(path: &str) -> String {
    let trimmed = if path.len() > 1 && path.ends_with('/') {
        &path[..path.len() - 1]
    } else {
        path
    };
    if trimmed == "/recommendations" {
        "/media".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// `decodeURIComponent`; `None` when it would throw.
pub fn decode_uri_component(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = value.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Segments after `prefix` when the rest is exactly `count` non-empty,
/// slash-free segments (`/^\/prefix\/([^/]+)...$/`).
fn segments<'a>(path: &'a str, prefix: &str, count: usize) -> Option<Vec<&'a str>> {
    let rest = path.strip_prefix(prefix)?.strip_prefix('/')?;
    let parts: Vec<&str> = rest.split('/').collect();
    (parts.len() == count && parts.iter().all(|p| !p.is_empty())).then_some(parts)
}

fn decoded(segment: &str) -> String {
    decode_uri_component(segment).unwrap_or_else(|| segment.to_owned())
}

/// Matches a normalized path.
pub fn match_route(path: &str) -> Route {
    if decode_uri_component(path).is_none() {
        return Route::NotFound;
    }
    if segments(path, "/workspaces", 2).is_some() || segments(path, "/workspaces", 1).is_some() {
        return Route::Workspaces;
    }
    if let Some(parts) = segments(path, "/feedback", 2)
        && let Some(kind) = FeedbackKind::from_segment(parts[0])
    {
        return Route::Feedback(kind, decoded(parts[1]));
    }
    if let Some(parts) = segments(path, "/media", 1) {
        return Route::MediaDetail(decoded(parts[0]));
    }
    if let Some(parts) = segments(path, "/podcasts", 1) {
        return Route::PodcastDetail(decoded(parts[0]));
    }
    if let Some(parts) = segments(path, "/pods", 1) {
        return Route::PodsDetail(decoded(parts[0]));
    }
    if let Some(parts) = segments(path, "/streamers", 2)
        && parts[1] == "intelligence"
    {
        return Route::StreamerIntelligence(decoded(parts[0]));
    }
    if let Some(parts) = segments(path, "/streamers", 1) {
        return Route::Streamer(decoded(parts[0]));
    }
    match path {
        "/reminders" => Route::Reminders,
        "/pets" => Route::Pets,
        "/media" => Route::Media,
        "/data" => Route::Data,
        "/podcasts" => Route::Podcasts,
        "/pods" => Route::Pods,
        "/briefings" => Route::Briefings,
        "/emails" => Route::Emails,
        "/costs" => Route::Costs,
        "/workspaces" => Route::Workspaces,
        "/operations" => Route::Operations,
        "/mcp-activity" => Route::Mcp,
        "/claude" => Route::Claude,
        "/live" => Route::Live,
        "/" => Route::Home,
        _ => Route::NotFound,
    }
}

/// `(workspaceId, subjectId)` of a `/workspaces[/:w[/:s]]` path.
pub fn workspace_ids(path: &str) -> (Option<String>, Option<String>) {
    if let Some(parts) = segments(path, "/workspaces", 2) {
        return (Some(decoded(parts[0])), Some(decoded(parts[1])));
    }
    if let Some(parts) = segments(path, "/workspaces", 1) {
        return (Some(decoded(parts[0])), None);
    }
    (None, None)
}

/// `"<Section> · Omni Notify"` for the static sections.
pub fn page_title(path: &str) -> String {
    let section = if path.starts_with("/workspaces") {
        Some("Workspaces")
    } else {
        match path {
            "/live" => Some("Live"),
            "/reminders" => Some("iCloud Reminders"),
            "/pets" => Some("Pets"),
            "/media" => Some("Watch"),
            "/podcasts" => Some("Podcasts"),
            "/pods" => Some("PressPods"),
            "/briefings" => Some("Briefings"),
            "/emails" => Some("Email Activity"),
            "/data" => Some("Data"),
            "/costs" => Some("Costs"),
            "/workspaces" => Some("Workspaces"),
            "/operations" => Some("Operations"),
            "/mcp-activity" => Some("MCP Activity"),
            "/claude" => Some("Claude Code"),
            _ => None,
        }
    };
    match section {
        Some(section) => format!("{section} · Omni Notify"),
        None => "Omni Notify".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(path: &str) -> Route {
        match_route(&normalize_path(path))
    }

    #[test]
    fn routes_and_deep_links() {
        assert_eq!(route("/"), Route::Home);
        assert_eq!(route("/live"), Route::Live);
        assert_eq!(route("/live/"), Route::Live);
        assert_eq!(route("/live/x"), Route::NotFound);
        assert_eq!(route("/media"), Route::Media);
        assert_eq!(route("/recommendations"), Route::Media);
        assert_eq!(route("/recommendations/"), Route::Media);
        assert_eq!(route("/media/rec%201"), Route::MediaDetail("rec 1".into()));
        assert_eq!(route("/podcasts"), Route::Podcasts);
        assert_eq!(route("/podcasts/p1"), Route::PodcastDetail("p1".into()));
        assert_eq!(
            route("/feedback/recommendations/r1"),
            Route::Feedback(FeedbackKind::Recommendations, "r1".into())
        );
        assert_eq!(
            route("/feedback/podcasts/p%2F1"),
            Route::Feedback(FeedbackKind::Podcasts, "p/1".into())
        );
        assert_eq!(route("/feedback/other/x"), Route::NotFound);
        assert_eq!(route("/pods"), Route::Pods);
        assert_eq!(route("/pods/e1"), Route::PodsDetail("e1".into()));
        assert_eq!(
            route("/streamers/dgg%3Akick%3Ax"),
            Route::Streamer("dgg:kick:x".into())
        );
        assert_eq!(
            route("/streamers/destiny/intelligence"),
            Route::StreamerIntelligence("destiny".into())
        );
        assert_eq!(route("/streamers/destiny/other"), Route::NotFound);
        assert_eq!(route("/briefings"), Route::Briefings);
        assert_eq!(route("/emails"), Route::Emails);
        assert_eq!(route("/data"), Route::Data);
        assert_eq!(route("/costs"), Route::Costs);
        assert_eq!(route("/workspaces"), Route::Workspaces);
        assert_eq!(route("/workspaces/w"), Route::Workspaces);
        assert_eq!(route("/workspaces/w/s"), Route::Workspaces);
        assert_eq!(route("/workspaces/w/s/x"), Route::NotFound);
        assert_eq!(
            workspace_ids("/workspaces/a%20b/s"),
            (Some("a b".into()), Some("s".into()))
        );
        assert_eq!(workspace_ids("/workspaces/a"), (Some("a".into()), None));
        assert_eq!(route("/operations"), Route::Operations);
        assert_eq!(route("/reminders"), Route::Reminders);
        assert_eq!(route("/pets"), Route::Pets);
        assert_eq!(route("/mcp-activity"), Route::Mcp);
        assert_eq!(route("/claude"), Route::Claude);
        assert_eq!(route("/nope"), Route::NotFound);
        assert_eq!(route("/media/%E0%A4%A"), Route::NotFound);
        assert_eq!(route("/%ZZ"), Route::NotFound);
    }

    #[test]
    fn titles_follow_sections() {
        assert_eq!(page_title("/costs"), "Costs · Omni Notify");
        assert_eq!(page_title("/live"), "Live · Omni Notify");
        assert_eq!(page_title("/workspaces/w/s"), "Workspaces · Omni Notify");
        assert_eq!(page_title("/streamers/x"), "Omni Notify");
        assert_eq!(page_title("/"), "Omni Notify");
    }
}
