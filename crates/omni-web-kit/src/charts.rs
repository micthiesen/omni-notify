//! Hand-written SVG charts: a stacked [`BarChart`] (Costs, streamer viewers)
//! and a [`LineChart`] with an optional brush (Pets). Both fill their
//! `.chart-container`, follow its size, draw dashed gridlines instead of a
//! y-axis line, and show a caller-supplied tooltip view inside
//! `.chart-tooltip` on hover, tap (pinned until tapped again or Escape) or
//! arrow keys while focused.

use leptos::html::Div;
use leptos::prelude::*;
use wasm_bindgen::JsCast as _;
use wasm_bindgen::closure::Closure;

use crate::task::on_cleanup_local;

const AXIS_HEIGHT: f64 = 30.0;
const TICK_FONT: f64 = 11.0;
/// Rough rendered width of one 12 px tick glyph, for tick thinning.
const GLYPH_WIDTH: f64 = 7.0;

/// Chart margins (recharts `margin`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Margin {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

impl Default for Margin {
    fn default() -> Self {
        Self {
            top: 8.0,
            right: 16.0,
            bottom: 8.0,
            left: 4.0,
        }
    }
}

/// Axis colors; defaults use the theme variables.
#[derive(Clone, Debug, PartialEq)]
pub struct AxisStyle {
    pub tick: String,
    pub line: String,
}

impl Default for AxisStyle {
    fn default() -> Self {
        Self {
            tick: "var(--axis)".into(),
            line: "var(--grid)".into(),
        }
    }
}

/// "Nice" ticks from `min` to at least `max` (about `count` of them).
pub fn nice_ticks(min: f64, max: f64, count: usize) -> Vec<f64> {
    let (min, max) = if max > min {
        (min, max)
    } else {
        (min, min + 1.0)
    };
    let rough = (max - min) / (count.max(2) - 1) as f64;
    let magnitude = 10f64.powf(rough.log10().floor());
    let normalized = rough / magnitude;
    let step = [1.0, 2.0, 2.5, 5.0, 10.0]
        .into_iter()
        .find(|n| *n >= normalized - 1e-9)
        .unwrap_or(10.0)
        * magnitude;
    let start = (min / step).floor() * step;
    let end = (max / step).ceil() * step;
    let mut ticks = Vec::new();
    let mut value = start;
    while value <= end + step * 1e-6 {
        // Trim float noise (0.30000000000000004).
        ticks.push((value / step).round() * step);
        value += step;
    }
    ticks
}

/// Indices of category labels to draw, keeping the last one and at least
/// `gap` pixels between label boxes (recharts `interval="preserveEnd"`).
pub fn thin_ticks(centers: &[f64], labels: &[String], gap: f64) -> Vec<usize> {
    let mut kept = Vec::new();
    let mut last_left: Option<f64> = None;
    for index in (0..centers.len()).rev() {
        let width = labels
            .get(index)
            .map_or(0.0, |l| l.chars().count() as f64 * GLYPH_WIDTH);
        let right = centers[index] + width / 2.0;
        if last_left.is_none_or(|left| right + gap <= left) {
            kept.push(index);
            last_left = Some(centers[index] - width / 2.0);
        }
    }
    kept.reverse();
    kept
}

/// Tracks the size of the node behind `node` (window resizes included).
fn use_size(node: NodeRef<Div>) -> ReadSignal<(f64, f64)> {
    let (size, set_size) = signal((0.0, 0.0));
    let measure = move || {
        if let Some(el) = node.get_untracked() {
            set_size.set((f64::from(el.client_width()), f64::from(el.client_height())));
        }
    };
    Effect::new(move |_| {
        if node.get().is_some() {
            measure();
        }
    });
    let on_resize = Closure::<dyn FnMut()>::new(measure);
    let win = window();
    let _ = win.add_event_listener_with_callback("resize", on_resize.as_ref().unchecked_ref());
    on_cleanup_local(move || {
        let _ =
            win.remove_event_listener_with_callback("resize", on_resize.as_ref().unchecked_ref());
    });
    size
}

/// Keyboard stepping over chart points: Left/Right move, Home/End jump,
/// Escape clears. Returns the next index, or `None` to clear.
pub fn step_index(key: &str, current: Option<usize>, count: usize) -> Option<Option<usize>> {
    if count == 0 {
        return None;
    }
    let last = count - 1;
    Some(match key {
        "ArrowRight" => Some(current.map_or(last, |i| (i + 1).min(last))),
        "ArrowLeft" => Some(current.map_or(last, |i| i.saturating_sub(1))),
        "Home" => Some(0),
        "End" => Some(last),
        "Escape" => None,
        _ => return None,
    })
}

fn pointer_x(event: &web_sys::MouseEvent) -> Option<(f64, f64)> {
    let target = event
        .current_target()?
        .dyn_into::<web_sys::Element>()
        .ok()?;
    let rect = target.get_bounding_client_rect();
    Some((
        f64::from(event.client_x()) - rect.left(),
        f64::from(event.client_y()) - rect.top(),
    ))
}

/// Tooltip placement beside the pointer, flipped near the right edge.
fn tooltip_style(x: f64, y: f64, width: f64) -> String {
    let horizontal = if x > width * 0.6 {
        format!("right: {}px;", (width - x + 10.0).max(0.0))
    } else {
        format!("left: {}px;", x + 10.0)
    };
    format!("top: {}px; {horizontal}", y.max(0.0))
}

// ---------------------------------------------------------------- bar chart

/// One stacked series of a [`BarChart`].
#[derive(Clone, Debug, PartialEq)]
pub struct BarSeries {
    pub key: String,
    pub color: String,
}

/// One category: its x key and a value per series (in series order).
#[derive(Clone, Debug, PartialEq)]
pub struct BarPoint {
    pub x: String,
    pub values: Vec<f64>,
}

/// Stacked bars over categorical x values.
#[component]
pub fn BarChart(
    #[prop(into)] data: Signal<Vec<BarPoint>>,
    #[prop(into)] series: Signal<Vec<BarSeries>>,
    x_tick: Callback<String, String>,
    y_tick: Callback<f64, String>,
    /// Tooltip content for a category index (render a `.chart-tooltip`).
    tooltip: Callback<usize, AnyView>,
    #[prop(default = 44.0)] y_width: f64,
    #[prop(default = 28.0)] max_bar_size: f64,
    /// Corner radius of the top segment.
    #[prop(default = 0.0)]
    radius: f64,
    #[prop(default = 40.0)] min_tick_gap: f64,
    #[prop(into, default = "var(--hover)".to_owned())] cursor_fill: String,
    #[prop(default = Margin::default())] margin: Margin,
    #[prop(default = AxisStyle::default())] axis: AxisStyle,
    /// Extra class for a category's bars (`"bar-record"`, `"bar-today"`).
    #[prop(optional)]
    bar_class: Option<Callback<usize, Option<String>>>,
    /// Dashed reference line with its label ("typical 12.4K").
    #[prop(into, optional)]
    reference: Signal<Option<(f64, String)>>,
    /// Accessible summary of the chart.
    #[prop(into, optional)]
    label: MaybeProp<String>,
) -> impl IntoView {
    let node = NodeRef::<Div>::new();
    let size = use_size(node);
    let hover = RwSignal::new(None::<(usize, f64, f64)>);
    let pinned = RwSignal::new(false);
    let anchors = StoredValue::new(Vec::<(f64, f64)>::new());

    let layout = move || {
        let (width, height) = size.get();
        let plot_left = margin.left + y_width;
        let plot_right = (width - margin.right).max(plot_left + 1.0);
        let plot_top = margin.top;
        let plot_bottom = (height - margin.bottom - AXIS_HEIGHT).max(plot_top + 1.0);
        (width, height, plot_left, plot_right, plot_top, plot_bottom)
    };

    let chart = move || {
        let (width, height, left, right, top, bottom) = layout();
        if width <= 0.0 {
            return None;
        }
        let points = data.get();
        let series = series.get();
        let reference = reference.get();
        let max = points
            .iter()
            .map(|p| p.values.iter().filter(|v| v.is_finite()).sum::<f64>())
            .chain(reference.as_ref().map(|(v, _)| *v))
            .fold(0.0, f64::max);
        let ticks = nice_ticks(0.0, max, 5);
        let y_max = ticks.last().copied().unwrap_or(1.0).max(f64::MIN_POSITIVE);
        let scale_y = move |v: f64| bottom - (v / y_max) * (bottom - top);
        let n = points.len().max(1) as f64;
        let band = (right - left) / n;
        let bar_width = (band * 0.8).min(max_bar_size);
        let centers: Vec<f64> = (0..points.len())
            .map(|i| left + band * (i as f64 + 0.5))
            .collect();
        let labels: Vec<String> = points.iter().map(|p| x_tick.run(p.x.clone())).collect();
        let shown = thin_ticks(&centers, &labels, min_tick_gap);
        anchors.set_value(
            points
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let total: f64 = p.values.iter().filter(|v| v.is_finite()).sum();
                    (centers[i], scale_y(total))
                })
                .collect(),
        );

        let hovered = hover.get();
        let cursor = hovered.map(|(index, _, _)| {
            view! {
                <rect
                    x=left + band * index as f64
                    y=top
                    width=band
                    height=bottom - top
                    fill=cursor_fill.clone()
                ></rect>
            }
        });
        let bars = points
            .iter()
            .enumerate()
            .flat_map(|(index, point)| {
                let x = centers[index] - bar_width / 2.0;
                let mark = bar_class.and_then(|c| c.run(index));
                let top_segment = point.values.iter().rposition(|v| *v > 0.0);
                let mut base = 0.0;
                point
                    .values
                    .iter()
                    .enumerate()
                    .filter_map(|(si, value)| {
                        let value = if value.is_finite() { *value } else { 0.0 };
                        if value <= 0.0 {
                            return None;
                        }
                        let y0 = scale_y(base);
                        base += value;
                        let y1 = scale_y(base);
                        let color = series.get(si).map(|s| s.color.clone()).unwrap_or_default();
                        let h = (y0 - y1).max(0.0);
                        let r = if Some(si) == top_segment { radius.min(h).min(bar_width / 2.0) } else { 0.0 };
                        let path = format!(
                            "M{x},{y0} L{x},{yr} Q{x},{y1} {xr},{y1} L{xl},{y1} Q{x2},{y1} {x2},{yr} L{x2},{y0} Z",
                            yr = y1 + r,
                            xr = x + r,
                            xl = x + bar_width - r,
                            x2 = x + bar_width,
                        );
                        Some(view! { <path d=path fill=color class=mark.clone()></path> })
                    })
                    .collect::<Vec<_>>()
            })
            .collect_view();
        let y_axis = ticks
            .iter()
            .map(|tick| {
                let y = scale_y(*tick);
                view! {
                    <g>
                        <line class="chart-grid" x1=left x2=right y1=y y2=y stroke=axis.line.clone() stroke-dasharray="2 4"></line>
                        <text
                            class="chart-axis"
                            x=left - 8.0
                            y=y
                            text-anchor="end"
                            dominant-baseline="central"
                            fill=axis.tick.clone()
                            font-size=TICK_FONT
                        >
                            {y_tick.run(*tick)}
                        </text>
                    </g>
                }
            })
            .collect_view();
        let x_axis = shown
            .into_iter()
            .map(|index| {
                let x = centers[index];
                view! {
                    <g>
                        <text
                            class="chart-axis"
                            x=x
                            y=bottom + 9.0
                            text-anchor="middle"
                            dominant-baseline="hanging"
                            fill=axis.tick.clone()
                            font-size=TICK_FONT
                        >
                            {labels[index].clone()}
                        </text>
                    </g>
                }
            })
            .collect_view();
        let tip = hovered.map(|(index, x, y)| {
            view! { <div class="chart-tooltip" style=tooltip_style(x, y, width)>{tooltip.run(index)}</div> }
        });
        let count = points.len();
        let locate = move |event: &web_sys::PointerEvent| {
            let (x, y) = pointer_x(event)?;
            (x >= left && x <= right && y >= top && y <= bottom && count > 0)
                .then(|| ((((x - left) / band).floor() as usize).min(count - 1), x, y))
        };
        let reference_line = reference.map(|(value, text)| {
            let y = scale_y(value);
            view! {
                <line class="chart-ref" x1=left x2=right y1=y y2=y></line>
                <text class="chart-ref-label" x=right y=y - 4.0 text-anchor="end">{text}</text>
            }
        });
        Some(view! {
            <svg
                class="chart-surface"
                width=width
                height=height
                viewBox=format!("0 0 {width} {height}")
                on:pointermove=move |event: web_sys::PointerEvent| {
                    if event.pointer_type() == "mouse" && !pinned.get_untracked() {
                        hover.set(locate(&event));
                    }
                }
                on:pointerleave=move |event: web_sys::PointerEvent| {
                    if event.pointer_type() == "mouse" && !pinned.get_untracked() {
                        hover.set(None);
                    }
                }
                on:pointerdown=move |event: web_sys::PointerEvent| {
                    let next = locate(&event);
                    let same = next.map(|n| n.0) == hover.get_untracked().map(|h| h.0);
                    if event.pointer_type() == "mouse" {
                        pinned.set(next.is_some() && !(same && pinned.get_untracked()));
                        hover.set(next);
                    } else if same {
                        hover.set(None);
                    } else {
                        hover.set(next);
                    }
                }
            >
                {cursor}
                {y_axis}
                <line class="chart-baseline" x1=left x2=right y1=bottom y2=bottom></line>
                {x_axis}
                {bars}
                {reference_line}
            </svg>
            {tip}
        })
    };

    let on_key = move |event: web_sys::KeyboardEvent| {
        let points = anchors.get_value();
        let current = hover.get_untracked().map(|h| h.0);
        if let Some(next) = step_index(&event.key(), current, points.len()) {
            event.prevent_default();
            pinned.set(next.is_some());
            hover.set(next.map(|i| (i, points[i].0, points[i].1)));
        }
    };

    view! {
        <div
            node_ref=node
            class="chart-frame"
            tabindex="0"
            role="img"
            aria-label=move || label.get().unwrap_or_else(|| "Bar chart".to_owned())
            on:keydown=on_key
            on:blur=move |_| {
                pinned.set(false);
                hover.set(None);
            }
        >
            {chart}
        </div>
    }
}

// --------------------------------------------------------------- line chart

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Curve {
    #[default]
    Linear,
    /// Monotone cubic interpolation (recharts `type="monotone"`).
    Monotone,
}

/// One line of a [`LineChart`].
#[derive(Clone, Debug, PartialEq)]
pub struct LineSeries {
    pub key: String,
    pub stroke: String,
    pub width: f64,
    pub dash: Option<String>,
    pub opacity: f64,
    /// Draw point markers (radius 3 when at most 60 points) and an active dot.
    pub dot: bool,
    pub curve: Curve,
}

/// One x position (epoch ms or any number) and a value per series.
#[derive(Clone, Debug, PartialEq)]
pub struct LinePoint {
    pub x: f64,
    pub values: Vec<Option<f64>>,
}

fn monotone_path(points: &[(f64, f64)]) -> String {
    let n = points.len();
    if n == 0 {
        return String::new();
    }
    if n < 3 {
        return linear_path(points);
    }
    let mut slopes = Vec::with_capacity(n - 1);
    for w in points.windows(2) {
        let dx = w[1].0 - w[0].0;
        slopes.push(if dx == 0.0 {
            0.0
        } else {
            (w[1].1 - w[0].1) / dx
        });
    }
    let mut tangents = vec![0.0; n];
    tangents[0] = slopes[0];
    tangents[n - 1] = slopes[n - 2];
    for i in 1..n - 1 {
        let (a, b) = (slopes[i - 1], slopes[i]);
        // Fritsch-Carlson: harmonic mean of the neighbouring slopes.
        tangents[i] = if a * b <= 0.0 {
            0.0
        } else {
            2.0 / (1.0 / a + 1.0 / b)
        };
    }
    let mut path = format!("M{},{}", points[0].0, points[0].1);
    for i in 0..n - 1 {
        let (x0, y0) = points[i];
        let (x1, y1) = points[i + 1];
        let h = (x1 - x0) / 3.0;
        path.push_str(&format!(
            " C{},{} {},{} {},{}",
            x0 + h,
            y0 + tangents[i] * h,
            x1 - h,
            y1 - tangents[i + 1] * h,
            x1,
            y1
        ));
    }
    path
}

fn linear_path(points: &[(f64, f64)]) -> String {
    points
        .iter()
        .enumerate()
        .map(|(i, (x, y))| format!("{}{x},{y}", if i == 0 { "M" } else { " L" }))
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum BrushDrag {
    Start,
    End,
    Window {
        anchor: f64,
        start: usize,
        end: usize,
    },
}

/// Lines over a numeric x axis with an optional index brush.
#[component]
pub fn LineChart(
    #[prop(into)] data: Signal<Vec<LinePoint>>,
    #[prop(into)] series: Signal<Vec<LineSeries>>,
    /// Fixed y domain; `None` uses nice ticks over the visible values.
    #[prop(into)]
    y_domain: Signal<Option<(f64, f64)>>,
    x_tick: Callback<f64, String>,
    y_tick: Callback<f64, String>,
    /// Tooltip content for a point index into `data`.
    tooltip: Callback<usize, AnyView>,
    #[prop(into, default = Signal::stored(false))] show_brush: Signal<bool>,
    #[prop(default = 40.0)] y_width: f64,
    #[prop(default = 50.0)] min_tick_gap: f64,
    #[prop(default = AxisStyle::default())] axis: AxisStyle,
    /// Accessible summary of the chart.
    #[prop(into, optional)]
    label: MaybeProp<String>,
) -> impl IntoView {
    let node = NodeRef::<Div>::new();
    let size = use_size(node);
    let hover = RwSignal::new(None::<(usize, f64, f64)>);
    let pinned = RwSignal::new(false);
    let anchors = StoredValue::new(Vec::<(usize, f64, f64)>::new());
    let range = RwSignal::new(None::<(usize, usize)>);
    let drag = StoredValue::new(None::<BrushDrag>);

    // A new data set resets the brush to the full range.
    Effect::new(move |_| {
        data.track();
        range.set(None);
    });

    let chart = move || {
        let (width, height) = size.get();
        if width <= 0.0 {
            return None;
        }
        let points = data.get();
        let series = series.get();
        let brush = show_brush.get() && points.len() > 1;
        let margin = Margin {
            bottom: if brush { 28.0 } else { 8.0 },
            ..Margin::default()
        };
        let left = margin.left + y_width;
        let right = (width - margin.right).max(left + 1.0);
        let top = margin.top;
        let brush_height = if brush { 24.0 } else { 0.0 };
        let bottom = (height - margin.bottom - AXIS_HEIGHT - brush_height).max(top + 1.0);
        let last = points.len().saturating_sub(1);
        let (start, end) = range.get().unwrap_or((0, last));
        let (start, end) = (start.min(last), end.min(last).max(start.min(last)));
        let visible: Vec<usize> = if points.is_empty() {
            Vec::new()
        } else {
            (start..=end).collect()
        };

        let x_min = visible.first().map_or(0.0, |i| points[*i].x);
        let x_max = visible.last().map_or(1.0, |i| points[*i].x);
        let x_span = if x_max > x_min { x_max - x_min } else { 1.0 };
        let scale_x = move |x: f64| {
            if x_max > x_min {
                left + (x - x_min) / x_span * (right - left)
            } else {
                (left + right) / 2.0
            }
        };
        let (y_lo, y_hi, y_ticks) = match y_domain.get() {
            Some((lo, hi)) => {
                let ticks = nice_ticks(lo, hi, 5)
                    .into_iter()
                    .filter(|t| *t >= lo - 1e-9 && *t <= hi + 1e-9)
                    .collect();
                (lo, if hi > lo { hi } else { lo + 1.0 }, ticks)
            }
            None => {
                let values: Vec<f64> = visible
                    .iter()
                    .flat_map(|i| points[*i].values.iter().flatten().copied())
                    .filter(|v| v.is_finite())
                    .collect();
                let lo = values
                    .iter()
                    .copied()
                    .fold(f64::INFINITY, f64::min)
                    .min(0.0);
                let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                let ticks = nice_ticks(
                    if lo.is_finite() { lo } else { 0.0 },
                    if hi.is_finite() { hi } else { 1.0 },
                    5,
                );
                let lo = ticks.first().copied().unwrap_or(0.0);
                let hi = ticks.last().copied().unwrap_or(1.0);
                (lo, hi, ticks)
            }
        };
        let scale_y = move |v: f64| bottom - (v - y_lo) / (y_hi - y_lo) * (bottom - top);

        let lines = series
            .iter()
            .enumerate()
            .map(|(si, line)| {
                let coords: Vec<(f64, f64)> = visible
                    .iter()
                    .filter_map(|i| {
                        let p = &points[*i];
                        p.values.get(si).copied().flatten().map(|v| (scale_x(p.x), scale_y(v)))
                    })
                    .collect();
                let d = match line.curve {
                    Curve::Linear => linear_path(&coords),
                    Curve::Monotone => monotone_path(&coords),
                };
                let dot_radius = if visible.len() <= 60 { 3.0 } else { 0.0 };
                let dots = (line.dot && dot_radius > 0.0).then(|| {
                    coords
                        .iter()
                        .map(|(x, y)| {
                            view! { <circle cx=*x cy=*y r=dot_radius fill=line.stroke.clone()></circle> }
                        })
                        .collect_view()
                });
                view! {
                    <g>
                        <path
                            d=d
                            fill="none"
                            stroke=line.stroke.clone()
                            stroke-width=line.width
                            stroke-dasharray=line.dash.clone()
                            stroke-opacity=line.opacity
                        ></path>
                        {dots}
                    </g>
                }
            })
            .collect_view();

        let y_axis = y_ticks
            .iter()
            .map(|tick| {
                let y = scale_y(*tick);
                view! {
                    <g>
                        <line class="chart-grid" x1=left x2=right y1=y y2=y stroke=axis.line.clone() stroke-dasharray="2 4"></line>
                        <text
                            class="chart-axis"
                            x=left - 8.0
                            y=y
                            text-anchor="end"
                            dominant-baseline="central"
                            fill=axis.tick.clone()
                            font-size=TICK_FONT
                        >
                            {y_tick.run(*tick)}
                        </text>
                    </g>
                }
            })
            .collect_view();
        let x_values: Vec<f64> = if x_max > x_min {
            nice_ticks(x_min, x_max, 6)
                .into_iter()
                .filter(|x| *x >= x_min && *x <= x_max)
                .collect()
        } else {
            vec![x_min]
        };
        let x_labels: Vec<String> = x_values.iter().map(|x| x_tick.run(*x)).collect();
        let x_centers: Vec<f64> = x_values.iter().map(|x| scale_x(*x)).collect();
        let x_axis = thin_ticks(&x_centers, &x_labels, min_tick_gap)
            .into_iter()
            .map(|i| {
                let x = x_centers[i];
                view! {
                    <g>
                        <text
                            class="chart-axis"
                            x=x
                            y=bottom + 9.0
                            text-anchor="middle"
                            dominant-baseline="hanging"
                            fill=axis.tick.clone()
                            font-size=TICK_FONT
                        >
                            {x_labels[i].clone()}
                        </text>
                    </g>
                }
            })
            .collect_view();

        let hovered = hover.get().filter(|(i, _, _)| visible.contains(i));
        let active = hovered.map(|(index, _, _)| {
            let p = &points[index];
            let x = scale_x(p.x);
            let dots = series
                .iter()
                .enumerate()
                .filter(|(_, line)| line.dot)
                .filter_map(|(si, line)| {
                    let v = p.values.get(si).copied().flatten()?;
                    Some(view! {
                        <circle
                            cx=x
                            cy=scale_y(v)
                            r="5"
                            fill=line.stroke.clone()
                            stroke="var(--overlay)"
                            stroke-width="2"
                        ></circle>
                    })
                })
                .collect_view();
            view! {
                <line x1=x x2=x y1=top y2=bottom stroke="var(--line-strong)"></line>
                {dots}
            }
        });
        let tip = hovered.map(|(index, x, y)| {
            view! { <div class="chart-tooltip" style=tooltip_style(x, y, width)>{tooltip.run(index)}</div> }
        });

        let brush_view = brush.then(|| {
            let by = bottom + AXIS_HEIGHT + 4.0;
            let step = if last > 0 {
                (right - left) / last as f64
            } else {
                0.0
            };
            let sx = left + start as f64 * step;
            let ex = left + end as f64 * step;
            let index_at = move |x: f64| {
                if step <= 0.0 {
                    0
                } else {
                    (((x - left) / step).round().max(0.0) as usize).min(last)
                }
            };
            let begin = move |kind: BrushDrag, event: web_sys::PointerEvent| {
                event.prevent_default();
                if let Some(target) = event
                    .current_target()
                    .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                {
                    let _ = target.set_pointer_capture(event.pointer_id());
                }
                drag.set_value(Some(kind));
            };
            let on_move = move |event: web_sys::PointerEvent| {
                let Some(kind) = drag.get_value() else { return };
                let Some(svg) = event
                    .current_target()
                    .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                    .and_then(|el| el.closest("svg").ok().flatten())
                else {
                    return;
                };
                let x = f64::from(event.client_x()) - svg.get_bounding_client_rect().left();
                let (s, e) = range.get_untracked().unwrap_or((0, last));
                let next = match kind {
                    BrushDrag::Start => (index_at(x).min(e), e),
                    BrushDrag::End => (s, index_at(x).max(s)),
                    BrushDrag::Window { anchor, start, end } => {
                        let shift = if step > 0.0 {
                            ((x - anchor) / step).round() as i64
                        } else {
                            0
                        };
                        let span = end - start;
                        let new_start =
                            (start as i64 + shift).clamp(0, (last - span) as i64) as usize;
                        (new_start, new_start + span)
                    }
                };
                range.set(Some(next));
            };
            let end_drag = move |_event: web_sys::PointerEvent| drag.set_value(None);
            view! {
                <g class="chart-brush">
                    <rect class="brush-track" x=left y=by width=right - left height="24"></rect>
                    <rect
                        class="brush-window"
                        x=sx
                        y=by
                        width=(ex - sx).max(1.0)
                        height="24"
                        on:pointerdown=move |event: web_sys::PointerEvent| {
                            let x = f64::from(event.offset_x());
                            begin(BrushDrag::Window { anchor: x, start, end }, event)
                        }
                        on:pointermove=on_move
                        on:pointerup=end_drag
                    ></rect>
                    <rect
                        x=sx - 4.0
                        y=by
                        class="brush-handle"
                        width="8"
                        height="24"
                        on:pointerdown=move |event| begin(BrushDrag::Start, event)
                        on:pointermove=on_move
                        on:pointerup=end_drag
                    ></rect>
                    <rect
                        x=ex - 4.0
                        y=by
                        class="brush-handle"
                        width="8"
                        height="24"
                        on:pointerdown=move |event| begin(BrushDrag::End, event)
                        on:pointermove=on_move
                        on:pointerup=end_drag
                    ></rect>
                </g>
            }
        });

        let xs: Vec<(usize, f64)> = visible
            .iter()
            .map(|i| (*i, scale_x(points[*i].x)))
            .collect();
        anchors.set_value(
            xs.iter()
                .map(|(i, x)| {
                    let y = points[*i]
                        .values
                        .iter()
                        .flatten()
                        .copied()
                        .fold(f64::NEG_INFINITY, f64::max);
                    (*i, *x, if y.is_finite() { scale_y(y) } else { bottom })
                })
                .collect(),
        );
        let locate = move |event: &web_sys::PointerEvent| {
            let (x, y) = pointer_x(event)?;
            if x < left || x > right || y < top || y > bottom {
                return None;
            }
            xs.iter()
                .min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs()))
                .map(|(i, _)| (*i, x, y))
        };
        let locate_down = locate.clone();
        Some(view! {
            <svg
                class="chart-surface"
                width=width
                height=height
                viewBox=format!("0 0 {width} {height}")
                on:pointermove=move |event: web_sys::PointerEvent| {
                    if event.pointer_type() == "mouse" && !pinned.get_untracked() && drag.get_value().is_none() {
                        hover.set(locate(&event));
                    }
                }
                on:pointerleave=move |event: web_sys::PointerEvent| {
                    if event.pointer_type() == "mouse" && !pinned.get_untracked() {
                        hover.set(None);
                    }
                }
                on:pointerdown=move |event: web_sys::PointerEvent| {
                    let next = locate_down(&event);
                    if next.is_none() {
                        return;
                    }
                    let same = next.map(|n| n.0) == hover.get_untracked().map(|h| h.0);
                    if event.pointer_type() == "mouse" {
                        pinned.set(!(same && pinned.get_untracked()));
                        hover.set(next);
                    } else if same {
                        hover.set(None);
                    } else {
                        hover.set(next);
                    }
                }
            >
                {y_axis}
                <line class="chart-baseline" x1=left x2=right y1=bottom y2=bottom></line>
                {x_axis}
                {lines}
                {active}
                {brush_view}
            </svg>
            {tip}
        })
    };

    let on_key = move |event: web_sys::KeyboardEvent| {
        let points = anchors.get_value();
        let current = hover
            .get_untracked()
            .and_then(|h| points.iter().position(|p| p.0 == h.0));
        if let Some(next) = step_index(&event.key(), current, points.len()) {
            event.prevent_default();
            pinned.set(next.is_some());
            hover.set(next.map(|i| points[i]));
        }
    };

    view! {
        <div
            node_ref=node
            class="chart-frame"
            tabindex="0"
            role="img"
            aria-label=move || label.get().unwrap_or_else(|| "Line chart".to_owned())
            on:keydown=on_key
            on:blur=move |_| {
                pinned.set(false);
                hover.set(None);
            }
        >
            {chart}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice_ticks_cover_the_range() {
        assert_eq!(nice_ticks(0.0, 95.0, 5), [0.0, 25.0, 50.0, 75.0, 100.0]);
        assert_eq!(nice_ticks(0.0, 0.0, 5), [0.0, 0.25, 0.5, 0.75, 1.0]);
        assert_eq!(nice_ticks(0.0, 4200.0, 5), [0.0, 2000.0, 4000.0, 6000.0]);
    }

    #[test]
    fn keys_step_through_points() {
        assert_eq!(step_index("ArrowRight", None, 5), Some(Some(4)));
        assert_eq!(step_index("ArrowLeft", Some(0), 5), Some(Some(0)));
        assert_eq!(step_index("ArrowRight", Some(2), 5), Some(Some(3)));
        assert_eq!(step_index("Escape", Some(2), 5), Some(None));
        assert_eq!(step_index("a", Some(2), 5), None);
        assert_eq!(step_index("Home", Some(2), 0), None);
    }

    #[test]
    fn thinning_keeps_the_last_label() {
        let centers: Vec<f64> = (0..10).map(|i| i as f64 * 20.0).collect();
        let labels: Vec<String> = (0..10).map(|_| "Oct 9".to_owned()).collect();
        let kept = thin_ticks(&centers, &labels, 40.0);
        assert_eq!(kept.last(), Some(&9));
        assert!(
            kept.windows(2)
                .all(|w| centers[w[1]] - centers[w[0]] >= 75.0)
        );
    }
}
