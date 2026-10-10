//! `/live/streamers`: add, edit, delete and reorder tracked streamers, and the
//! global Destiny.gg top-embeds setting. The server validates everything; the
//! form mirrors the rules lightly so mistakes show before a round trip.
//!
//! Pushover application tokens are write-only: the API only reports whether
//! one is set, so the UI never shows one.

use leptos::prelude::*;
use omni_api::streamer_config::{
    LiveSettings, StreamerConfigCreate, StreamerConfigPatch, StreamerConfigResponse,
    StreamerConfigView,
};
use omni_api::streamers::StreamerTier;
use omni_web_kit::api;
use omni_web_kit::components::{
    Button, ButtonSize, ButtonVariant, ConfirmButton, EmptyState, ErrorState, Icon, InlineNote,
    Inspector, PageHead, Panel, PlatformIcon, SegOption, Segmented, SkeletonRows, Tag, ToastKind,
    Tone, use_toast,
};
use omni_web_kit::task::{spawn_detached, spawn_scoped};

/// Display names and usernames are at most this many characters.
pub const MAX_NAME_CHARS: usize = 100;
/// Usernames per platform.
pub const MAX_PER_PLATFORM: usize = 10;
/// Upper bound of `dggTopEmbeds`.
pub const MAX_DGG_TOP_EMBEDS: u32 = 20;
/// Pushover application tokens are alphanumeric and at most this long.
pub const MAX_TOKEN_CHARS: usize = 64;

/// A platform a streamer can have sources on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Platform {
    YouTube,
    Twitch,
    Kick,
}

impl Platform {
    pub const ALL: [Platform; 3] = [Platform::YouTube, Platform::Twitch, Platform::Kick];

    pub fn label(self) -> &'static str {
        match self {
            Platform::YouTube => "YouTube",
            Platform::Twitch => "Twitch",
            Platform::Kick => "Kick",
        }
    }

    /// The API and `PlatformIcon` key.
    pub fn key(self) -> &'static str {
        match self {
            Platform::YouTube => "youtube",
            Platform::Twitch => "twitch",
            Platform::Kick => "kick",
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Platform::YouTube => "@handle",
            Platform::Twitch | Platform::Kick => "username",
        }
    }

    fn index(self) -> usize {
        match self {
            Platform::YouTube => 0,
            Platform::Twitch => 1,
            Platform::Kick => 2,
        }
    }
}

fn platform_list(view: &StreamerConfigView, platform: Platform) -> &[String] {
    match platform {
        Platform::YouTube => &view.youtube,
        Platform::Twitch => &view.twitch,
        Platform::Kick => &view.kick,
    }
}

/// The editable fields of one streamer (everything but the token).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StreamerDraft {
    pub display_name: String,
    pub youtube: Vec<String>,
    pub twitch: Vec<String>,
    pub kick: Vec<String>,
    pub tier: StreamerTier,
    /// `None` uses the tier default.
    pub live_notifications: Option<bool>,
}

impl StreamerDraft {
    pub fn from_view(view: &StreamerConfigView) -> Self {
        Self {
            display_name: view.display_name.clone(),
            youtube: view.youtube.clone(),
            twitch: view.twitch.clone(),
            kick: view.kick.clone(),
            tier: view.tier,
            live_notifications: view.live_notifications,
        }
    }

    pub fn usernames(&self, platform: Platform) -> &[String] {
        match platform {
            Platform::YouTube => &self.youtube,
            Platform::Twitch => &self.twitch,
            Platform::Kick => &self.kick,
        }
    }

    pub fn usernames_mut(&mut self, platform: Platform) -> &mut Vec<String> {
        match platform {
            Platform::YouTube => &mut self.youtube,
            Platform::Twitch => &mut self.twitch,
            Platform::Kick => &mut self.kick,
        }
    }

    /// The background tier mutes notifications, so it drops any override.
    pub fn set_tier(&mut self, tier: StreamerTier) {
        self.tier = tier;
        if tier == StreamerTier::Background {
            self.live_notifications = None;
        }
    }

    fn source_count(&self) -> usize {
        Platform::ALL.iter().map(|p| self.usernames(*p).len()).sum()
    }
}

/// Adds the usernames in `input` (separated by whitespace or commas) to
/// `list`, skipping case-insensitive repeats. Fails when one is too long or
/// the platform would exceed [`MAX_PER_PLATFORM`].
pub fn add_usernames(list: &[String], input: &str) -> Result<Vec<String>, String> {
    let mut next = list.to_vec();
    for name in input
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|n| !n.is_empty())
    {
        if name.chars().count() > MAX_NAME_CHARS {
            return Err(format!("\u{201c}{name}\u{201d} is too long for a username"));
        }
        if next.iter().any(|n| n.eq_ignore_ascii_case(name)) {
            continue;
        }
        if next.len() >= MAX_PER_PLATFORM {
            return Err(format!("At most {MAX_PER_PLATFORM} usernames per platform"));
        }
        next.push(name.to_owned());
    }
    Ok(next)
}

/// Problems the form shows before saving; the server stays authoritative.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DraftProblems {
    pub name: Option<String>,
    pub sources: Option<String>,
}

impl DraftProblems {
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.sources.is_none()
    }
}

/// Checks `draft` against the other configured streamers (`editing` is the
/// id being edited, excluded from the uniqueness checks).
pub fn draft_problems(
    draft: &StreamerDraft,
    streamers: &[StreamerConfigView],
    editing: Option<&str>,
) -> DraftProblems {
    let others: Vec<&StreamerConfigView> = streamers
        .iter()
        .filter(|s| Some(s.id.as_str()) != editing)
        .collect();
    let name = draft.display_name.trim();
    let name_problem = if name.is_empty() {
        Some("Enter a display name".to_owned())
    } else if name.chars().count() > MAX_NAME_CHARS {
        Some(format!("At most {MAX_NAME_CHARS} characters"))
    } else if others
        .iter()
        .any(|s| s.display_name.trim().to_lowercase() == name.to_lowercase())
    {
        Some(format!("Another streamer is already named {name}"))
    } else {
        None
    };
    let sources_problem = if draft.source_count() == 0 {
        Some("Add at least one YouTube, Twitch or Kick source".to_owned())
    } else {
        Platform::ALL.iter().find_map(|platform| {
            let names = draft.usernames(*platform);
            if names.len() > MAX_PER_PLATFORM {
                return Some(format!(
                    "{}: at most {MAX_PER_PLATFORM} usernames",
                    platform.label()
                ));
            }
            names.iter().find_map(|name| {
                others
                    .iter()
                    .find(|s| {
                        platform_list(s, *platform)
                            .iter()
                            .any(|n| n.eq_ignore_ascii_case(name))
                    })
                    .map(|owner| {
                        format!(
                            "{} {name} already belongs to {}",
                            platform.label(),
                            owner.display_name
                        )
                    })
            })
        })
    };
    DraftProblems {
        name: name_problem,
        sources: sources_problem,
    }
}

/// Why `token` (already trimmed) is not a Pushover application token.
pub fn token_problem(token: &str) -> Option<String> {
    if token.chars().count() > MAX_TOKEN_CHARS || !token.chars().all(|c| c.is_ascii_alphanumeric())
    {
        Some(format!(
            "A Pushover application token is letters and digits only, at most {MAX_TOKEN_CHARS}"
        ))
    } else {
        None
    }
}

/// The create body: trimmed name, every source, and the token when given.
pub fn build_create(draft: &StreamerDraft, token: &str) -> StreamerConfigCreate {
    let token = token.trim();
    StreamerConfigCreate {
        display_name: draft.display_name.trim().to_owned(),
        youtube: draft.youtube.clone(),
        twitch: draft.twitch.clone(),
        kick: draft.kick.clone(),
        tier: draft.tier,
        live_notifications: match draft.tier {
            StreamerTier::Primary => draft.live_notifications,
            StreamerTier::Background => None,
        },
        pushover_token: (!token.is_empty()).then(|| token.to_owned()),
    }
}

/// Only the fields that differ from `original`. Switching to background
/// leaves `liveNotifications` out: the server clears it with the tier.
pub fn build_patch(original: &StreamerConfigView, draft: &StreamerDraft) -> StreamerConfigPatch {
    let name = draft.display_name.trim();
    let changed = |platform: Platform| {
        let next = draft.usernames(platform);
        (next != platform_list(original, platform)).then(|| next.to_vec())
    };
    let live_notifications = match draft.tier {
        StreamerTier::Background => None,
        StreamerTier::Primary => (draft.live_notifications != original.live_notifications)
            .then_some(draft.live_notifications),
    };
    StreamerConfigPatch {
        display_name: (name != original.display_name).then(|| name.to_owned()),
        youtube: changed(Platform::YouTube),
        twitch: changed(Platform::Twitch),
        kick: changed(Platform::Kick),
        tier: (draft.tier != original.tier).then_some(draft.tier),
        live_notifications,
        pushover_token: None,
    }
}

/// `ids` with `id` moved one place up or down; `None` at either end.
pub fn move_order(ids: &[String], id: &str, up: bool) -> Option<Vec<String>> {
    let index = ids.iter().position(|i| i == id)?;
    let target = if up {
        index.checked_sub(1)?
    } else {
        (index + 1 < ids.len()).then_some(index + 1)?
    };
    let mut next = ids.to_vec();
    next.swap(index, target);
    Some(next)
}

/// Parses the top-embeds field (0 to [`MAX_DGG_TOP_EMBEDS`]).
pub fn parse_top_embeds(input: &str) -> Result<u32, String> {
    input
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|n| *n <= MAX_DGG_TOP_EMBEDS)
        .ok_or_else(|| format!("Enter a whole number from 0 to {MAX_DGG_TOP_EMBEDS}"))
}

/// The row's notification tag, when it deviates from the primary default.
fn notification_tag(view: &StreamerConfigView) -> Option<(&'static str, &'static str)> {
    match (view.tier, view.live_notifications) {
        (StreamerTier::Background, _) => Some((
            "Background",
            "Muted notifications, all-time records only, polled every third check",
        )),
        (StreamerTier::Primary, Some(false)) => Some((
            "Live alerts off",
            "Live notifications are off for this streamer",
        )),
        _ => None,
    }
}

/// One editor save.
enum SaveRequest {
    Create(StreamerConfigCreate),
    Update(String, StreamerConfigPatch),
}

/// What the inspector edits.
#[derive(Clone, Debug, PartialEq, Eq)]
enum EditorTarget {
    New,
    Edit(String),
}

fn find(data: &Option<StreamerConfigResponse>, id: &str) -> Option<StreamerConfigView> {
    data.as_ref()?
        .streamers
        .iter()
        .find(|s| s.id == id)
        .cloned()
}

fn replace_view(data: RwSignal<Option<StreamerConfigResponse>>, view: StreamerConfigView) {
    data.try_update(|d| {
        if let Some(d) = d {
            match d.streamers.iter_mut().find(|s| s.id == view.id) {
                Some(slot) => *slot = view,
                None => d.streamers.push(view),
            }
        }
    });
}

#[component]
pub fn StreamerConfigPage() -> impl IntoView {
    let toast = use_toast();
    let data = RwSignal::new(None::<StreamerConfigResponse>);
    let load_error = RwSignal::new(None::<String>);
    let reload = RwSignal::new(0u32);
    let editor = RwSignal::new(None::<EditorTarget>);
    let list_error = RwSignal::new(None::<String>);
    let reordering = RwSignal::new(false);
    let deleting = RwSignal::new(None::<String>);

    Effect::new(move |_| {
        reload.track();
        load_error.set(None);
        spawn_scoped(async move {
            match api::fetch_streamer_config().await {
                Ok(response) => data.set(Some(response)),
                Err(e) => load_error.set(Some(e.message().to_owned())),
            }
        });
    });

    // Gate on loaded-ness only, so mutations never rebuild the page.
    let loaded = Memo::new(move |_| data.with(Option::is_some));
    let ids = Memo::new(move |_| {
        data.with(|d| {
            d.as_ref()
                .map(|d| d.streamers.iter().map(|s| s.id.clone()).collect::<Vec<_>>())
                .unwrap_or_default()
        })
    });
    let kick_configured =
        Memo::new(move |_| data.with(|d| d.as_ref().is_none_or(|d| d.kick_configured)));
    let unpolled_kick = Memo::new(move |_| {
        !kick_configured.get()
            && data.with(|d| {
                d.as_ref()
                    .is_some_and(|d| d.streamers.iter().any(|s| !s.kick.is_empty()))
            })
    });

    let on_move = Callback::new(move |(id, up): (String, bool)| {
        let Some(next) = ids.with_untracked(|ids| move_order(ids, &id, up)) else {
            return;
        };
        reordering.set(true);
        list_error.set(None);
        spawn_detached(async move {
            match api::reorder_streamer_config(next).await {
                Ok(response) => {
                    data.try_set(Some(response));
                }
                Err(e) => {
                    list_error.try_set(Some(e.message().to_owned()));
                }
            }
            reordering.try_set(false);
        });
    });
    let on_delete = Callback::new(move |id: String| {
        deleting.set(Some(id.clone()));
        list_error.set(None);
        spawn_detached(async move {
            match api::delete_streamer_config(&id).await {
                Ok(removed) => {
                    data.try_update(|d| {
                        if let Some(d) = d {
                            d.streamers.retain(|s| s.id != removed.id);
                        }
                    });
                    if editor.try_get_untracked() == Some(Some(EditorTarget::Edit(id))) {
                        editor.try_set(None);
                    }
                    toast.show(format!("Deleted {}", removed.display_name), ToastKind::Info);
                }
                Err(e) => {
                    list_error.try_set(Some(e.message().to_owned()));
                }
            }
            deleting.try_set(None);
        });
    });
    let on_edit = Callback::new(move |id: String| editor.set(Some(EditorTarget::Edit(id))));

    let lede = Signal::derive(move || {
        loaded.get().then(|| {
            let n = ids.with(Vec::len);
            format!(
                "{n} tracked. The live check picks up changes on its next run; the order here is the order on Live."
            )
        })
    });

    view! {
        <PageHead
            title="Streamers"
            lede
            actions=ViewFn::from(move || view! {
                <Button
                    variant=ButtonVariant::Primary
                    icon=Icon::Plus
                    disabled=Signal::derive(move || !loaded.get())
                    disabled_reason="Streamers are still loading"
                    on_click=Callback::new(move |_| editor.set(Some(EditorTarget::New)))
                >
                    "Add streamer"
                </Button>
            })
        />
        {move || {
            if !loaded.get() {
                return match load_error.get() {
                    Some(e) => view! {
                        <ErrorState
                            title="Could not load the streamer configuration"
                            raw=e
                            retry=Callback::new(move |()| reload.update(|n| *n += 1))
                            page=true
                        />
                    }.into_any(),
                    None => view! { <SkeletonRows count=6/> }.into_any(),
                };
            }
            view! {
                <div class="stack-lg">
                    <Panel
                        title="Tracked streamers"
                        head_end=ViewFn::from(move || view! {
                            <span class="num">{move || ids.with(Vec::len)}</span>
                        })
                    >
                        {move || unpolled_kick.get().then(|| view! {
                            <div class="panel-body">
                                <InlineNote tone=Tone::Warn>
                                    "Kick sources are saved but not polled until Kick credentials are configured on the server."
                                </InlineNote>
                            </div>
                        })}
                        {move || list_error.get().map(|e| view! {
                            <div class="panel-body">
                                <p class="field-error" role="alert">{e}</p>
                            </div>
                        })}
                        {move || if ids.with(Vec::is_empty) {
                            view! {
                                <EmptyState
                                    compact=true
                                    icon=Icon::Live
                                    message="No streamers tracked yet. Add one to get live notifications."
                                />
                            }.into_any()
                        } else {
                            view! {
                                <div class="rows streamer-config-rows">
                                    <For each=move || ids.get() key=|id| id.clone() children=move |id| view! {
                                        <StreamerRow id data ids reordering deleting on_move on_edit on_delete/>
                                    }/>
                                </div>
                            }.into_any()
                        }}
                    </Panel>
                    <DggSettings data/>
                </div>
            }.into_any()
        }}
        {move || editor.get().map(|target| view! {
            <StreamerEditor
                target
                data
                kick_configured
                on_close=Callback::new(move |()| editor.set(None))
            />
        })}
    }
}

#[component]
fn StreamerRow(
    id: String,
    data: RwSignal<Option<StreamerConfigResponse>>,
    ids: Memo<Vec<String>>,
    reordering: RwSignal<bool>,
    deleting: RwSignal<Option<String>>,
    on_move: Callback<(String, bool)>,
    on_edit: Callback<String>,
    on_delete: Callback<String>,
) -> impl IntoView {
    let key = StoredValue::new(id.clone());
    let item = Memo::new(move |_| data.with(|d| key.with_value(|id| find(d, id))));
    let position = Memo::new(move |_| {
        ids.with(|ids| {
            let index = key.with_value(|id| ids.iter().position(|i| i == id));
            (index.unwrap_or(0), ids.len())
        })
    });
    let name = Memo::new(move |_| {
        item.with(|i| {
            i.as_ref()
                .map(|i| i.display_name.clone())
                .unwrap_or_default()
        })
    });
    let is_first = Signal::derive(move || position.get().0 == 0);
    let is_last = Signal::derive(move || {
        let (index, len) = position.get();
        index + 1 >= len
    });
    let busy_delete = Signal::derive(move || {
        deleting.with(|d| key.with_value(|id| d.as_deref() == Some(id.as_str())))
    });
    let sources = move || {
        item.with(|item| {
            let Some(item) = item else { return Vec::new() };
            Platform::ALL
                .iter()
                .filter_map(|platform| {
                    let names = platform_list(item, *platform);
                    (!names.is_empty()).then(|| {
                        view! {
                            <span class="source-group">
                                <PlatformIcon platform=platform.key() size=14/>
                                <span class="mono">{names.join(", ")}</span>
                            </span>
                        }
                    })
                })
                .collect::<Vec<_>>()
        })
    };
    let tags = move || {
        item.with(|item| {
            let item = item.as_ref()?;
            let tier = notification_tag(item).map(|(label, title)| view! { <Tag title=title>{label}</Tag> });
            let token = item.has_pushover_token.then(|| view! {
                <Tag title="Notifications for this streamer use its own Pushover application">"Own Pushover app"</Tag>
            });
            Some(view! { {tier} {token} })
        })
    };
    view! {
        <div class="row streamer-config-row" id=format!("streamer-config-{id}")>
            <span class="reorder">
                <Button
                    variant=ButtonVariant::Ghost
                    size=ButtonSize::Sm
                    icon=Icon::ChevronUp
                    icon_only=true
                    aria_label=Signal::derive(move || format!("Move {} up", name.get()))
                    disabled=Signal::derive(move || is_first.get() || reordering.get())
                    disabled_reason=Signal::derive(move || if reordering.get() { "Saving the order".to_owned() } else { "Already first".to_owned() })
                    on_click=Callback::new(move |_| on_move.run((key.get_value(), true)))
                />
                <Button
                    variant=ButtonVariant::Ghost
                    size=ButtonSize::Sm
                    icon=Icon::ChevronDown
                    icon_only=true
                    aria_label=Signal::derive(move || format!("Move {} down", name.get()))
                    disabled=Signal::derive(move || is_last.get() || reordering.get())
                    disabled_reason=Signal::derive(move || if reordering.get() { "Saving the order".to_owned() } else { "Already last".to_owned() })
                    on_click=Callback::new(move |_| on_move.run((key.get_value(), false)))
                />
            </span>
            <span class="row-main">
                <span class="row-title">{move || name.get()}</span>
                <span class="row-sub streamer-sources">{sources}</span>
            </span>
            <span class="row-end">
                <span class="cluster hide-phone">{tags}</span>
                <Button
                    variant=ButtonVariant::Ghost
                    size=ButtonSize::Sm
                    icon=Icon::Pencil
                    aria_label=Signal::derive(move || format!("Edit {}", name.get()))
                    on_click=Callback::new(move |_| on_edit.run(key.get_value()))
                >
                    <span class="hide-phone">"Edit"</span>
                </Button>
                <ConfirmButton
                    label="Delete"
                    confirm_label="Delete streamer"
                    destructive=true
                    variant=ButtonVariant::Ghost
                    size=ButtonSize::Sm
                    icon=Icon::Trash
                    title="Stops tracking; sessions and records are kept, so re-adding the name resumes them"
                    busy=busy_delete
                    on_confirm=Callback::new(move |()| on_delete.run(key.get_value()))
                />
            </span>
        </div>
    }
}

#[component]
fn DggSettings(data: RwSignal<Option<StreamerConfigResponse>>) -> impl IntoView {
    let toast = use_toast();
    let saved =
        Memo::new(move |_| data.with(|d| d.as_ref().map_or(0, |d| d.settings.dgg_top_embeds)));
    let input = RwSignal::new(saved.get_untracked().to_string());
    let saving = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let parsed = Memo::new(move |_| input.with(|i| parse_top_embeds(i)));
    let unchanged = Signal::derive(move || parsed.with(|p| p.as_ref().ok() == Some(&saved.get())));
    let invalid = Signal::derive(move || parsed.with(Result::is_err));
    let save = move || {
        let Ok(value) = parsed.get_untracked() else {
            return;
        };
        saving.set(true);
        error.set(None);
        spawn_detached(async move {
            match api::save_live_settings(LiveSettings {
                dgg_top_embeds: value,
            })
            .await
            {
                Ok(settings) => {
                    data.try_update(|d| {
                        if let Some(d) = d {
                            d.settings = settings;
                        }
                    });
                    input.try_set(settings.dgg_top_embeds.to_string());
                    toast.show("Destiny.gg embeds saved", ToastKind::Info);
                }
                Err(e) => {
                    error.try_set(Some(e.message().to_owned()));
                }
            }
            saving.try_set(false);
        });
    };
    view! {
        <Panel title="Destiny.gg embeds" pad=true>
            <form
                class="field"
                on:submit=move |event| {
                    event.prevent_default();
                    save();
                }
            >
                <label class="field-label" for="dgg-top-embeds">"Top embeds to track"</label>
                <div class="cluster">
                    <input
                        id="dgg-top-embeds"
                        class="input num embeds-input"
                        type="number"
                        inputmode="numeric"
                        min="0"
                        max=MAX_DGG_TOP_EMBEDS.to_string()
                        step="1"
                        aria-invalid=move || invalid.get().then_some("true")
                        aria-describedby="dgg-top-embeds-help"
                        prop:value=move || input.get()
                        on:input=move |ev| input.set(event_target_value(&ev))
                    />
                    <Button
                        submit=true
                        busy=saving
                        disabled=Signal::derive(move || unchanged.get() || invalid.get())
                        disabled_reason=Signal::derive(move || {
                            if invalid.get() { format!("Enter 0 to {MAX_DGG_TOP_EMBEDS}") } else { "No change to save".to_owned() }
                        })
                    >
                        "Save"
                    </Button>
                </div>
                <p class="field-help" id="dgg-top-embeds-help">
                    "The current top Destiny.gg embeds are tracked as temporary streamers. 0 turns this off; at most 20."
                </p>
                {move || parsed.with(|p| p.as_ref().err().cloned()).map(|e| view! { <p class="field-error">{e}</p> })}
                {move || error.get().map(|e| view! { <p class="field-error" role="alert">{e}</p> })}
            </form>
        </Panel>
    }
}

fn tier_options() -> Vec<SegOption<StreamerTier>> {
    vec![
        SegOption::new(StreamerTier::Primary, "Primary"),
        SegOption::new(StreamerTier::Background, "Background"),
    ]
}

fn live_options() -> Vec<SegOption<Option<bool>>> {
    vec![
        SegOption::new(None, "Default (on)"),
        SegOption::new(Some(true), "On"),
        SegOption::new(Some(false), "Off"),
    ]
}

#[component]
fn StreamerEditor(
    target: EditorTarget,
    data: RwSignal<Option<StreamerConfigResponse>>,
    kick_configured: Memo<bool>,
    on_close: Callback<()>,
) -> impl IntoView {
    let toast = use_toast();
    let editing = match &target {
        EditorTarget::Edit(id) => Some(id.clone()),
        EditorTarget::New => None,
    };
    let original = editing
        .as_deref()
        .and_then(|id| data.with_untracked(|d| find(d, id)));
    let title = match &original {
        Some(o) => format!("Edit {}", o.display_name),
        None => "Add streamer".to_owned(),
    };
    let draft = RwSignal::new(
        original
            .as_ref()
            .map(StreamerDraft::from_view)
            .unwrap_or_default(),
    );
    let pending: [RwSignal<String>; 3] = std::array::from_fn(|_| RwSignal::new(String::new()));
    let source_error = RwSignal::new(None::<String>);
    let new_token = RwSignal::new(String::new());
    let attempted = RwSignal::new(false);
    let saving = RwSignal::new(false);
    let save_error = RwSignal::new(None::<String>);
    let editing = StoredValue::new(editing);

    let problems = Memo::new(move |_| {
        draft.with(|draft| {
            data.with(|d| {
                let streamers = d
                    .as_ref()
                    .map(|d| d.streamers.as_slice())
                    .unwrap_or_default();
                editing.with_value(|id| draft_problems(draft, streamers, id.as_deref()))
            })
        })
    });
    let shown = Memo::new(move |_| {
        if attempted.get() {
            problems.get()
        } else {
            DraftProblems::default()
        }
    });

    let add_pending = move |platform: Platform| -> bool {
        let slot = pending[platform.index()];
        let text = slot.get_untracked();
        if text.trim().is_empty() {
            return true;
        }
        match draft.with_untracked(|d| add_usernames(d.usernames(platform), &text)) {
            Ok(next) => {
                draft.update(|d| *d.usernames_mut(platform) = next);
                slot.set(String::new());
                source_error.set(None);
                true
            }
            Err(e) => {
                source_error.set(Some(format!("{}: {e}", platform.label())));
                false
            }
        }
    };

    let save = move || {
        if saving.get_untracked() || !Platform::ALL.into_iter().all(add_pending) {
            return;
        }
        attempted.set(true);
        if !problems.get_untracked().is_empty() {
            return;
        }
        let token = new_token.get_untracked();
        if editing.with_value(Option::is_none)
            && let Some(problem) = (!token.trim().is_empty())
                .then(|| token_problem(token.trim()))
                .flatten()
        {
            save_error.set(Some(problem));
            return;
        }
        save_error.set(None);
        let current = draft.get_untracked();
        let request = match editing.get_value() {
            None => SaveRequest::Create(build_create(&current, &token)),
            Some(id) => {
                let Some(original) = data.with_untracked(|d| find(d, &id)) else {
                    save_error.set(Some("This streamer no longer exists".to_owned()));
                    return;
                };
                let patch = build_patch(&original, &current);
                if patch == StreamerConfigPatch::default() {
                    on_close.run(());
                    return;
                }
                SaveRequest::Update(id, patch)
            }
        };
        saving.set(true);
        spawn_detached(async move {
            let (verb, result) = match &request {
                SaveRequest::Create(create) => ("Added", api::create_streamer_config(create).await),
                SaveRequest::Update(id, patch) => {
                    ("Saved", api::update_streamer_config(id, patch).await)
                }
            };
            saving.try_set(false);
            match result {
                Ok(view) => {
                    toast.show(format!("{verb} {}", view.display_name), ToastKind::Info);
                    replace_view(data, view);
                    on_close.try_run(());
                }
                Err(e) => {
                    save_error.try_set(Some(e.message().to_owned()));
                }
            }
        });
    };

    let actions = ViewFn::from(move || {
        view! {
            <Button
                variant=ButtonVariant::Primary
                icon=Icon::Check
                busy=saving
                on_click=Callback::new(move |_| save())
            >
                {if editing.with_value(Option::is_some) { "Save changes" } else { "Add streamer" }}
            </Button>
            <Button variant=ButtonVariant::Ghost on_click=Callback::new(move |_| on_close.run(()))>"Cancel"</Button>
        }
    });

    let has_original = original.is_some();
    view! {
        <Inspector title=title actions on_close>
            <form
                class="streamer-editor"
                on:submit=move |event| {
                    event.prevent_default();
                    save();
                }
            >
                <div class="inspector-section field">
                    <label class="field-label" for="streamer-name">"Display name"</label>
                    <input
                        id="streamer-name"
                        class="input"
                        type="text"
                        maxlength=MAX_NAME_CHARS.to_string()
                        autocomplete="off"
                        aria-invalid=move || shown.with(|p| p.name.is_some()).then_some("true")
                        prop:value=move || draft.with(|d| d.display_name.clone())
                        on:input=move |ev| draft.update(|d| d.display_name = event_target_value(&ev))
                    />
                    {move || shown.with(|p| p.name.clone()).map(|e| view! { <p class="field-error">{e}</p> })}
                    {has_original.then(|| view! {
                        <p class="field-help">"Renaming keeps the streamer's id, history and records."</p>
                    })}
                </div>
                <div class="inspector-section">
                    <h3>"Sources"</h3>
                    <div class="stack">
                        {Platform::ALL.into_iter().map(|platform| view! {
                            <SourceList platform draft pending=pending[platform.index()] add_pending kick_configured/>
                        }).collect_view()}
                    </div>
                    {move || source_error.get().map(|e| view! { <p class="field-error" role="alert">{e}</p> })}
                    {move || shown.with(|p| p.sources.clone()).map(|e| view! { <p class="field-error">{e}</p> })}
                </div>
                <div class="inspector-section stack">
                    <h3>"Notifications"</h3>
                    <div class="field">
                        <span class="field-label">"Tier"</span>
                        <Segmented
                            options=Signal::derive(tier_options)
                            value=Signal::derive(move || draft.with(|d| d.tier))
                            on_change=Callback::new(move |tier| draft.update(|d| d.set_tier(tier)))
                            aria_label="Tier"
                            small=true
                        />
                        <p class="field-help">
                            {move || match draft.with(|d| d.tier) {
                                StreamerTier::Primary => "Live, offline and title notifications; 7, 30 and 90-day viewer records.",
                                StreamerTier::Background => "Muted: no live, offline or title notifications, all-time viewer records only, polled every third check.",
                            }}
                        </p>
                    </div>
                    {move || (draft.with(|d| d.tier) == StreamerTier::Primary).then(|| view! {
                        <div class="field">
                            <span class="field-label">"Live notifications"</span>
                            <Segmented
                                options=Signal::derive(live_options)
                                value=Signal::derive(move || draft.with(|d| d.live_notifications))
                                on_change=Callback::new(move |value| draft.update(|d| d.live_notifications = value))
                                aria_label="Live notifications"
                                small=true
                            />
                        </div>
                    })}
                </div>
                {(!has_original).then(|| view! {
                    <div class="inspector-section field">
                        <label class="field-label" for="streamer-new-token">"Pushover application token (optional)"</label>
                        <input
                            id="streamer-new-token"
                            class="input mono"
                            type="password"
                            autocomplete="new-password"
                            spellcheck="false"
                            maxlength=MAX_TOKEN_CHARS.to_string()
                            placeholder="Uses the default Omni app"
                            prop:value=move || new_token.get()
                            on:input=move |ev| new_token.set(event_target_value(&ev))
                        />
                        <p class="field-help">"Send this streamer's notifications through its own Pushover app. Write-only: it is never shown again."</p>
                    </div>
                })}
                {move || save_error.get().map(|e| view! {
                    <div class="inspector-section">
                        <p class="field-error" role="alert">{e}</p>
                    </div>
                })}
                // Enter in a text field submits through this hidden button.
                <button type="submit" class="sr-only" tabindex="-1" aria-hidden="true"></button>
            </form>
            {editing.get_value().map(|id| view! { <TokenSection id data/> })}
        </Inspector>
    }
}

#[component]
fn SourceList(
    platform: Platform,
    draft: RwSignal<StreamerDraft>,
    pending: RwSignal<String>,
    add_pending: impl Fn(Platform) -> bool + Copy + Send + Sync + 'static,
    kick_configured: Memo<bool>,
) -> impl IntoView {
    let input_id = format!("streamer-source-{}", platform.key());
    let names = Memo::new(move |_| draft.with(|d| d.usernames(platform).to_vec()));
    let full = Signal::derive(move || names.with(Vec::len) >= MAX_PER_PLATFORM);
    view! {
        <div class="field source-list">
            <label class="field-label source-label" for=input_id.clone()>
                <PlatformIcon platform=platform.key() size=14/>
                {platform.label()}
                <span class="num dim">{move || format!("{}/{MAX_PER_PLATFORM}", names.with(Vec::len))}</span>
            </label>
            {move || (!names.with(Vec::is_empty)).then(|| view! {
                <div class="chips">
                    <For each=move || names.get() key=|name| name.clone() children=move |name| {
                        let label = format!("Remove {name}");
                        let target = name.clone();
                        view! {
                            <span class="chip removable">
                                <span class="mono">{name}</span>
                                <button
                                    type="button"
                                    class="chip-x"
                                    aria-label=label.clone()
                                    title=label
                                    on:click=move |_| draft.update(|d| d.usernames_mut(platform).retain(|n| *n != target))
                                >
                                    "×"
                                </button>
                            </span>
                        }
                    }/>
                </div>
            })}
            <div class="cluster source-add">
                <input
                    id=input_id
                    class="input mono"
                    type="text"
                    autocomplete="off"
                    autocapitalize="off"
                    spellcheck="false"
                    placeholder=platform.placeholder()
                    disabled=move || full.get()
                    title=move || full.get().then(|| format!("At most {MAX_PER_PLATFORM} usernames per platform"))
                    prop:value=move || pending.get()
                    on:input=move |ev| pending.set(event_target_value(&ev))
                    on:keydown=move |ev: web_sys::KeyboardEvent| {
                        if ev.key() == "Enter" {
                            ev.prevent_default();
                            add_pending(platform);
                        }
                    }
                />
                <Button
                    size=ButtonSize::Sm
                    icon=Icon::Plus
                    disabled=Signal::derive(move || full.get() || pending.with(|p| p.trim().is_empty()))
                    disabled_reason=Signal::derive(move || {
                        if full.get() { format!("At most {MAX_PER_PLATFORM} usernames per platform") } else { "Type a username first".to_owned() }
                    })
                    on_click=Callback::new(move |_| { add_pending(platform); })
                >
                    "Add"
                </Button>
            </div>
            {move || (platform == Platform::Kick && !kick_configured.get()).then(|| view! {
                <p class="field-help warn">"Saved, but not polled until Kick credentials are configured on the server."</p>
            })}
        </div>
    }
}

/// Set, replace or remove an existing streamer's Pushover token. Each action
/// saves on its own; the token is never read back.
#[component]
fn TokenSection(id: String, data: RwSignal<Option<StreamerConfigResponse>>) -> impl IntoView {
    let toast = use_toast();
    let key = StoredValue::new(id);
    let has_token = Memo::new(move |_| {
        data.with(|d| {
            key.with_value(|id| find(d, id))
                .is_some_and(|s| s.has_pushover_token)
        })
    });
    let token = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let problem = Memo::new(move |_| {
        token.with(|t| {
            let t = t.trim();
            (!t.is_empty()).then(|| token_problem(t)).flatten()
        })
    });
    let send = move |value: String, done: &'static str| {
        busy.set(true);
        error.set(None);
        let id = key.get_value();
        spawn_detached(async move {
            let patch = StreamerConfigPatch {
                pushover_token: Some(value),
                ..StreamerConfigPatch::default()
            };
            match api::update_streamer_config(&id, &patch).await {
                Ok(view) => {
                    replace_view(data, view);
                    token.try_set(String::new());
                    toast.show(done, ToastKind::Info);
                }
                Err(e) => {
                    error.try_set(Some(e.message().to_owned()));
                }
            }
            busy.try_set(false);
        });
    };
    view! {
        <form
            class="inspector-section field"
            on:submit=move |event| {
                event.prevent_default();
                let value = token.get_untracked().trim().to_owned();
                if !value.is_empty() && problem.get_untracked().is_none() {
                    send(value, "Pushover token saved");
                }
            }
        >
            <h3>"Pushover app"</h3>
            <p class="small dim">
                {move || if has_token.get() {
                    "This streamer's notifications use its own Pushover app. The token is write-only and never shown."
                } else {
                    "Notifications use the default Omni app."
                }}
            </p>
            <label class="sr-only" for="streamer-token">"Pushover application token"</label>
            <div class="cluster token-row">
                <input
                    id="streamer-token"
                    class="input mono"
                    type="password"
                    autocomplete="new-password"
                    spellcheck="false"
                    maxlength=MAX_TOKEN_CHARS.to_string()
                    placeholder=move || if has_token.get() { "New application token" } else { "Application token" }
                    aria-invalid=move || problem.with(Option::is_some).then_some("true")
                    prop:value=move || token.get()
                    on:input=move |ev| token.set(event_target_value(&ev))
                />
                <Button
                    submit=true
                    busy=busy
                    disabled=Signal::derive(move || token.with(|t| t.trim().is_empty()) || problem.with(Option::is_some))
                    disabled_reason="Paste a valid application token first"
                >
                    {move || if has_token.get() { "Replace" } else { "Set" }}
                </Button>
                {move || has_token.get().then(|| view! {
                    <ConfirmButton
                        label="Remove"
                        confirm_label="Remove token"
                        variant=ButtonVariant::Ghost
                        busy=busy
                        title="Go back to the default Omni app"
                        on_confirm=Callback::new(move |()| send(String::new(), "Pushover token removed"))
                    />
                })}
            </div>
            {move || problem.get().map(|e| view! { <p class="field-error">{e}</p> })}
            {move || error.get().map(|e| view! { <p class="field-error" role="alert">{e}</p> })}
        </form>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(id: &str, name: &str) -> StreamerConfigView {
        StreamerConfigView {
            id: id.into(),
            display_name: name.into(),
            youtube: vec!["@destiny".into()],
            twitch: vec!["destiny".into()],
            kick: Vec::new(),
            tier: StreamerTier::Primary,
            live_notifications: None,
            has_pushover_token: false,
        }
    }

    #[test]
    fn unchanged_form_builds_an_empty_patch() {
        let original = view("destiny", "Destiny");
        let mut draft = StreamerDraft::from_view(&original);
        draft.display_name = "  Destiny ".into();
        assert_eq!(
            build_patch(&original, &draft),
            StreamerConfigPatch::default()
        );
    }

    #[test]
    fn patch_carries_only_changed_fields() {
        let original = view("destiny", "Destiny");
        let mut draft = StreamerDraft::from_view(&original);
        draft.display_name = "Steven ".into();
        draft.kick.push("destiny".into());
        draft.live_notifications = Some(false);
        let patch = build_patch(&original, &draft);
        assert_eq!(
            patch,
            StreamerConfigPatch {
                display_name: Some("Steven".into()),
                kick: Some(vec!["destiny".into()]),
                live_notifications: Some(Some(false)),
                ..StreamerConfigPatch::default()
            }
        );
        let json = serde_json::to_value(&patch).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"displayName": "Steven", "kick": ["destiny"], "liveNotifications": false})
        );
    }

    #[test]
    fn clearing_an_override_sends_null() {
        let mut original = view("destiny", "Destiny");
        original.live_notifications = Some(false);
        let mut draft = StreamerDraft::from_view(&original);
        draft.live_notifications = None;
        let patch = build_patch(&original, &draft);
        assert_eq!(patch.live_notifications, Some(None));
        assert_eq!(
            serde_json::to_value(&patch).unwrap(),
            serde_json::json!({"liveNotifications": null})
        );
    }

    #[test]
    fn switching_to_background_drops_the_override() {
        let mut original = view("destiny", "Destiny");
        original.live_notifications = Some(true);
        let mut draft = StreamerDraft::from_view(&original);
        draft.set_tier(StreamerTier::Background);
        assert_eq!(draft.live_notifications, None);
        let patch = build_patch(&original, &draft);
        assert_eq!(patch.tier, Some(StreamerTier::Background));
        assert_eq!(patch.live_notifications, None);
    }

    #[test]
    fn create_trims_and_omits_an_empty_token() {
        let draft = StreamerDraft {
            display_name: " Jerma ".into(),
            twitch: vec!["jerma985".into()],
            tier: StreamerTier::Background,
            live_notifications: Some(true),
            ..StreamerDraft::default()
        };
        let create = build_create(&draft, "  ");
        assert_eq!(create.display_name, "Jerma");
        assert_eq!(create.pushover_token, None);
        assert_eq!(create.live_notifications, None);
        assert_eq!(
            build_create(&draft, " abc123 ").pushover_token.as_deref(),
            Some("abc123")
        );
    }

    #[test]
    fn usernames_split_dedupe_and_cap() {
        let list = vec!["destiny".to_owned()];
        assert_eq!(
            add_usernames(&list, "DESTINY, destinyclips  other"),
            Ok(vec![
                "destiny".to_owned(),
                "destinyclips".to_owned(),
                "other".to_owned()
            ])
        );
        let full: Vec<String> = (0..MAX_PER_PLATFORM).map(|i| format!("n{i}")).collect();
        assert!(add_usernames(&full, "n0").is_ok());
        assert!(add_usernames(&full, "extra").is_err());
        assert!(add_usernames(&list, &"x".repeat(MAX_NAME_CHARS + 1)).is_err());
    }

    #[test]
    fn problems_flag_names_and_sources_owned_elsewhere() {
        let streamers = [view("destiny", "Destiny"), view("jerma", "Jerma")];
        let mut draft = StreamerDraft {
            display_name: "jerma".into(),
            twitch: vec!["DESTINY".into()],
            ..StreamerDraft::default()
        };
        let problems = draft_problems(&draft, &streamers, None);
        assert!(problems.name.is_some());
        assert_eq!(
            problems.sources.as_deref(),
            Some("Twitch DESTINY already belongs to Destiny")
        );
        // Editing a streamer does not conflict with itself.
        draft.display_name = "Destiny".into();
        assert!(draft_problems(&draft, &streamers[..1], Some("destiny")).is_empty());
        let empty = StreamerDraft {
            display_name: "New".into(),
            ..StreamerDraft::default()
        };
        assert!(draft_problems(&empty, &streamers, None).sources.is_some());
        assert!(
            draft_problems(&StreamerDraft::default(), &[], None)
                .name
                .is_some()
        );
    }

    #[test]
    fn tokens_are_alphanumeric_and_bounded() {
        assert_eq!(token_problem("azGePB1234"), None);
        assert!(token_problem("abc-123").is_some());
        assert!(token_problem(&"a".repeat(MAX_TOKEN_CHARS + 1)).is_some());
    }

    #[test]
    fn moves_stop_at_the_ends() {
        let ids: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
        assert_eq!(
            move_order(&ids, "b", true),
            Some(["b", "a", "c"].map(String::from).to_vec())
        );
        assert_eq!(
            move_order(&ids, "b", false),
            Some(["a", "c", "b"].map(String::from).to_vec())
        );
        assert_eq!(move_order(&ids, "a", true), None);
        assert_eq!(move_order(&ids, "c", false), None);
        assert_eq!(move_order(&ids, "x", true), None);
    }

    #[test]
    fn top_embeds_are_bounded() {
        assert_eq!(parse_top_embeds(" 3 "), Ok(3));
        assert_eq!(parse_top_embeds("0"), Ok(0));
        assert_eq!(parse_top_embeds("20"), Ok(20));
        assert!(parse_top_embeds("21").is_err());
        assert!(parse_top_embeds("-1").is_err());
        assert!(parse_top_embeds("").is_err());
    }
}
