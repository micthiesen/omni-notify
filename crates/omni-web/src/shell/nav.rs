//! The information architecture: rail groups, `g` shortcuts, the current
//! item of a path and its breadcrumbs. Pure, so it is unit-tested natively.

use omni_web_kit::components::Icon;

/// A rail group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Top,
    Watch,
    Listen,
    Research,
    Personal,
    System,
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Group::Top => "",
            Group::Watch => "Watch",
            Group::Listen => "Listen",
            Group::Research => "Research",
            Group::Personal => "Personal",
            Group::System => "System",
        }
    }

    /// Where the group's crumb links (its first item).
    pub fn href(self) -> &'static str {
        match self {
            Group::Top => "/",
            Group::Watch => "/media",
            Group::Listen => "/podcasts",
            Group::Research => "/workspaces",
            Group::Personal => "/emails",
            Group::System => "/operations",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NavItem {
    pub label: &'static str,
    pub href: &'static str,
    pub icon: Icon,
    pub group: Group,
    /// Second key of the `g` shortcut.
    pub key: Option<char>,
}

const fn item(
    label: &'static str,
    href: &'static str,
    icon: Icon,
    group: Group,
    key: Option<char>,
) -> NavItem {
    NavItem {
        label,
        href,
        icon,
        group,
        key,
    }
}

/// Every destination except `/live` (the On air header), in rail order.
pub const NAV: [NavItem; 14] = [
    item("Home", "/", Icon::Home, Group::Top, Some('h')),
    item("Movies & TV", "/media", Icon::Film, Group::Watch, Some('m')),
    item(
        "Podcasts",
        "/podcasts",
        Icon::Headphones,
        Group::Listen,
        Some('p'),
    ),
    item("PressPods", "/pods", Icon::Mic, Group::Listen, None),
    item(
        "Workspaces",
        "/workspaces",
        Icon::Flask,
        Group::Research,
        Some('w'),
    ),
    item(
        "Briefings",
        "/briefings",
        Icon::Doc,
        Group::Research,
        Some('b'),
    ),
    item("Email", "/emails", Icon::Mail, Group::Personal, Some('e')),
    item(
        "Reminders",
        "/reminders",
        Icon::CheckSquare,
        Group::Personal,
        None,
    ),
    item("Pets", "/pets", Icon::Paw, Group::Personal, None),
    item(
        "Operations",
        "/operations",
        Icon::Pulse,
        Group::System,
        Some('o'),
    ),
    item("Costs", "/costs", Icon::Coin, Group::System, Some('c')),
    item("MCP", "/mcp-activity", Icon::Plug, Group::System, None),
    item("Claude", "/claude", Icon::Terminal, Group::System, None),
    item("Data", "/data", Icon::Database, Group::System, Some('d')),
];

pub const LIVE_HREF: &str = "/live";

/// The `g` shortcut target for `key`.
pub fn shortcut_target(key: char) -> Option<&'static str> {
    if key == 'l' {
        return Some(LIVE_HREF);
    }
    NAV.iter().find(|i| i.key == Some(key)).map(|i| i.href)
}

/// The rail href that is current for `path` (`/live` covers streamer pages
/// whose streamer is offline; the shell marks live ones on their own row).
pub fn current_href(path: &str) -> Option<&'static str> {
    if path == LIVE_HREF || path.starts_with("/streamers/") {
        return Some(LIVE_HREF);
    }
    if path.starts_with("/feedback/recommendations/") {
        return Some("/media");
    }
    if path.starts_with("/feedback/podcasts/") {
        return Some("/podcasts");
    }
    NAV.iter()
        .filter(|i| i.href != "/")
        .find(|i| path == i.href || path.starts_with(&format!("{}/", i.href)))
        .map(|i| i.href)
        .or((path == "/").then_some("/"))
}

pub fn item_for(href: &str) -> Option<&'static NavItem> {
    NAV.iter().find(|i| i.href == href)
}

/// One breadcrumb; `href` is `None` for the current page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Crumb {
    pub label: String,
    pub href: Option<String>,
}

fn link(label: impl Into<String>, href: impl Into<String>) -> Crumb {
    Crumb {
        label: label.into(),
        href: Some(href.into()),
    }
}

fn here(label: impl Into<String>) -> Crumb {
    Crumb {
        label: label.into(),
        href: None,
    }
}

/// Names the shell resolves for crumbs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CrumbNames {
    /// The page's own label (streamer, workspace subject, pick title).
    pub page: Option<String>,
    /// The streamer's display name (`/streamers/:id`).
    pub streamer: Option<String>,
    /// The workspace's title (`/workspaces/:w[/:s]`).
    pub workspace: Option<String>,
}

/// Breadcrumbs for a normalized path. `streamer_id` and `workspace_id` come
/// from the route; `names` fills dynamic labels.
pub fn crumbs(
    path: &str,
    route_detail: Option<(&str, Option<&str>)>,
    names: &CrumbNames,
) -> Vec<Crumb> {
    let page = names.page.clone();
    if path == "/" {
        return vec![here("Home")];
    }
    if path == LIVE_HREF {
        return vec![here("Live")];
    }
    if let Some(rest) = path.strip_prefix("/streamers/") {
        let id = route_detail.map_or(rest, |(id, _)| id);
        let name = names
            .streamer
            .clone()
            .or_else(|| page.clone())
            .unwrap_or_else(|| id.to_owned());
        return if path.ends_with("/intelligence") {
            vec![
                link("Live", LIVE_HREF),
                link(
                    name,
                    format!("/streamers/{}", omni_api::common::encode_uri_component(id)),
                ),
                here("Intelligence"),
            ]
        } else {
            vec![link("Live", LIVE_HREF), here(name)]
        };
    }
    if path.starts_with("/workspaces/") {
        let (workspace_id, subject) = route_detail.unwrap_or((path, None));
        let title = names
            .workspace
            .clone()
            .unwrap_or_else(|| workspace_id.to_owned());
        return match subject {
            Some(_) => vec![
                link("Research", "/workspaces"),
                link(
                    title,
                    format!(
                        "/workspaces/{}",
                        omni_api::common::encode_uri_component(workspace_id)
                    ),
                ),
                here(page.unwrap_or_else(|| "Subject".to_owned())),
            ],
            None => vec![link("Research", "/workspaces"), here(page.unwrap_or(title))],
        };
    }
    if let Some(current) = current_href(path)
        && let Some(item) = item_for(current)
    {
        if path == item.href {
            return vec![
                link(item.group.label(), item.group.href()),
                here(item.label),
            ];
        }
        let detail = if path.starts_with("/feedback/") {
            "Feedback".to_owned()
        } else {
            page.unwrap_or_else(|| "Details".to_owned())
        };
        return vec![link(item.label, item.href), here(detail)];
    }
    vec![here("Not found")]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(crumbs: &[Crumb]) -> Vec<(&str, bool)> {
        crumbs
            .iter()
            .map(|c| (c.label.as_str(), c.href.is_some()))
            .collect()
    }

    #[test]
    fn current_item_follows_sections() {
        assert_eq!(current_href("/"), Some("/"));
        assert_eq!(current_href("/media/abc"), Some("/media"));
        assert_eq!(current_href("/feedback/podcasts/x"), Some("/podcasts"));
        assert_eq!(current_href("/workspaces/w/s"), Some("/workspaces"));
        assert_eq!(current_href("/streamers/hutch"), Some("/live"));
        assert_eq!(current_href("/podsx"), None);
        assert_eq!(current_href("/nope"), None);
    }

    #[test]
    fn shortcuts_cover_the_documented_keys() {
        for (key, href) in [
            ('h', "/"),
            ('l', "/live"),
            ('m', "/media"),
            ('p', "/podcasts"),
            ('w', "/workspaces"),
            ('b', "/briefings"),
            ('e', "/emails"),
            ('o', "/operations"),
            ('c', "/costs"),
            ('d', "/data"),
        ] {
            assert_eq!(shortcut_target(key), Some(href), "g {key}");
        }
        assert_eq!(shortcut_target('z'), None);
    }

    #[test]
    fn crumbs_name_the_trail() {
        let names = CrumbNames {
            streamer: Some("Hutch".into()),
            ..CrumbNames::default()
        };
        assert_eq!(
            labels(&crumbs(
                "/streamers/hutch/intelligence",
                Some(("hutch", None)),
                &names
            )),
            [("Live", true), ("Hutch", true), ("Intelligence", false)]
        );
        assert_eq!(
            labels(&crumbs("/operations", None, &CrumbNames::default())),
            [("System", true), ("Operations", false)]
        );
        let names = CrumbNames {
            page: Some("2022 NCM C7".into()),
            workspace: Some("Marketplace Selling".into()),
            ..CrumbNames::default()
        };
        assert_eq!(
            labels(&crumbs(
                "/workspaces/marketplace-selling/s1",
                Some(("marketplace-selling", Some("s1"))),
                &names
            )),
            [
                ("Research", true),
                ("Marketplace Selling", true),
                ("2022 NCM C7", false)
            ]
        );
        assert_eq!(
            labels(&crumbs("/nope", None, &CrumbNames::default())),
            [("Not found", false)]
        );
    }
}
