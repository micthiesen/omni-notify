//! Numbers as instruments: [`TickNum`], [`Readout`], [`ReadoutBand`],
//! [`Meter`], [`Sparkline`], [`RunStrip`], [`TimeLane`].

use leptos::html::Span;
use leptos::prelude::*;
use wasm_bindgen::JsCast as _;

use super::tone::Tone;

/// Restarts the `.tick` flash on `el`.
fn restart_tick(el: &web_sys::Element) {
    let list = el.class_list();
    let _ = list.remove_1("tick");
    if let Some(html) = el.dyn_ref::<web_sys::HtmlElement>() {
        // Reading layout restarts the CSS animation.
        let _ = html.offset_width();
    }
    let _ = list.add_1("tick");
}

/// A mono figure whose text flashes Signal when it changes (never on first
/// render). Only the text node updates, so focus and hover survive.
#[component]
pub fn TickNum(
    #[prop(into)] value: Signal<String>,
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(into, optional)] title: MaybeProp<String>,
) -> impl IntoView {
    let node = NodeRef::<Span>::new();
    Effect::new(move |previous: Option<String>| {
        let current = value.get();
        if let (Some(previous), Some(el)) = (previous, node.get_untracked())
            && previous != current
        {
            restart_tick(&el);
        }
        current
    });
    view! {
        <span
            node_ref=node
            class=move || format!("num {}", class.get().unwrap_or_default())
            title=move || title.get()
        >
            {move || value.get()}
        </span>
    }
}

/// Readout size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReadoutSize {
    /// The one live hero figure per page.
    Xl,
    #[default]
    L,
    M,
}

/// Key, figure and sub-line. `unit` is a small suffix (`/17`, `watching`).
#[component]
pub fn Readout(
    #[prop(into)] label: Signal<String>,
    #[prop(into)] value: Signal<String>,
    #[prop(optional)] size: ReadoutSize,
    #[prop(into, optional)] unit: MaybeProp<String>,
    #[prop(into, optional)] tone: Signal<Tone>,
    #[prop(into, optional)] stale: Signal<bool>,
    #[prop(into, optional)] title: MaybeProp<String>,
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let size_class = match size {
        ReadoutSize::Xl => "xl",
        ReadoutSize::L => "",
        ReadoutSize::M => "m",
    };
    let class = move || {
        format!(
            "readout {size_class} {} {} {}",
            tone.get().class(),
            if stale.get() { "stale" } else { "" },
            class.get().unwrap_or_default()
        )
    };
    view! {
        <div class=class>
            <span class="readout-k">{move || label.get()}</span>
            <span class="readout-v">
                <TickNum value title/>
                {move || unit.get().map(|u| view! { <small>{u}</small> })}
            </span>
            {children.map(|c| view! { <div class="readout-sub">{c()}</div> })}
        </div>
    }
}

/// A row of readouts in one panel with hairline dividers (2 columns on
/// phone).
#[component]
pub fn ReadoutBand(
    #[prop(optional, default = 4)] cols: usize,
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(into, optional)] aria_label: MaybeProp<String>,
    children: Children,
) -> impl IntoView {
    view! {
        <section
            class=move || format!("readouts {}", class.get().unwrap_or_default())
            style=format!("--cols: {cols}")
            aria-label=move || aria_label.get()
        >
            {children()}
        </section>
    }
}

fn percent(value: f64, max: f64) -> f64 {
    if max > 0.0 && value.is_finite() {
        (value / max * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    }
}

/// A 96x6 proportion bar with an optional reference tick.
#[component]
pub fn Meter(
    #[prop(into)] value: Signal<f64>,
    #[prop(into)] max: Signal<f64>,
    #[prop(into, optional)] reference: Signal<Option<f64>>,
    #[prop(into, optional)] tone: Signal<Tone>,
    /// Width in px; `None` fills the parent.
    #[prop(optional)]
    width: Option<u32>,
    #[prop(into)] label: Signal<String>,
) -> impl IntoView {
    let style = width.map(|w| format!("--meter-w: {w}px"));
    view! {
        <span
            class=move || format!("meter {} {}", tone.get().class(), if width.is_none() { "fluid" } else { "" })
            style=style
            role="meter"
            aria-label=move || label.get()
            aria-valuemin="0"
            aria-valuemax=move || max.get().to_string()
            aria-valuenow=move || value.get().to_string()
        >
            <span
                class="meter-fill"
                style=move || format!("width: {:.2}%", percent(value.get(), max.get()))
            ></span>
            {move || {
                reference.get().map(|r| view! {
                    <span class="meter-ref" style=format!("left: {:.2}%", percent(r, max.get()))></span>
                })
            }}
        </span>
    }
}

/// SVG path data for `points` scaled into a `100 x height` box.
pub fn spark_paths(points: &[f64], lo: f64, hi: f64, height: f64) -> (String, String, f64, f64) {
    let span = if hi > lo { hi - lo } else { 1.0 };
    let n = points.len();
    let x = |i: usize| {
        if n <= 1 {
            100.0
        } else {
            i as f64 / (n - 1) as f64 * 100.0
        }
    };
    let y = |v: f64| height - (v - lo) / span * height;
    let mut line = String::new();
    for (i, v) in points.iter().enumerate() {
        line.push_str(&format!(
            "{}{:.2},{:.2}",
            if i == 0 { "M" } else { " L" },
            x(i),
            y(*v)
        ));
    }
    let area = if n > 1 {
        format!("{line} L100,{height} L0,{height} Z")
    } else {
        String::new()
    };
    let (end_x, end_y) = points.last().map_or((100.0, height), |v| (x(n - 1), y(*v)));
    (line, area, end_x, end_y)
}

/// A 1.5 px line with a 12% area wash, an end dot and an optional dashed
/// reference line (typical peak).
#[component]
pub fn Sparkline(
    #[prop(into)] points: Signal<Vec<f64>>,
    #[prop(into, optional)] reference: Signal<Option<f64>>,
    #[prop(optional)] tone: Tone,
    #[prop(optional, default = 48)] height: u32,
    #[prop(into)] label: Signal<String>,
) -> impl IntoView {
    let h = f64::from(height);
    let shape = Memo::new(move |_| {
        let pts = points.get();
        let reference = reference.get();
        let lo = pts.iter().copied().fold(f64::INFINITY, f64::min);
        let top = pts.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        // No movement yet: centre the line instead of pinning it to an edge.
        let flat = lo.is_finite() && top - lo < 1e-9;
        let pad = if flat { (lo.abs() * 0.1).max(1.0) } else { 0.0 };
        let hi = pts
            .iter()
            .copied()
            .map(|p| p + pad)
            .chain(reference)
            .fold(f64::NEG_INFINITY, f64::max);
        let lo = if flat {
            lo - pad
        } else if lo.is_finite() {
            (lo * 0.9).min(hi)
        } else {
            0.0
        };
        let hi = match (hi.is_finite(), flat) {
            (false, _) => 1.0,
            (true, true) => hi,
            (true, false) => hi * 1.04,
        };
        let (line, area, ex, ey) = spark_paths(&pts, lo, hi, h);
        let ref_y = reference.map(|r| h - (r - lo) / (hi - lo).max(1e-9) * h);
        (
            line,
            area,
            ex,
            ey,
            ref_y,
            if flat { pts.len().min(1) } else { pts.len() },
            pts.len(),
        )
    });
    view! {
        <div class="spark-wrap" style=format!("height: {height}px") role="img" aria-label=move || label.get()>
            <svg class=format!("spark {}", tone.class()) viewBox=format!("0 0 100 {height}") preserveAspectRatio="none" height=height>
                {move || {
                    let (line, area, _, _, ref_y, _, n) = shape.get();
                    // One sample draws nothing useful: keep the space quiet.
                    (n > 1).then(|| view! {
                        <path class="area" d=area></path>
                        {ref_y.map(|y| view! { <line class="ref" x1="0" x2="100" y1=y y2=y></line> })}
                        <path class="line" d=line></path>
                    })
                }}
            </svg>
            // Shown until the line has moved: a flat line alone reads as no data.
            {move || (shape.with(|s| s.5) < 2).then(|| view! {
                <span class="spark-wait" aria-hidden="true">"Charting from now"</span>
            })}
            {move || {
                let (_, _, ex, ey, _, _, n) = shape.get();
                (n > 1).then(|| view! {
                    <span
                        class=format!("spark-end {}", tone.class())
                        style=format!("left: {ex:.2}%; top: {:.2}%", ey / h * 100.0)
                    ></span>
                })
            }}
        </div>
    }
}

/// One cell of a [`RunStrip`] or tick of a [`TimeLane`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellKind {
    Ok,
    Fault,
    Running,
    /// Skipped or degraded.
    Warn,
    Missing,
}

impl CellKind {
    fn class(self) -> &'static str {
        match self {
            CellKind::Ok => "",
            CellKind::Fault => "fault",
            CellKind::Running => "running",
            CellKind::Warn => "warn",
            CellKind::Missing => "missing",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunCell {
    pub kind: CellKind,
    /// Time, duration and summary.
    pub title: String,
}

/// Last N runs, oldest first, padded with missing cells to `slots`. On phone
/// only the newest 8 show.
#[component]
pub fn RunStrip(
    #[prop(into)] cells: Signal<Vec<RunCell>>,
    #[prop(optional, default = 12)] slots: usize,
    #[prop(into)] label: Signal<String>,
) -> impl IntoView {
    view! {
        <span class="runstrip" role="img" aria-label=move || label.get()>
            {move || {
                let cells = cells.get();
                let pad = slots.saturating_sub(cells.len());
                let skip = cells.len().saturating_sub(slots);
                let hide_phone = slots.saturating_sub(8);
                (0..pad)
                    .map(|_| RunCell { kind: CellKind::Missing, title: "No run".to_owned() })
                    .chain(cells.into_iter().skip(skip))
                    .enumerate()
                    .map(|(i, cell)| {
                        let class = format!(
                            "{}{}",
                            cell.kind.class(),
                            if i < hide_phone { " hide-phone" } else { "" },
                        );
                        view! { <i class=class title=cell.title></i> }
                    })
                    .collect_view()
            }}
        </span>
    }
}

/// Realtime tasks: run ticks over the last 10 minutes plus 2 ahead, a
/// now-line and hollow scheduled runs.
#[component]
pub fn TimeLane(
    #[prop(into)] now: Signal<f64>,
    #[prop(into)] ticks: Signal<Vec<(f64, CellKind)>>,
    #[prop(into)] scheduled: Signal<Vec<f64>>,
    #[prop(into)] label: Signal<String>,
) -> impl IntoView {
    const BEHIND: f64 = 10.0 * 60_000.0;
    const AHEAD: f64 = 2.0 * 60_000.0;
    let pos = move |t: f64, now: f64| (t - (now - BEHIND)) / (BEHIND + AHEAD) * 100.0;
    view! {
        <span class="timelane" role="img" aria-label=move || label.get()>
            {move || {
                let now = now.get();
                let past = ticks
                    .get()
                    .into_iter()
                    .filter(|(t, _)| *t >= now - BEHIND && *t <= now)
                    .map(|(t, kind)| view! {
                        <i class=kind.class() style=format!("left: {:.2}%", pos(t, now))></i>
                    })
                    .collect_view();
                let next = scheduled
                    .get()
                    .into_iter()
                    .filter(|t| *t > now && *t <= now + AHEAD)
                    .map(|t| view! { <i class="next" style=format!("left: {:.2}%", pos(t, now))></i> })
                    .collect_view();
                view! {
                    {past}
                    {next}
                    <span class="now" style=format!("left: {:.2}%", pos(now, now))></span>
                }
            }}
        </span>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spark_paths_span_the_box() {
        let (line, area, ex, ey) = spark_paths(&[0.0, 5.0, 10.0], 0.0, 10.0, 40.0);
        assert!(line.starts_with("M0.00,40.00"));
        assert!(area.ends_with("Z"));
        assert_eq!((ex, ey), (100.0, 0.0));
        assert_eq!(percent(5.0, 0.0), 0.0);
        assert_eq!(percent(150.0, 100.0), 100.0);
    }
}
