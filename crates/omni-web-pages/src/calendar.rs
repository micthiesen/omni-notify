//! Calendar (`/calendar`): a read-only agenda of the primary iCloud calendar
//! for today and the next seven days, with its sync health. Also the compact
//! Home agenda and the sync facts in the Operations inspector.

use leptos::prelude::*;
use omni_api::calendar::{CalendarOccurrence, CalendarStatusResponse};
use omni_web_kit::api;
use omni_web_kit::components::{
    EmptyState, ErrorState, Icon, PageHead, Panel, Skeleton, SkeletonRows, Status, StatusKind, Tag,
    Tone,
};
use omni_web_kit::feeds::{TaskBacked, use_task_backed};
use omni_web_kit::hooks::{use_now, use_query_highlight};
use omni_web_kit::router::Link;
use omni_web_kit::task::spawn_scoped;
use omni_web_kit::utils::days::{day_label, parse_ymd, ymd};
use omni_web_kit::utils::format::{format_absolute, format_clock_time, format_relative_at};
use omni_web_kit::utils::js::{local_day_index, local_time_parts, parse_date_ms};

/// The task that keeps the mirror current.
pub const CALENDAR_TASK: &str = "CalendarPrimarySync";
/// Today plus the next seven days.
pub const AGENDA_DAYS: i64 = 8;
/// The background sync tolerates a five-minute-old mirror; twice that is
/// stale.
const STALE_SYNC_MS: f64 = 10.0 * 60_000.0;

/// One occurrence placed on local days.
#[derive(Clone, Debug, PartialEq)]
pub struct AgendaItem {
    pub key: String,
    pub title: String,
    pub all_day: bool,
    pub start_ms: f64,
    pub end_ms: f64,
    pub start_day: i64,
    /// Inclusive.
    pub last_day: i64,
    /// Local minutes after midnight (timed events).
    pub start_min: i64,
    pub end_min: i64,
    pub location: Option<String>,
    pub cancelled: bool,
    pub tentative: bool,
    pub free: bool,
    pub recurring: bool,
}

/// Places an occurrence on local days. `day_of` and `minute_of` map an
/// instant to its local day index and minutes after midnight.
pub fn to_item(
    o: &CalendarOccurrence,
    day_of: &dyn Fn(f64) -> i64,
    minute_of: &dyn Fn(f64) -> i64,
) -> Option<AgendaItem> {
    let start_ms = parse_date_ms(&o.start_utc)?;
    let end_ms = parse_date_ms(&o.end_utc).unwrap_or(start_ms);
    let (start_day, last_day) = if o.all_day {
        let start = parse_ymd(&o.start)?;
        let last = o
            .last_date
            .as_deref()
            .and_then(parse_ymd)
            .or_else(|| parse_ymd(&o.end).map(|end| end - 1))
            .unwrap_or(start)
            .max(start);
        (start, last)
    } else {
        let start = day_of(start_ms);
        let last = if end_ms > start_ms {
            day_of(end_ms - 1.0)
        } else {
            start
        };
        (start, last.max(start))
    };
    let status = o.status.as_deref().unwrap_or_default().to_ascii_uppercase();
    Some(AgendaItem {
        key: format!(
            "{}:{}",
            o.event_id,
            o.recurrence_id.clone().unwrap_or_default()
        ),
        title: if o.title.trim().is_empty() {
            "Untitled event".to_owned()
        } else {
            o.title.trim().to_owned()
        },
        all_day: o.all_day,
        start_ms,
        end_ms,
        start_day,
        last_day,
        start_min: minute_of(start_ms),
        end_min: minute_of(end_ms),
        location: o
            .location
            .clone()
            .map(|l| l.trim().replace('\n', ", "))
            .filter(|l| !l.is_empty()),
        cancelled: status == "CANCELLED",
        tentative: status == "TENTATIVE",
        free: o.free,
        recurring: o.recurring,
    })
}

fn clock(min: i64) -> String {
    format_clock_time(min.div_euclid(60) % 24, min.rem_euclid(60))
}

/// One line of a day.
#[derive(Clone, Debug, PartialEq)]
pub struct AgendaEntry {
    pub item: AgendaItem,
    /// `9:30 AM – 10:00 AM`, `All day`, `Until 11:00 AM`, `9:00 PM →`.
    pub when: String,
    /// Happening now.
    pub now: bool,
    /// Already over.
    pub past: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgendaDay {
    pub day: i64,
    pub entries: Vec<AgendaEntry>,
}

fn when_on(item: &AgendaItem, day: i64) -> String {
    if item.all_day {
        let span = item.last_day - item.start_day + 1;
        return if span > 1 {
            format!("All day · {} of {span}", day - item.start_day + 1)
        } else {
            "All day".to_owned()
        };
    }
    let starts = day == item.start_day;
    let ends = day == item.last_day;
    match (starts, ends) {
        (true, true) if item.end_ms > item.start_ms => {
            format!("{} – {}", clock(item.start_min), clock(item.end_min))
        }
        (true, true) => clock(item.start_min),
        (true, false) => format!("{} →", clock(item.start_min)),
        (false, true) => format!("Until {}", clock(item.end_min)),
        (false, false) => "All day".to_owned(),
    }
}

/// Today (always, possibly empty) and every later day of the window that has
/// something on it; all-day items first, then by start.
pub fn agenda(items: &[AgendaItem], today: i64, days: i64, now: f64) -> Vec<AgendaDay> {
    (today..today + days)
        .filter_map(|day| {
            let mut on: Vec<&AgendaItem> = items
                .iter()
                .filter(|i| i.start_day <= day && day <= i.last_day)
                .collect();
            on.sort_by(|a, b| {
                (!a.all_day, a.start_ms, &a.title)
                    .partial_cmp(&(!b.all_day, b.start_ms, &b.title))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            if on.is_empty() && day != today {
                return None;
            }
            Some(AgendaDay {
                day,
                entries: on
                    .into_iter()
                    .map(|item| AgendaEntry {
                        when: when_on(item, day),
                        now: !item.all_day && item.start_ms <= now && now < item.end_ms,
                        past: !item.all_day && item.end_ms <= now,
                        item: item.clone(),
                    })
                    .collect(),
            })
        })
        .collect()
}

/// The next timed event that has not started, for the headline.
pub fn next_up(days: &[AgendaDay], now: f64) -> Option<&AgendaEntry> {
    days.iter()
        .flat_map(|d| d.entries.iter())
        .filter(|e| !e.item.all_day && !e.item.cancelled && e.item.start_ms > now)
        .min_by(|a, b| a.item.start_ms.total_cmp(&b.item.start_ms))
}

/// Headline and lede for the page.
pub fn agenda_sentence(days: &[AgendaDay], today: i64, now: f64) -> (String, String) {
    // A multi-day event counts once: on today if it covers today.
    let keys = |d: &AgendaDay| -> Vec<String> {
        d.entries
            .iter()
            .filter(|e| !e.item.cancelled)
            .map(|e| e.item.key.clone())
            .collect()
    };
    let on_today: std::collections::HashSet<String> = days
        .iter()
        .filter(|d| d.day == today)
        .flat_map(keys)
        .collect();
    let all: std::collections::HashSet<String> = days.iter().flat_map(keys).collect();
    let today_count = on_today.len();
    let week = all.len();
    let head = match today_count {
        0 => "Nothing on today.".to_owned(),
        1 => "1 event today.".to_owned(),
        n => format!("{n} events today."),
    };
    let later = week - today_count;
    let rest = match later {
        0 => "nothing else this week".to_owned(),
        1 => "1 more this week".to_owned(),
        n => format!("{n} more this week"),
    };
    let lede = match next_up(days, now) {
        Some(e) => {
            let at = clock(e.item.start_min);
            let when = match e.item.start_day - today {
                0 => format!("at {at}"),
                1 => format!("tomorrow at {at}"),
                _ => format!("{} at {at}", day_label(e.item.start_day, today)),
            };
            format!("Next: {} {when}, {rest}.", e.item.title)
        }
        None => {
            let mut r = rest;
            if let Some(first) = r.get(..1) {
                r = format!("{}{}", first.to_uppercase(), &r[1..]);
            }
            format!("{r}.")
        }
    };
    (head, lede)
}

/// Sync health: status, short label and the server's message.
pub fn sync_health(s: &CalendarStatusResponse, now: f64) -> (StatusKind, String, Option<String>) {
    if !s.configured || s.state == "not_configured" {
        return (
            StatusKind::Idle,
            "Not configured".to_owned(),
            s.message.clone(),
        );
    }
    match s.state.as_str() {
        "identity_error" => (
            StatusKind::Fault,
            "Calendar not found".to_owned(),
            s.message.clone(),
        ),
        "sync_error" => (
            StatusKind::Fault,
            "Sync failing".to_owned(),
            s.message.clone(),
        ),
        "ready" => match s.last_sync_at.map(|t| t as f64) {
            None => (StatusKind::Idle, "Not synced yet".to_owned(), None),
            Some(at) if now - at > STALE_SYNC_MS => (
                StatusKind::Stale,
                format!("Last synced {}", format_relative_at(at, now)),
                s.message.clone(),
            ),
            Some(at) => (
                StatusKind::Ok,
                format!("Synced {}", format_relative_at(at, now)),
                None,
            ),
        },
        other => (StatusKind::Warn, other.replace('_', " "), s.message.clone()),
    }
}

/// A calendar problem worth a "Needs you" line.
pub fn sync_needs_you(s: &CalendarStatusResponse, now: f64) -> bool {
    matches!(
        sync_health(s, now).0,
        StatusKind::Fault | StatusKind::Stale | StatusKind::Warn
    )
}

fn browser_day(ms: f64) -> i64 {
    local_day_index(ms)
}

fn browser_minute(ms: f64) -> i64 {
    let (h, m, _, _) = local_time_parts(ms);
    i64::from(h) * 60 + i64::from(m)
}

/// The calendar status (refreshed with every sync run) and the agenda,
/// refetched when the change cursor moves or the day turns over.
#[derive(Clone, Copy)]
struct CalendarData {
    status: TaskBacked<CalendarStatusResponse>,
    items: RwSignal<Option<Vec<AgendaItem>>>,
    truncated: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    today: Memo<i64>,
    now: ReadSignal<f64>,
}

/// The calendar status, reloaded with every sync run.
pub fn use_calendar_status() -> TaskBacked<CalendarStatusResponse> {
    use_task_backed(CALENDAR_TASK, api::fetch_calendar_status)
}

fn use_calendar(status: Option<TaskBacked<CalendarStatusResponse>>) -> CalendarData {
    let status = status.unwrap_or_else(use_calendar_status);
    let now = use_now(30_000);
    let today = Memo::new(move |_| local_day_index(now.get()));
    let items = RwSignal::new(None::<Vec<AgendaItem>>);
    let truncated = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let key = Memo::new(move |_| {
        status.data.with(|s| {
            s.as_ref()
                .filter(|s| s.configured)
                .map(|s| (s.change_cursor, s.last_full_sync_at))
        })
    });
    Effect::new(move |_| {
        let Some(_) = key.get() else { return };
        let day = today.get();
        spawn_scoped(async move {
            match api::fetch_calendar_events(&ymd(day), &ymd(day + AGENDA_DAYS)).await {
                Ok(res) => {
                    let list = res
                        .events
                        .iter()
                        .filter_map(|o| to_item(o, &browser_day, &browser_minute))
                        .collect();
                    items.set(Some(list));
                    truncated.set(res.truncated);
                    error.set(None);
                }
                Err(e) => error.set(Some(e.message().to_owned())),
            }
        });
    });
    CalendarData {
        status,
        items,
        truncated,
        error,
        today,
        now,
    }
}

#[component]
fn AgendaRows(day: AgendaDay, #[prop(optional)] compact: bool) -> impl IntoView {
    view! {
        <div class=if compact { "rows dense" } else { "rows" }>
            {day.entries.into_iter().map(|e| {
                let mut class = String::from("row agenda-row");
                if e.now { class.push_str(" now"); }
                if e.past { class.push_str(" past"); }
                if e.item.cancelled { class.push_str(" cancelled"); }
                let title_attr = if e.item.all_day {
                    e.item.title.clone()
                } else {
                    format!("{} · {}", e.item.title, format_absolute(e.item.start_ms))
                };
                view! {
                    <div class=class title=title_attr>
                        <span class="agenda-when num">{e.when}</span>
                        <span class="row-main">
                            <span class="row-title">{e.item.title.clone()}</span>
                            {(!compact).then(|| e.item.location.clone()).flatten().map(|l| view! { <span class="row-sub">{l}</span> })}
                        </span>
                        <span class="row-end">
                            {e.now.then(|| view! { <Tag tone=Tone::Signal>"Now"</Tag> })}
                            {e.item.cancelled.then(|| view! { <Tag tone=Tone::Warn>"Cancelled"</Tag> })}
                            {(e.item.tentative && !compact).then(|| view! { <Tag>"Tentative"</Tag> })}
                            {(e.item.free && !compact).then(|| view! { <Tag>"Free"</Tag> })}
                            {(e.item.recurring && !compact).then(|| view! { <Tag>"Repeats"</Tag> })}
                        </span>
                    </div>
                }
            }).collect_view()}
        </div>
    }
}

/// Sync facts as a key-value list (page side panel and Operations).
#[component]
pub fn CalendarSyncFacts(
    #[prop(optional)] status: Option<TaskBacked<CalendarStatusResponse>>,
) -> impl IntoView {
    let status = status.unwrap_or_else(use_calendar_status);
    let now = use_now(30_000);
    move || {
        if let Some(e) = status
            .error
            .get()
            .filter(|_| status.data.with(Option::is_none))
        {
            return view! { <ErrorState title="Calendar status could not load" raw=e retry=Callback::new(move |()| status.reload())/> }.into_any();
        }
        let Some(s) = status.data.get() else {
            return view! { <SkeletonRows count=3/> }.into_any();
        };
        let (kind, label, message) = sync_health(&s, now.get());
        let flag = |v: Option<bool>| match v {
            Some(true) => "yes",
            Some(false) => "no",
            None => "—",
        };
        view! {
            <dl class="kv">
                <dt>"Sync"</dt>
                <dd><Status kind label=label/></dd>
                {message.map(|m| view! { <dt>"Detail"</dt><dd class=if kind == StatusKind::Fault { "text-fault" } else { "text-warn" }>{m}</dd> })}
                <dt>"Calendar"</dt>
                <dd>{s.calendar_name.clone()} <span class="mono off">" " {s.default_time_zone.clone()}</span></dd>
                {s.last_full_sync_at.map(|t| view! {
                    <dt>"Full sync"</dt>
                    <dd class="num" title=format_absolute(t as f64)>{format_relative_at(t as f64, now.get())}</dd>
                })}
                <dt>"Events"</dt>
                <dd class="num">{format!("{} mirrored · change #{}", s.event_count, s.change_cursor)}</dd>
                <dt>"Writable"</dt>
                <dd>{flag(s.writable)}</dd>
                <dt>"Email events"</dt>
                <dd>{match s.pipeline_targets_primary { Some(true) => "Added to this calendar", Some(false) => "Added to another calendar", None => "—" }}</dd>
            </dl>
        }
        .into_any()
    }
}

#[component]
pub fn CalendarPage() -> impl IntoView {
    let cal = use_calendar(None);
    let loaded = Signal::derive(move || cal.items.with(Option::is_some));
    let highlighted = use_query_highlight("day", "day", loaded);
    let days = Memo::new(move |_| {
        cal.items.with(|i| {
            i.as_ref()
                .map(|i| agenda(i, cal.today.get(), AGENDA_DAYS, cal.now.get()))
        })
    });
    let status = cal.status;
    // Gate on the page's shape only; the parts below track the clock and the
    // status reloads themselves, so neither rebuilds the page.
    let state = Memo::new(
        move |_| match status.data.with(|s| s.as_ref().map(|s| s.configured)) {
            Some(true) => CalendarPageState::Loaded,
            Some(false) => CalendarPageState::Unconfigured,
            None => match status.error.get() {
                Some(e) => CalendarPageState::Error(e),
                None => CalendarPageState::Loading,
            },
        },
    );
    let health = Memo::new(move |_| {
        status.data.with(|s| {
            s.as_ref()
                .map(|s| (sync_health(s, cal.now.get()), s.calendar_name.clone()))
        })
    });
    let sentence = Memo::new(move |_| {
        days.with(|d| {
            d.as_ref()
                .map(|d| agenda_sentence(d, cal.today.get(), cal.now.get()))
        })
        .unwrap_or_else(|| ("Calendar".to_owned(), String::new()))
    });
    let head = Signal::derive(move || sentence.get().0);
    let lede = Signal::derive(move || Some(sentence.get().1));

    move || {
        match state.get() {
            CalendarPageState::Error(e) => {
                return view! {
                    <ErrorState title="The calendar could not load" raw=e retry=Callback::new(move |()| status.reload()) page=true/>
                }
                .into_any();
            }
            CalendarPageState::Loading => {
                return view! {
                    <div class="stack-lg" aria-busy="true">
                        <PageHead title="Calendar"/>
                        <SkeletonRows count=5/>
                    </div>
                }
                .into_any();
            }
            CalendarPageState::Unconfigured => {
                return view! {
                    <PageHead title="Calendar"/>
                    <Panel>
                        <EmptyState
                            icon=Icon::Calendar
                            title="The calendar isn't connected"
                            message="No iCloud CalDAV credentials are configured on the server. Once they are, the primary calendar's next eight days show here."
                        />
                    </Panel>
                }
                .into_any();
            }
            CalendarPageState::Loaded => {}
        }
        let sync_line = move || {
            health.get().map(|((kind, label, _), name)| {
                view! {
                    <p class="page-status">
                        <Status kind label=label/>
                        <span class="small muted">{format!("{name} · read-only")}</span>
                    </p>
                }
            })
        };
        let problem = move || {
            let ((kind, label, message), _) = health.get()?;
            matches!(
                kind,
                StatusKind::Fault | StatusKind::Stale | StatusKind::Warn
            )
            .then(|| {
                view! {
                    <ErrorState
                        warn=kind != StatusKind::Fault
                        title=format!("{label}: the agenda may be out of date")
                        detail="It shows the last mirrored copy of the calendar."
                        raw=message.unwrap_or_default()
                        link=("Operations".to_owned(), format!("/operations#inspect={CALENDAR_TASK}"))
                    />
                }
            })
        };
        let hl = highlighted.clone();
        view! {
            <PageHead title=head lede=lede sentence=true>
                {sync_line}
            </PageHead>
            {problem}
            <div class="split calendar-layout">
                <Panel title="Next 8 days" refreshing=status.refreshing class="agenda">
                    {move || {
                        if let Some(e) = cal.error.get().filter(|_| cal.items.with(Option::is_none)) {
                            return view! { <ErrorState title="Events could not load" raw=e/> }.into_any();
                        }
                        let Some(days) = days.get() else {
                            return view! { <SkeletonRows count=5/> }.into_any();
                        };
                        let today = cal.today.get();
                        let quiet_week = days.iter().all(|d| d.entries.is_empty());
                        view! {
                            {days.into_iter().map(|day| {
                                let id = format!("day-{}", ymd(day.day));
                                let is_hl = hl.as_deref() == Some(&id[4..]);
                                let count = day.entries.len();
                                let d = day.day;
                                let label = day_label(d, today);
                                let aria = label.clone();
                                view! {
                                    <section class=if is_hl { "agenda-day deep-link-target" } else { "agenda-day" } id=id aria-label=aria>
                                        <h3 class="group-head">
                                            <span class=if d == today { "text-signal" } else { "" }>{label}</span>
                                            {(d <= today + 1).then(|| view! { <span class="meta">{omni_web_kit::utils::days::short_date(d, today)}</span> })}
                                            <span class="meta num end">{if count == 0 { String::new() } else { count.to_string() }}</span>
                                        </h3>
                                        {if count == 0 {
                                            view! { <p class="agenda-empty small muted">"Nothing scheduled."</p> }.into_any()
                                        } else {
                                            view! { <AgendaRows day/> }.into_any()
                                        }}
                                    </section>
                                }
                            }).collect_view()}
                            {quiet_week.then(|| view! { <p class="agenda-empty small muted">"Nothing else in the next seven days."</p> })}
                            {move || cal.truncated.get().then(|| view! { <p class="inline-note warn">"Some events were left out; the window holds more than 1,000."</p> })}
                        }
                        .into_any()
                    }}
                </Panel>
                <div class="sticky-side">
                    <Panel title="Sync" pad=true>
                        <CalendarSyncFacts status/>
                    </Panel>
                </div>
            </div>
        }
        .into_any()
    }
}

/// What the calendar page shows.
#[derive(Clone, Debug, PartialEq)]
enum CalendarPageState {
    Error(String),
    Loading,
    Unconfigured,
    Loaded,
}

/// Home: today and the next days at a glance (at most six lines). Renders
/// nothing while the calendar is not configured.
#[component]
pub fn AgendaTile(
    /// Shared with the caller's own status reads.
    #[prop(optional)]
    status: Option<TaskBacked<CalendarStatusResponse>>,
) -> impl IntoView {
    let cal = use_calendar(status);
    move || {
        let s = cal.status.data.get()?;
        if !s.configured {
            return None;
        }
        let (kind, label, _) = sync_health(&s, cal.now.get());
        let today = cal.today.get();
        let body = match cal.items.get() {
            None => view! { <div class="panel-body"><Skeleton width="70%"/></div> }.into_any(),
            Some(items) => {
                let mut budget = 6usize;
                let days: Vec<AgendaDay> = agenda(&items, today, AGENDA_DAYS, cal.now.get())
                    .into_iter()
                    .filter_map(|mut d| {
                        if budget == 0 {
                            return None;
                        }
                        // Today's finished events drop off the tile.
                        d.entries.retain(|e| !e.past);
                        if d.entries.is_empty() && d.day != today {
                            return None;
                        }
                        d.entries.truncate(budget);
                        budget -= d.entries.len();
                        Some(d)
                    })
                    .collect();
                view! {
                    {days.into_iter().map(|day| {
                        let href = format!("/calendar?day={}", ymd(day.day));
                        let empty = day.entries.is_empty();
                        view! {
                            <Link to=href class="group-head agenda-tile-head">
                                <span class=if day.day == today { "text-signal" } else { "" }>{day_label(day.day, today)}</span>
                            </Link>
                            {if empty {
                                view! { <p class="agenda-empty small muted">"Nothing else today."</p> }.into_any()
                            } else {
                                view! { <AgendaRows day compact=true/> }.into_any()
                            }}
                        }
                    }).collect_view()}
                }
                .into_any()
            }
        };
        Some(view! {
            <Panel
                title="Agenda"
                class="agenda"
                head_end=ViewFn::from(move || view! {
                    {(kind != StatusKind::Ok).then(|| view! { <Status kind label=label.clone()/> })}
                    <Link to="/calendar" class="textlink small">"Calendar"</Link>
                })
            >
                {body}
            </Panel>
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: f64 = 60_000.0;

    fn occurrence(title: &str, start: &str, end: &str, all_day: bool) -> CalendarOccurrence {
        let utc = |t: &str| {
            if all_day {
                format!("{t}T07:00:00.000Z")
            } else {
                format!("{t}.000Z")
            }
        };
        CalendarOccurrence {
            event_id: title.into(),
            recurrence_id: None,
            title: title.into(),
            start: start.into(),
            end: end.into(),
            start_utc: utc(start),
            end_utc: utc(end),
            all_day,
            last_date: None,
            time_zone: None,
            location: Some("  Clinic\nMain St ".into()),
            recurring: false,
            is_exception: false,
            scheduling_role: "none".into(),
            free: false,
            status: None,
        }
    }

    fn item(o: &CalendarOccurrence) -> AgendaItem {
        // Native tests run in UTC.
        to_item(o, &local_day_index, &|ms| {
            ((ms / MIN) as i64).rem_euclid(1440)
        })
        .unwrap()
    }

    #[test]
    fn items_land_on_their_days() {
        let today = parse_ymd("2026-10-09").unwrap();
        let dentist = item(&occurrence(
            "Dentist",
            "2026-10-09T14:30:00",
            "2026-10-09T15:00:00",
            false,
        ));
        assert_eq!((dentist.start_day, dentist.last_day), (today, today));
        assert_eq!(dentist.location.as_deref(), Some("Clinic, Main St"));
        let trip = item(&occurrence("Trip", "2026-10-10", "2026-10-13", true));
        assert_eq!((trip.start_day, trip.last_day), (today + 1, today + 3));
        let late = item(&occurrence(
            "Show",
            "2026-10-11T22:00:00",
            "2026-10-12T01:00:00",
            false,
        ));
        assert_eq!((late.start_day, late.last_day), (today + 2, today + 3));

        let now = parse_date_ms("2026-10-09T14:40:00.000Z").unwrap();
        let days = agenda(
            &[late.clone(), trip.clone(), dentist.clone()],
            today,
            AGENDA_DAYS,
            now,
        );
        let shape: Vec<(i64, Vec<String>)> = days
            .iter()
            .map(|d| {
                (
                    d.day - today,
                    d.entries.iter().map(|e| e.when.clone()).collect(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            [
                (0, vec!["2:30 PM – 3:00 PM".to_owned()]),
                (1, vec!["All day · 1 of 3".to_owned()]),
                (
                    2,
                    vec!["All day · 2 of 3".to_owned(), "10:00 PM →".to_owned()]
                ),
                (
                    3,
                    vec!["All day · 3 of 3".to_owned(), "Until 1:00 AM".to_owned()]
                ),
            ]
        );
        assert!(days[0].entries[0].now);
        assert!(!days[0].entries[0].past);
    }

    #[test]
    fn sentences_name_the_next_event() {
        let today = parse_ymd("2026-10-09").unwrap();
        let dentist = item(&occurrence(
            "Dentist",
            "2026-10-09T14:30:00",
            "2026-10-09T15:00:00",
            false,
        ));
        let lunch = item(&occurrence(
            "Lunch",
            "2026-10-10T12:00:00",
            "2026-10-10T13:00:00",
            false,
        ));
        let morning = parse_date_ms("2026-10-09T09:00:00.000Z").unwrap();
        let days = agenda(
            &[dentist.clone(), lunch.clone()],
            today,
            AGENDA_DAYS,
            morning,
        );
        assert_eq!(
            agenda_sentence(&days, today, morning),
            (
                "1 event today.".into(),
                "Next: Dentist at 2:30 PM, 1 more this week.".into()
            )
        );
        let evening = parse_date_ms("2026-10-09T20:00:00.000Z").unwrap();
        let days = agenda(&[dentist, lunch], today, AGENDA_DAYS, evening);
        assert_eq!(
            agenda_sentence(&days, today, evening).1,
            "Next: Lunch tomorrow at 12:00 PM, 1 more this week."
        );
        let trip = item(&occurrence("Trip", "2026-10-10", "2026-10-13", true));
        let days = agenda(&[trip], today, AGENDA_DAYS, evening);
        assert_eq!(
            agenda_sentence(&days, today, evening).1,
            "1 more this week."
        );
        let empty = agenda(&[], today, AGENDA_DAYS, evening);
        assert_eq!(empty.len(), 1);
        assert_eq!(
            agenda_sentence(&empty, today, evening),
            ("Nothing on today.".into(), "Nothing else this week.".into())
        );
    }

    #[test]
    fn sync_health_reads_state_and_age() {
        let mut s = CalendarStatusResponse {
            configured: true,
            state: "ready".into(),
            message: None,
            calendar_name: "iCloud".into(),
            is_server_default: None,
            pipeline_targets_primary: None,
            writable: None,
            supports_sync: None,
            last_sync_at: Some(0),
            last_full_sync_at: None,
            event_count: 3,
            change_cursor: 1,
            default_time_zone: "America/Vancouver".into(),
        };
        assert_eq!(sync_health(&s, 2.0 * MIN).0, StatusKind::Ok);
        assert_eq!(sync_health(&s, 2.0 * MIN).1, "Synced 2m ago");
        assert_eq!(sync_health(&s, 25.0 * MIN).0, StatusKind::Stale);
        assert!(sync_needs_you(&s, 25.0 * MIN));
        s.state = "sync_error".into();
        assert_eq!(sync_health(&s, 0.0).0, StatusKind::Fault);
        s.configured = false;
        assert_eq!(sync_health(&s, 0.0).0, StatusKind::Idle);
        assert!(!sync_needs_you(&s, 0.0));
    }
}
