//! Deliveries (`/deliveries`): Omni's cached Parcel read. Active deliveries
//! first with their latest carrier event, delivered ones collapsed, and the
//! cache's age, backoff and last error. Also the compact Home tile.

use leptos::prelude::*;
use omni_api::common::encode_uri_component;
use omni_api::parcels::{ParcelDelivery, ParcelDeliveryStatus, ParcelEvent, ParcelsResponse};
use omni_web_kit::api;
use omni_web_kit::components::{
    Disclosure, EmptyState, ErrorState, Glyph, Icon, IconSize, PageHead, Panel, SkeletonRows,
    Status, StatusKind, Tag, Tone,
};
use omni_web_kit::feeds::{TaskBacked, use_task_backed};
use omni_web_kit::hooks::{use_now, use_query_highlight};
use omni_web_kit::router::Link;
use omni_web_kit::utils::days::{day_label, parse_ymd, short_date};
use omni_web_kit::utils::format::{format_absolute, format_clock_time, format_relative_at};
use omni_web_kit::utils::js::{local_day_index, now_ms};

/// The task that refreshes the cache.
pub const PARCEL_TASK: &str = "ParcelDeliveries";
const MIN_MS: f64 = 60_000.0;

/// Status shape and word. Progress is neutral; only what needs you or went
/// wrong takes a hue.
pub fn status_look(status: ParcelDeliveryStatus) -> (StatusKind, &'static str) {
    let kind = match status {
        ParcelDeliveryStatus::Completed => StatusKind::Ok,
        ParcelDeliveryStatus::OutForDelivery => StatusKind::Info,
        ParcelDeliveryStatus::AwaitingPickup => StatusKind::Warn,
        ParcelDeliveryStatus::FailedAttempt | ParcelDeliveryStatus::Exception => StatusKind::Fault,
        ParcelDeliveryStatus::Frozen | ParcelDeliveryStatus::NotFound => StatusKind::Stale,
        ParcelDeliveryStatus::InTransit
        | ParcelDeliveryStatus::InfoReceived
        | ParcelDeliveryStatus::Unknown => StatusKind::Idle,
    };
    (kind, status.label())
}

/// A pickup, failed attempt or exception waits on you.
pub fn needs_you(status: ParcelDeliveryStatus) -> bool {
    matches!(
        status,
        ParcelDeliveryStatus::AwaitingPickup
            | ParcelDeliveryStatus::FailedAttempt
            | ParcelDeliveryStatus::Exception
    )
}

/// `(hour, minute)` of a Parcel `YYYY-MM-DD HH:MM[:SS]` text; `None` at
/// midnight, which Parcel uses for "date only".
fn clock(text: &str) -> Option<(i64, i64)> {
    let time = text.get(11..16)?;
    let (h, m) = time.split_once(':')?;
    let (h, m): (i64, i64) = (h.parse().ok()?, m.parse().ok()?);
    (h != 0 || m != 0).then_some((h, m))
}

/// Only a parcel still moving can be late: one waiting for pickup, out for
/// delivery or after a failed attempt is past its estimate by nature.
pub fn can_be_late(d: &ParcelDelivery) -> bool {
    d.active && !needs_you(d.status) && d.status != ParcelDeliveryStatus::OutForDelivery
}

/// The expected delivery as words, and whether an active delivery is past it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expected {
    pub text: String,
    pub late: bool,
}

/// `active` is [`can_be_late`].
pub fn expected_label(
    expected: Option<&str>,
    expected_end: Option<&str>,
    active: bool,
    today: i64,
) -> Option<Expected> {
    let start_text = expected?;
    let start = parse_ymd(start_text)?;
    let end = expected_end.and_then(parse_ymd).filter(|e| *e > start);
    let last = end.unwrap_or(start);
    if last < today {
        // A past estimate only matters while the parcel is still moving.
        return active.then(|| Expected {
            text: format!("Was due {}", short_date(last, today)),
            late: true,
        });
    }
    let text = match end {
        Some(end) if end - today <= 1 || start - today <= 1 => {
            format!("{}–{}", day_label(start, today), day_label(end, today))
        }
        Some(end) => format!("{} – {}", short_date(start, today), short_date(end, today)),
        None => {
            let day = day_label(start, today);
            match (
                clock(start_text),
                expected_end
                    .filter(|e| parse_ymd(e) == Some(start))
                    .and_then(clock),
            ) {
                (Some((h1, m1)), Some((h2, m2))) => format!(
                    "{day} · {}–{}",
                    format_clock_time(h1, m1),
                    format_clock_time(h2, m2)
                ),
                (Some((h, m)), None) => format!("{day} · {}", format_clock_time(h, m)),
                _ => day,
            }
        }
    };
    Some(Expected { text, late: false })
}

/// A carrier event's date in words (`Yesterday 11:31 AM`), or the carrier's
/// own text when it is not `YYYY-MM-DD…`; `None` for placeholders.
pub fn event_when(date: Option<&str>, today: i64) -> Option<String> {
    let date = date?.trim();
    match parse_ymd(date) {
        Some(day) => Some(match clock(date) {
            Some((h, m)) => format!("{} {}", day_label(day, today), format_clock_time(h, m)),
            None => day_label(day, today),
        }),
        None => date
            .chars()
            .any(|c| c.is_ascii_digit())
            .then(|| date.to_owned()),
    }
}

/// How current the cached read is.
#[derive(Clone, Debug, PartialEq)]
pub enum Freshness {
    /// No `PARCEL_API_KEY`: nothing is read.
    Unconfigured,
    /// Configured but no read has succeeded yet.
    Waiting,
    Fresh {
        fetched_at: f64,
    },
    /// Older than twice the read interval.
    Stale {
        fetched_at: f64,
    },
    /// Parcel answered 429; no reads until `until`.
    Backoff {
        until: f64,
        fetched_at: Option<f64>,
    },
    /// The last read failed.
    Failed {
        message: String,
        fetched_at: Option<f64>,
    },
}

/// Reads happen every 30 minutes while anything is active, every 3 hours
/// otherwise.
pub fn read_interval_ms(active_count: u32) -> f64 {
    if active_count > 0 {
        30.0 * MIN_MS
    } else {
        180.0 * MIN_MS
    }
}

pub fn freshness(res: &ParcelsResponse, now: f64) -> Freshness {
    if !res.configured {
        return Freshness::Unconfigured;
    }
    let fetched_at = res.fetched_at.map(|t| t as f64);
    if let Some(until) = res.backoff_until.map(|t| t as f64).filter(|u| *u > now) {
        return Freshness::Backoff { until, fetched_at };
    }
    if let Some(message) = res.last_error.clone() {
        return Freshness::Failed {
            message,
            fetched_at,
        };
    }
    match fetched_at {
        None => Freshness::Waiting,
        Some(at) if now - at > 2.0 * read_interval_ms(res.active_count) => {
            Freshness::Stale { fetched_at: at }
        }
        Some(at) => Freshness::Fresh { fetched_at: at },
    }
}

/// `(tone, text)` for a panel header: `updated 12m ago`, in warn when stale.
pub fn freshness_note(f: &Freshness, now: f64) -> Option<(Tone, String)> {
    let ago = |at: f64| format!("updated {}", format_relative_at(at, now));
    match f {
        Freshness::Unconfigured => None,
        Freshness::Waiting => Some((Tone::Neutral, "waiting for the first read".to_owned())),
        Freshness::Fresh { fetched_at } => Some((Tone::Neutral, ago(*fetched_at))),
        Freshness::Stale { fetched_at } => Some((Tone::Warn, ago(*fetched_at))),
        Freshness::Backoff { until, .. } => Some((
            Tone::Warn,
            format!(
                "rate limited · next read {}",
                format_relative_at(*until, now)
            ),
        )),
        Freshness::Failed { fetched_at, .. } => Some((
            Tone::Warn,
            fetched_at.map_or_else(|| "last read failed".to_owned(), ago),
        )),
    }
}

/// Whether an active parcel is past its estimate (shown as "Was due …").
pub fn is_late(d: &ParcelDelivery, today: i64) -> bool {
    expected_label(
        d.expected.as_deref(),
        d.expected_end.as_deref(),
        can_be_late(d),
        today,
    )
    .is_some_and(|e| e.late)
}

/// Headline and lede for the page. Late parcels are counted so the sentence
/// agrees with their warn "Was due" dates.
pub fn deliveries_sentence(res: &ParcelsResponse, today_index: i64) -> (String, String) {
    if !res.configured {
        return (
            "Delivery tracking is off.".to_owned(),
            "Set PARCEL_API_KEY on the server to follow deliveries from Parcel.".to_owned(),
        );
    }
    let active: Vec<&ParcelDelivery> = res.deliveries.iter().filter(|d| d.active).collect();
    let delivered = res.deliveries.len() - active.len();
    let today = active
        .iter()
        .filter(|d| d.status == ParcelDeliveryStatus::OutForDelivery)
        .count();
    let waiting = active.iter().filter(|d| needs_you(d.status)).count();
    let late = active.iter().filter(|d| is_late(d, today_index)).count();
    let head = match (active.len(), waiting, today) {
        (0, ..) => "Nothing on the way.".to_owned(),
        (_, 1, _) => "1 delivery needs you.".to_owned(),
        (_, w, _) if w > 1 => format!("{w} deliveries need you."),
        _ if late == 1 => "1 delivery is late.".to_owned(),
        _ if late > 1 => format!("{late} deliveries are late."),
        (_, _, t) if t > 0 => format!("{t} arriving today."),
        (1, ..) => "1 delivery on the way.".to_owned(),
        (n, ..) => format!("{n} deliveries on the way."),
    };
    let mut parts = Vec::new();
    if !active.is_empty() && (waiting > 0 || today > 0 || late > 0) {
        parts.push(format!("{} active", active.len()));
    }
    if today > 0 && (waiting > 0 || late > 0) {
        parts.push(format!("{today} out for delivery"));
    }
    if late > 0 && waiting > 0 {
        parts.push(format!("{late} late"));
    }
    parts.push(match delivered {
        0 => "nothing delivered recently".to_owned(),
        1 => "1 delivered recently".to_owned(),
        n => format!("{n} delivered recently"),
    });
    let mut lede = parts.join(", ");
    if let Some(first) = lede.get(..1) {
        lede = format!("{}{}.", first.to_uppercase(), &lede[1..]);
    }
    (head, lede)
}

fn latest_event(d: &ParcelDelivery) -> Option<&ParcelEvent> {
    d.events.first()
}

fn title_of(d: &ParcelDelivery) -> String {
    let description = d.description.trim();
    if description.is_empty() {
        d.carrier_name
            .clone()
            .map_or_else(|| "Package".to_owned(), |c| format!("{c} package"))
    } else {
        description.to_owned()
    }
}

fn carrier_of(d: &ParcelDelivery) -> String {
    d.carrier_name
        .clone()
        .unwrap_or_else(|| d.carrier_code.to_uppercase())
}

fn event_line(event: &ParcelEvent, today: i64) -> String {
    let mut parts = vec![event.description.trim().to_owned()];
    if let Some(location) = event
        .location
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty())
    {
        parts.push(location.to_owned());
    }
    if let Some(when) = event_when(event.date.as_deref(), today) {
        parts.push(when);
    }
    parts.join(" · ")
}

/// One delivery: status, title, carrier and tracking number, the latest
/// event, the expected date and a disclosure with the full event history.
#[component]
fn DeliveryItem(delivery: ParcelDelivery, today: i64, highlighted: bool) -> impl IntoView {
    let (kind, word) = status_look(delivery.status);
    let expected = expected_label(
        delivery.expected.as_deref(),
        delivery.expected_end.as_deref(),
        can_be_late(&delivery),
        today,
    );
    let latest = latest_event(&delivery).map(|e| event_line(e, today));
    let extra = delivery
        .extra_information
        .clone()
        .filter(|e| !e.trim().is_empty());
    let id = format!("delivery-{}", delivery.tracking_number);
    let class = if highlighted {
        "delivery deep-link-target"
    } else {
        "delivery"
    };
    let events = delivery.events.clone();
    let event_count = delivery.event_count.max(events.len() as u32);
    let source = delivery.source.clone();
    let summary = match event_count {
        0 => "No carrier events yet".to_owned(),
        1 => "Tracking history · 1 event".to_owned(),
        n => format!("Tracking history · {n} events"),
    };
    view! {
        <article class=class id=id>
            <div class="row delivery-row">
                <Status kind label=word dot_only=kind == StatusKind::Ok/>
                <span class="row-main">
                    <span class="row-title">{title_of(&delivery)}</span>
                    <span class="row-sub">
                        {carrier_of(&delivery)} " · "
                        <span class="mono">{delivery.tracking_number.clone()}</span>
                    </span>
                    {latest.map(|l| view! { <span class="delivery-latest">{l}</span> })}
                    {extra.map(|e| view! { <span class="row-sub">{e}</span> })}
                </span>
                <span class="row-end">
                    {expected.map(|e| view! {
                        <span class=if e.late { "delivery-eta num text-warn" } else { "delivery-eta num" }>{e.text}</span>
                    })}
                </span>
            </div>
            {(event_count > 0 || source.is_some()).then(|| view! {
                <Disclosure summary=summary flush=false class="delivery-history">
                    <ol class="event-rail">
                        {events.into_iter().map(|e| {
                            let when = event_when(e.date.as_deref(), today);
                            let place = e.location.clone().filter(|l| !l.trim().is_empty());
                            view! {
                                <li>
                                    <span class="event-what">{e.description.clone()}</span>
                                    <span class="event-meta">
                                        {when.map(|w| view! { <time class="num">{w}</time> })}
                                        {place.map(|p| view! { <span>{p}</span> })}
                                    </span>
                                    {e.additional.clone().filter(|a| !a.trim().is_empty()).map(|a| view! { <span class="event-meta">{a}</span> })}
                                </li>
                            }
                        }).collect_view()}
                    </ol>
                    {(event_count as usize > delivery.events.len()).then(|| view! {
                        <p class="small muted">{format!("{} older events are not shown.", event_count as usize - delivery.events.len())}</p>
                    })}
                    {source.map(|s| view! {
                        <p class="small muted">
                            "Added from an email on " {format_absolute(s.submitted_at as f64)} " · "
                            <Link to=format!("/emails#inspect={}", encode_uri_component(&s.activity_id)) class="textlink">"Source email"</Link>
                        </p>
                    })}
                </Disclosure>
            })}
        </article>
    }
}

/// The cache note for a panel header.
#[component]
fn FreshnessMeta(res: Signal<Option<ParcelsResponse>>) -> impl IntoView {
    let now = use_now(30_000);
    move || {
        res.with(|r| r.as_ref().map(|r| freshness(r, now.get())))
            .and_then(|f| freshness_note(&f, now.get()))
            .map(|(tone, text)| {
                view! { <span class=format!("num {}", if tone == Tone::Warn { "text-warn" } else { "" })>{text}</span> }
            })
    }
}

/// The cached Parcel read, reloaded whenever the read task runs.
pub fn use_parcels() -> TaskBacked<ParcelsResponse> {
    use_task_backed(PARCEL_TASK, api::fetch_parcels)
}

#[component]
pub fn DeliveriesPage() -> impl IntoView {
    let parcels = use_parcels();
    let now = use_now(30_000);
    let today = Memo::new(move |_| local_day_index(now.get()));
    let loaded = Signal::derive(move || parcels.data.with(Option::is_some));
    let highlighted = use_query_highlight("tracking", "delivery", loaded);
    let data: Signal<Option<ParcelsResponse>> = parcels.data.into();
    let sentence = Memo::new(move |_| {
        parcels
            .data
            .with(|d| d.as_ref().map(|d| deliveries_sentence(d, today.get())))
    });

    // Gate on the page's shape and narrow each part to what it shows: a
    // reload that only moves the read timestamps must not rebuild the lists
    // (and close every opened tracking history).
    let state = Memo::new(
        move |_| match parcels.data.with(|d| d.as_ref().map(|r| r.configured)) {
            Some(true) => PageState::Loaded,
            Some(false) => PageState::Unconfigured,
            None => match parcels.error.get() {
                Some(e) => PageState::Error(e),
                None => PageState::Loading,
            },
        },
    );
    let fresh = Memo::new(move |_| {
        parcels
            .data
            .with(|d| d.as_ref().map(|r| freshness(r, now.get_untracked())))
    });
    let deliveries = Memo::new(move |_| {
        parcels
            .data
            .with(|d| d.as_ref().map(|r| r.deliveries.clone()).unwrap_or_default())
    });
    let head = Signal::derive(move || sentence.get().unwrap_or_default().0);
    let lede = Signal::derive(move || sentence.get().map(|(_, lede)| lede));

    let notice = move || {
        match fresh.get()? {
        Freshness::Backoff { until, .. } => Some(view! {
            <ErrorState
                warn=true
                title="Parcel is rate limiting Omni"
                detail=format!("No reads until {}. The list below is the last good read.", format_absolute(until))
            />
        }.into_any()),
        Freshness::Failed { message, .. } => Some(view! {
            <ErrorState
                warn=true
                title="The last Parcel read failed"
                detail="The list below is the last good read; the next scheduled read retries."
                raw=message
                link=("Operations".to_owned(), format!("/operations#inspect={PARCEL_TASK}"))
            />
        }.into_any()),
        _ => None,
    }
    };

    let lists = move || {
        let today = today.get();
        let (active, delivered): (Vec<ParcelDelivery>, Vec<ParcelDelivery>) =
            deliveries.get().into_iter().partition(|d| d.active);
        let mark = |list: Vec<ParcelDelivery>| -> Vec<(ParcelDelivery, bool)> {
            list.into_iter()
                .map(|d| {
                    let hit = highlighted.as_deref() == Some(d.tracking_number.as_str());
                    (d, hit)
                })
                .collect()
        };
        let (active, delivered) = (mark(active), mark(delivered));
        // A deep link to a delivered parcel opens the history.
        let open_history = delivered.iter().any(|(_, hit)| *hit);
        let delivered_count = delivered.len();
        view! {
            <div class="stack-lg">
                <Panel
                    title=format!("On the way · {}", active.len())
                    refreshing=parcels.refreshing
                    head_end=ViewFn::from(move || view! { <FreshnessMeta res=data/> })
                    class="deliveries"
                >
                    {if active.is_empty() {
                        view! { <EmptyState compact=true icon=Icon::Package message="Nothing on the way. New tracking numbers from email show up here."/> }.into_any()
                    } else {
                        view! {
                            <div class="rows">
                                {active.into_iter().map(|(d, highlighted)| {
                                    view! { <DeliveryItem delivery=d today highlighted/> }
                                }).collect_view()}
                            </div>
                        }.into_any()
                    }}
                </Panel>
                {(delivered_count > 0).then(|| view! {
                    <Panel class="deliveries">
                        <Disclosure
                            summary=format!("Delivered · {delivered_count}")
                            meta="Kept until Parcel drops them"
                            open=open_history
                        >
                            <div class="rows">
                                {delivered.into_iter().map(|(d, highlighted)| {
                                    view! { <DeliveryItem delivery=d today highlighted/> }
                                }).collect_view()}
                            </div>
                        </Disclosure>
                    </Panel>
                })}
            </div>
        }
    };

    move || {
        match state.get() {
        PageState::Error(e) => view! {
            <ErrorState
                title="Deliveries could not load"
                raw=e
                retry=Callback::new(move |()| parcels.reload())
                page=true
            />
        }
        .into_any(),
        PageState::Loading => view! {
            <div class="stack-lg" aria-busy="true">
                <PageHead title="Deliveries"/>
                <SkeletonRows count=4/>
            </div>
        }
        .into_any(),
        PageState::Unconfigured => view! {
            <PageHead title="Deliveries"/>
            <Panel>
                <EmptyState
                    icon=Icon::Package
                    title="Delivery tracking is off"
                    message="Set PARCEL_API_KEY on the server. Tracking numbers found in email are then followed here."
                />
            </Panel>
        }
        .into_any(),
        PageState::Loaded => view! {
            <PageHead title=head lede=lede sentence=true/>
            {notice}
            {lists.clone()}
        }
        .into_any(),
    }
    }
}

/// What the deliveries page shows.
#[derive(Clone, Debug, PartialEq)]
enum PageState {
    Error(String),
    Loading,
    Unconfigured,
    Loaded,
}

/// Active deliveries ordered for a glance: needs-you first, then out for
/// delivery, then the rest in Parcel's order.
pub fn tile_order(deliveries: &[ParcelDelivery]) -> Vec<&ParcelDelivery> {
    let mut active: Vec<&ParcelDelivery> = deliveries.iter().filter(|d| d.active).collect();
    active.sort_by_key(|d| {
        if needs_you(d.status) {
            0
        } else if d.status == ParcelDeliveryStatus::OutForDelivery {
            1
        } else {
            2
        }
    });
    active
}

/// Home: active deliveries at a glance; renders nothing while nothing is on
/// the way (or tracking is off).
#[component]
pub fn DeliveriesTile(
    /// Shared with the caller's own reads.
    #[prop(optional)]
    parcels: Option<TaskBacked<ParcelsResponse>>,
) -> impl IntoView {
    let parcels = parcels.unwrap_or_else(use_parcels);
    let data: Signal<Option<ParcelsResponse>> = parcels.data.into();
    move || {
        let res = parcels.data.get()?;
        let today = local_day_index(now_ms());
        let list: Vec<ParcelDelivery> = tile_order(&res.deliveries)
            .into_iter()
            .take(4)
            .cloned()
            .collect();
        if list.is_empty() {
            return None;
        }
        let more = res.active_count as usize - list.len().min(res.active_count as usize);
        Some(view! {
            <Panel
                title="Deliveries"
                head_end=ViewFn::from(move || view! { <FreshnessMeta res=data/> <Link to="/deliveries" class="textlink small">"All"</Link> })
            >
                <div class="rows">
                    {list.into_iter().map(|d| {
                        let (kind, word) = status_look(d.status);
                        let eta = expected_label(d.expected.as_deref(), d.expected_end.as_deref(), can_be_late(&d), today);
                        view! {
                            <Link to=format!("/deliveries?tracking={}", encode_uri_component(&d.tracking_number)) class="row dense">
                                <span class="row-main">
                                    <span class="row-title truncate">{title_of(&d)}</span>
                                    <span class="row-sub"><Status kind label=word/></span>
                                </span>
                                {eta.map(|e| view! {
                                    <span class=if e.late { "row-end num small text-warn" } else { "row-end num small" }>{e.text}</span>
                                })}
                            </Link>
                        }
                    }).collect_view()}
                    {(more > 0).then(|| view! {
                        <Link to="/deliveries" class="row dense">
                            <span class="row-main"><span class="row-title dim">{format!("{more} more on the way")}</span></span>
                            <span class="row-end"><Glyph icon=Icon::ChevronRight size=IconSize::Small/></span>
                        </Link>
                    })}
                </div>
            </Panel>
        })
    }
}

/// Deliveries waiting on you, for Home's attention list.
pub fn attention_count(res: &ParcelsResponse) -> usize {
    res.deliveries
        .iter()
        .filter(|d| d.active && needs_you(d.status))
        .count()
}

/// Tag shown on the Operations inspector for the parcel task.
#[component]
pub fn ParcelCacheFacts() -> impl IntoView {
    let parcels = use_parcels();
    let now = use_now(30_000);
    move || {
        parcels.data.get().map(|res| {
            let fresh = freshness(&res, now.get());
            let (tone, note) = freshness_note(&fresh, now.get())
                .unwrap_or((Tone::Neutral, "tracking off".to_owned()));
            view! {
                <dl class="kv">
                    <dt>"Cache"</dt>
                    <dd><Tag tone>{note}</Tag></dd>
                    <dt>"Deliveries"</dt>
                    <dd class="num">{format!("{} active · {} total", res.active_count, res.deliveries.len())}</dd>
                    {res.next_read_after.map(|t| view! {
                        <dt>"Next read"</dt>
                        <dd class="num">{format!("after {}", format_absolute(t as f64))}</dd>
                    })}
                    {res.last_error.map(|e| view! { <dt>"Last error"</dt><dd class="text-warn">{e}</dd> })}
                </dl>
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delivery(status: ParcelDeliveryStatus) -> ParcelDelivery {
        ParcelDelivery {
            tracking_number: "T1".into(),
            carrier_code: "ups".into(),
            carrier_name: Some("UPS".into()),
            description: "Parts".into(),
            status,
            status_code: 2,
            active: status.is_active(),
            expected: None,
            expected_end: None,
            extra_information: None,
            events: Vec::new(),
            event_count: 0,
            source: None,
        }
    }

    fn response(deliveries: Vec<ParcelDelivery>) -> ParcelsResponse {
        ParcelsResponse {
            configured: true,
            fetched_at: Some(1_000_000),
            last_attempt_at: Some(1_000_000),
            next_read_after: None,
            backoff_until: None,
            last_error: None,
            active_count: deliveries.iter().filter(|d| d.active).count() as u32,
            deliveries,
        }
    }

    #[test]
    fn expected_dates_read_in_days() {
        let today = parse_ymd("2026-10-09").unwrap();
        let label = |a: Option<&str>, b: Option<&str>, active| {
            expected_label(a, b, active, today).map(|e| (e.text, e.late))
        };
        assert_eq!(
            label(Some("2026-10-09 00:00:00"), None, true),
            Some(("Today".into(), false))
        );
        assert_eq!(
            label(
                Some("2026-10-09 14:00:00"),
                Some("2026-10-09 20:00:00"),
                true
            ),
            Some(("Today · 2:00 PM–8:00 PM".into(), false))
        );
        assert_eq!(
            label(Some("2026-10-10"), Some("2026-10-12"), true),
            Some(("Tomorrow–Mon, Oct 12".into(), false))
        );
        assert_eq!(
            label(Some("2026-10-14"), Some("2026-10-16"), true),
            Some(("Oct 14 – Oct 16".into(), false))
        );
        assert_eq!(
            label(Some("2026-10-07"), None, true),
            Some(("Was due Oct 7".into(), true))
        );
        assert_eq!(label(Some("2026-10-07"), None, false), None);
        assert_eq!(label(None, None, true), None);
    }

    #[test]
    fn event_dates_tolerate_carrier_formats() {
        let today = parse_ymd("2026-10-09").unwrap();
        assert_eq!(
            event_when(Some("2026-10-08 11:31:05"), today).as_deref(),
            Some("Yesterday 11:31 AM")
        );
        assert_eq!(
            event_when(Some("07.10.2026 15:44"), today).as_deref(),
            Some("07.10.2026 15:44")
        );
        assert_eq!(event_when(Some("--//--"), today), None);
        assert_eq!(event_when(None, today), None);
    }

    #[test]
    fn freshness_flags_stale_backoff_and_errors() {
        let base = response(vec![delivery(ParcelDeliveryStatus::InTransit)]);
        let fetched = 1_000_000.0;
        assert_eq!(
            freshness(&base, fetched + 10.0 * MIN_MS),
            Freshness::Fresh {
                fetched_at: fetched
            }
        );
        assert_eq!(
            freshness(&base, fetched + 61.0 * MIN_MS),
            Freshness::Stale {
                fetched_at: fetched
            }
        );
        let idle = response(vec![delivery(ParcelDeliveryStatus::Completed)]);
        assert!(matches!(
            freshness(&idle, fetched + 61.0 * MIN_MS),
            Freshness::Fresh { .. }
        ));
        let mut limited = base.clone();
        limited.backoff_until = Some(5_000_000);
        assert!(matches!(
            freshness(&limited, fetched),
            Freshness::Backoff { .. }
        ));
        assert!(matches!(
            freshness(&limited, 6_000_000.0),
            Freshness::Stale { .. }
        ));
        let mut failed = base.clone();
        failed.last_error = Some("503".into());
        assert!(matches!(
            freshness(&failed, fetched),
            Freshness::Failed { .. }
        ));
        let mut off = base;
        off.configured = false;
        assert_eq!(freshness(&off, fetched), Freshness::Unconfigured);
        assert_eq!(
            freshness_note(&Freshness::Stale { fetched_at: 0.0 }, 12.0 * MIN_MS),
            Some((Tone::Warn, "updated 12m ago".into()))
        );
    }

    #[test]
    fn sentence_and_order_lead_with_what_needs_you() {
        let today = parse_ymd("2026-10-09").unwrap();
        let res = response(vec![
            delivery(ParcelDeliveryStatus::InTransit),
            delivery(ParcelDeliveryStatus::OutForDelivery),
            delivery(ParcelDeliveryStatus::AwaitingPickup),
            delivery(ParcelDeliveryStatus::Completed),
        ]);
        assert_eq!(
            deliveries_sentence(&res, today),
            (
                "1 delivery needs you.".into(),
                "3 active, 1 out for delivery, 1 delivered recently.".into()
            )
        );
        let order: Vec<ParcelDeliveryStatus> = tile_order(&res.deliveries)
            .iter()
            .map(|d| d.status)
            .collect();
        assert_eq!(
            order,
            [
                ParcelDeliveryStatus::AwaitingPickup,
                ParcelDeliveryStatus::OutForDelivery,
                ParcelDeliveryStatus::InTransit
            ]
        );
        assert_eq!(attention_count(&res), 1);
        assert!(!can_be_late(&delivery(
            ParcelDeliveryStatus::AwaitingPickup
        )));
        assert!(can_be_late(&delivery(ParcelDeliveryStatus::InTransit)));
        let quiet = response(vec![delivery(ParcelDeliveryStatus::InTransit)]);
        assert_eq!(
            deliveries_sentence(&quiet, today),
            (
                "1 delivery on the way.".into(),
                "Nothing delivered recently.".into()
            )
        );
        let mut late = delivery(ParcelDeliveryStatus::InTransit);
        late.expected = Some("2026-10-06".into());
        let pickup = delivery(ParcelDeliveryStatus::AwaitingPickup);
        assert_eq!(
            deliveries_sentence(&response(vec![late.clone(), pickup]), today),
            (
                "1 delivery needs you.".into(),
                "2 active, 1 late, nothing delivered recently.".into()
            )
        );
        assert_eq!(
            deliveries_sentence(&response(vec![late]), today).0,
            "1 delivery is late."
        );
        assert_eq!(
            status_look(ParcelDeliveryStatus::Exception).0,
            StatusKind::Fault
        );
        assert_eq!(
            status_look(ParcelDeliveryStatus::InTransit).0,
            StatusKind::Idle
        );
    }
}
