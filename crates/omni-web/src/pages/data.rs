//! Data: browse, inspect, download and delete stored entity rows.

use std::collections::HashMap;

use leptos::prelude::*;
use omni_web_kit::api::{self, DataRow, ManagedDataSummary, ManagedEntitySummary};
use omni_web_kit::components::{
    Button, ButtonSize, ButtonVariant, ConfirmButton, EmptyState, ErrorState, Icon, InlineNote,
    Inspector, PageHead, Readout, ReadoutBand, ReadoutSize, SearchField, ShowMoreButton,
    SkeletonRows, ToastKind, Tone, use_show_more, use_toast,
};
use omni_web_kit::hooks::use_is_wide;
use omni_web_kit::task::{spawn_detached, spawn_scoped};
use omni_web_kit::utils::download::download_file;
use omni_web_kit::utils::format::to_title_case;
use omni_web_kit::utils::js::{
    locale_compare, locale_compare_numeric, locale_lowercase, locale_number, number_string,
    to_fixed, utf16_len, utf16_slice,
};
use serde_json::{Map, Value};

const ROWS_PER_PAGE: usize = 100;
const MALFORMED_ROW_KEY: &str = "__dataManagerMalformed";
const CELL_TEXT_MAX: usize = 200;
const CELL_TITLE_MAX: usize = 1000;

/// Recency columns in preference order; the first numeric one is the default
/// sort (newest first).
const RECENCY_COLUMNS: [&str; 15] = [
    "createdAt",
    "t",
    "timestamp",
    "startedAt",
    "recordedAt",
    "observedAt",
    "processedAt",
    "receivedAt",
    "recommendedAt",
    "generatedAt",
    "submittedAt",
    "deliveredAt",
    "updatedAt",
    "endedAt",
    "evaluatedThrough",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction {
    Asc,
    Desc,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SortState {
    column: String,
    direction: Direction,
}

fn default_sort(rows: &[DataRow]) -> Option<SortState> {
    RECENCY_COLUMNS
        .iter()
        .find(|column| {
            rows.iter()
                .any(|row| row.get(**column).is_some_and(Value::is_number))
        })
        .map(|column| SortState {
            column: (*column).to_owned(),
            direction: Direction::Desc,
        })
}

struct Malformed {
    raw_key: String,
    error: String,
}

fn malformed_metadata(row: &DataRow) -> Option<Malformed> {
    let value = row.get(MALFORMED_ROW_KEY)?.as_object()?;
    Some(Malformed {
        raw_key: value.get("rawKey")?.as_str()?.to_owned(),
        error: value.get("error")?.as_str()?.to_owned(),
    })
}

pub fn format_bytes(bytes: f64) -> String {
    if bytes < 1024.0 {
        return format!("{} B", number_string(bytes));
    }
    let mut value = bytes / 1024.0;
    let mut unit = "KB";
    for next in ["MB", "GB"] {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = next;
    }
    format!(
        "{} {unit}",
        if value >= 10.0 {
            to_fixed(value, 0)
        } else {
            to_fixed(value, 1)
        }
    )
}

/// `JSON.stringify` (compact) with JS number rendering.
pub fn json(value: &Value) -> String {
    match value {
        Value::Number(n) => n.as_f64().map_or_else(|| n.to_string(), number_string),
        Value::Array(items) => {
            format!("[{}]", items.iter().map(json).collect::<Vec<_>>().join(","))
        }
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(k, v)| format!("{}:{}", Value::String(k.clone()), json(v)))
                .collect::<Vec<_>>()
                .join(",")
        ),
        other => other.to_string(),
    }
}

fn json_row(row: &DataRow) -> String {
    json(&Value::Object(row.clone()))
}

fn key_for(row: &DataRow, primary_key: &[String]) -> DataRow {
    if let Some(value) = row
        .get(MALFORMED_ROW_KEY)
        .filter(|_| malformed_metadata(row).is_some())
    {
        let mut key = Map::new();
        if let Some(meta) = value.as_object() {
            let mut only = Map::new();
            for field in ["rawKey", "error"] {
                if let Some(v) = meta.get(field) {
                    only.insert(field.to_owned(), v.clone());
                }
            }
            key.insert(MALFORMED_ROW_KEY.to_owned(), Value::Object(only));
        }
        return key;
    }
    primary_key
        .iter()
        .filter_map(|p| row.get(p).map(|v| (p.clone(), v.clone())))
        .collect()
}

fn row_id(row: &DataRow, primary_key: &[String]) -> String {
    json_row(&key_for(row, primary_key))
}

fn truncate(value: &str, max: usize) -> String {
    if utf16_len(value) > max {
        format!("{}…", utf16_slice(value, max))
    } else {
        value.to_owned()
    }
}

fn plain_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(_) => json(value),
        Value::Null => "null".to_owned(),
        _ => json(value),
    }
}

/// Cell text and title; huge values are capped so the table stays light.
fn display_value(value: Option<&Value>) -> (String, String) {
    match value {
        None => (String::new(), "Missing".to_owned()),
        Some(Value::Null) => ("null".to_owned(), "null".to_owned()),
        Some(Value::Array(items)) => (
            format!(
                "[{} item{}]",
                items.len(),
                if items.len() == 1 { "" } else { "s" }
            ),
            truncate(&json(value.unwrap_or(&Value::Null)), CELL_TITLE_MAX),
        ),
        Some(Value::Object(map)) => (
            format!(
                "{{{} field{}}}",
                map.len(),
                if map.len() == 1 { "" } else { "s" }
            ),
            truncate(&json(value.unwrap_or(&Value::Null)), CELL_TITLE_MAX),
        ),
        Some(other) => {
            let text = plain_string(other);
            (
                truncate(&text, CELL_TEXT_MAX),
                truncate(&text, CELL_TITLE_MAX),
            )
        }
    }
}

/// `narrow`, `medium` or `wide` from sampled content and header length.
fn classify_column_width(column: &str, rows: &[DataRow]) -> &'static str {
    let mut max_len = (utf16_len(column) as f64 * 0.8).ceil() as usize;
    let mut sampled = 0;
    for row in rows {
        let Some(value) = row.get(column) else {
            continue;
        };
        max_len = max_len.max(utf16_len(&display_value(Some(value)).0));
        sampled += 1;
        if sampled >= 50 {
            break;
        }
    }
    if max_len <= 10 {
        "narrow"
    } else if max_len >= 40 {
        "wide"
    } else {
        "medium"
    }
}

fn compare_values(a: Option<&Value>, b: Option<&Value>) -> std::cmp::Ordering {
    if let (Some(Value::Number(a)), Some(Value::Number(b))) = (a, b) {
        return a
            .as_f64()
            .unwrap_or(0.0)
            .total_cmp(&b.as_f64().unwrap_or(0.0));
    }
    locale_compare_numeric(&display_value(a).0, &display_value(b).0)
}

fn columns_for(rows: &[DataRow], selected: &ManagedEntitySummary) -> Vec<String> {
    let mut frequency: Vec<(String, usize)> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for row in rows {
        for property in row.keys() {
            match index.get(property) {
                Some(i) => frequency[*i].1 += 1,
                None => {
                    index.insert(property.clone(), frequency.len());
                    frequency.push((property.clone(), 1));
                }
            }
        }
    }
    let has_malformed = index.contains_key(MALFORMED_ROW_KEY);
    let mut rest: Vec<(String, usize)> = frequency
        .into_iter()
        .filter(|(p, _)| !selected.primary_key.contains(p) && p != MALFORMED_ROW_KEY)
        .collect();
    rest.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| locale_compare(&a.0, &b.0)));
    let mut columns = selected.primary_key.clone();
    if has_malformed {
        columns.push(MALFORMED_ROW_KEY.to_owned());
    }
    columns.extend(rest.into_iter().map(|(p, _)| p));
    columns
}

#[component]
fn RowInspector(
    entity: ManagedEntitySummary,
    row: DataRow,
    #[prop(into)] docked: Signal<bool>,
    #[prop(into)] deleting: Signal<bool>,
    on_close: Callback<()>,
    on_delete: Callback<()>,
) -> impl IntoView {
    let key = malformed_metadata(&row).map_or_else(
        || json_row(&key_for(&row, &entity.primary_key)),
        |m| m.raw_key,
    );
    let pretty = serde_json::to_string_pretty(&Value::Object(row.clone())).unwrap_or_default();
    let file = format!("{}-row.json", entity.slug);
    let title = to_title_case(&entity.label);
    let download = StoredValue::new((file, pretty.clone()));
    let pretty = StoredValue::new(pretty);
    let key = StoredValue::new(key);
    view! {
        <Inspector
            title=title
            docked
            on_close
            actions=ViewFn::from(move || view! {
                <Button size=ButtonSize::Sm variant=ButtonVariant::Ghost icon=Icon::Download on_click=Callback::new(move |_| {
                    download.with_value(|(name, content)| download_file(name, content, "application/json"));
                })>"Download"</Button>
                <ConfirmButton
                    label="Delete row"
                    destructive=true
                    size=ButtonSize::Sm
                    variant=ButtonVariant::Ghost
                    icon=Icon::Trash
                    busy=deleting
                    on_confirm=on_delete
                />
            })
        >
            <section class="inspector-section">
                <div class="label">"Key"</div>
                <code class="mono small">{key.get_value()}</code>
            </section>
            <section class="inspector-section">
                <pre class="json-block">{pretty.get_value()}</pre>
            </section>
        </Inspector>
    }
}

#[component]
pub fn DataPage() -> impl IntoView {
    let wide = use_is_wide();
    let entities = RwSignal::new(Vec::<ManagedEntitySummary>::new());
    let storage = RwSignal::new(None::<ManagedDataSummary>);
    let selected_slug = RwSignal::new(String::new());
    let rows = RwSignal::new(Vec::<DataRow>::new());
    let loading_entities = RwSignal::new(true);
    let loading_rows = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let query = RwSignal::new(String::new());
    let sort = RwSignal::new(None::<SortState>);
    let detail_row = RwSignal::new(None::<DataRow>);
    let deleting_id = RwSignal::new(None::<String>);
    let rows_request = StoredValue::new(0u64);
    let toast = use_toast();

    let refresh_entities = move |report_error: bool| {
        spawn_detached(async move {
            match api::fetch_data_entities().await {
                Ok(response) => {
                    let first = response
                        .entities
                        .first()
                        .map(|e| e.slug.clone())
                        .unwrap_or_default();
                    entities.set(response.entities);
                    storage.set(Some(response.storage));
                    selected_slug.update(|current| {
                        if current.is_empty() {
                            *current = first;
                        }
                    });
                    error.set(None);
                }
                Err(e) => {
                    if report_error {
                        error.set(Some(e.message().to_owned()));
                    }
                }
            }
            loading_entities.set(false);
        });
    };
    refresh_entities(true);

    let selected = Memo::new(move |_| {
        let slug = selected_slug.get();
        entities.with(|list| list.iter().find(|e| e.slug == slug).cloned())
    });

    let load_rows = move || {
        let slug = selected_slug.get_untracked();
        if slug.is_empty() {
            return;
        }
        let request = rows_request.try_get_value().unwrap_or(0) + 1;
        rows_request.try_set_value(request);
        loading_rows.set(true);
        spawn_scoped(async move {
            let result = api::fetch_data_rows(&slug).await;
            if rows_request.try_get_value() != Some(request) {
                return;
            }
            match result {
                Ok(response) => {
                    sort.update(|current| {
                        if current.is_none() {
                            *current = default_sort(&response.rows);
                        }
                    });
                    rows.set(response.rows);
                    let summary = response.summary;
                    entities.update(|list| {
                        for entity in list.iter_mut() {
                            if entity.slug == summary.slug {
                                *entity = summary.clone();
                            }
                        }
                    });
                    error.set(None);
                }
                Err(e) => error.set(Some(e.message().to_owned())),
            }
            loading_rows.set(false);
        });
    };

    Effect::new(move |_| {
        selected_slug.track();
        rows.set(Vec::new());
        query.set(String::new());
        sort.set(None);
        detail_row.set(None);
        load_rows();
    });

    let columns = Memo::new(move |_| match selected.get() {
        Some(entity) => rows.with(|rows| columns_for(rows, &entity)),
        None => Vec::new(),
    });
    let column_widths = Memo::new(move |_| {
        let columns = columns.get();
        rows.with(|rows| {
            columns
                .iter()
                .filter(|c| c.as_str() != MALFORMED_ROW_KEY)
                .map(|c| (c.clone(), classify_column_width(c, rows)))
                .collect::<HashMap<_, _>>()
        })
    });
    let visible_rows = Memo::new(move |_| {
        let needle = locale_lowercase(query.get().trim());
        let mut filtered: Vec<DataRow> = rows.with(|rows| {
            if needle.is_empty() {
                rows.clone()
            } else {
                rows.iter()
                    .filter(|row| locale_lowercase(&json_row(row)).contains(&needle))
                    .cloned()
                    .collect()
            }
        });
        if let Some(sort) = sort.get() {
            filtered.sort_by(|a, b| {
                let result = compare_values(a.get(&sort.column), b.get(&sort.column));
                if sort.direction == Direction::Asc {
                    result
                } else {
                    result.reverse()
                }
            });
        }
        filtered
    });
    let paged = use_show_more(
        Signal::derive(move || visible_rows.get()),
        ROWS_PER_PAGE,
        Signal::derive(move || format!("{}|{}", selected_slug.get(), query.get())),
    );

    let download_rows = Callback::new(move |_| {
        let Some(entity) = selected.get_untracked() else {
            return;
        };
        let suffix = if query.get_untracked().trim().is_empty() {
            ""
        } else {
            "-filtered"
        };
        let rows = visible_rows.get_untracked();
        let content = serde_json::to_string_pretty(&rows).unwrap_or_default();
        download_file(
            &format!("{}{suffix}.json", entity.slug),
            &content,
            "application/json",
        );
    });
    let select_sort = move |column: String| {
        sort.update(|current| {
            *current = Some(match current {
                Some(state) if state.column == column => SortState {
                    column,
                    direction: if state.direction == Direction::Asc {
                        Direction::Desc
                    } else {
                        Direction::Asc
                    },
                },
                _ => SortState {
                    column,
                    direction: Direction::Asc,
                },
            });
        });
    };
    let delete_row = move |row: DataRow| {
        let Some(entity) = selected.get_untracked() else {
            return;
        };
        let key = key_for(&row, &entity.primary_key);
        let id = row_id(&row, &entity.primary_key);
        deleting_id.set(Some(id.clone()));
        spawn_detached(async move {
            match api::delete_data_row(&entity.slug, &key).await {
                Ok(_) => {
                    rows.update(|rows| rows.retain(|c| row_id(c, &entity.primary_key) != id));
                    entities.update(|list| {
                        for e in list.iter_mut() {
                            if e.slug == entity.slug {
                                e.count = e.count.saturating_sub(1);
                            }
                        }
                    });
                    detail_row.set(None);
                    toast.show("Row deleted", ToastKind::Info);
                    refresh_entities(false);
                }
                Err(e) => toast.show(e.message().to_owned(), ToastKind::Error),
            }
            deleting_id.set(None);
        });
    };

    let header_cells = move || {
        let pk = selected.get().map(|e| e.primary_key).unwrap_or_default();
        let widths = column_widths.get();
        columns
            .get()
            .into_iter()
            .map(|column| {
                let width = widths.get(&column).copied().unwrap_or("medium");
                let is_pk = pk.contains(&column);
                let sort_column = column.clone();
                let label_column = column.clone();
                let aria_column = column.clone();
                view! {
                    <th
                        aria-sort=move || match sort.get() {
                            Some(s) if s.column == aria_column => {
                                if s.direction == Direction::Asc { "ascending" } else { "descending" }
                            }
                            _ => "none",
                        }
                        class=format!("col-{width} {}", if is_pk { "col-pk" } else { "" })
                    >
                        <button type="button" class="th-sort" on:click=move |_| select_sort(sort_column.clone())>
                            {if column == MALFORMED_ROW_KEY { "Malformed record".to_owned() } else { column.clone() }}
                            {move || match sort.get() {
                                Some(s) if s.column == label_column => Some(view! {
                                    <span aria-hidden="true">{if s.direction == Direction::Asc { " ↑" } else { " ↓" }}</span>
                                }),
                                _ => None,
                            }}
                        </button>
                    </th>
                }
            })
            .collect_view()
    };
    let body_rows = move || {
        let Some(entity) = selected.get() else {
            return ().into_any();
        };
        let columns = columns.get();
        let widths = column_widths.get();
        paged
            .visible
            .get()
            .into_iter()
            .map(|row| {
                let id = row_id(&row, &entity.primary_key);
                let malformed = malformed_metadata(&row);
                let cells = columns
                    .iter()
                    .map(|column| {
                        if column == MALFORMED_ROW_KEY
                            && let Some(m) = &malformed
                        {
                            return view! {
                                <td class="text-fault" title=m.error.clone()>
                                    {format!("Malformed: {}", m.error)}
                                </td>
                            }
                            .into_any();
                        }
                        let (text, title) = display_value(row.get(column));
                        let width = widths.get(column).copied().unwrap_or("medium");
                        view! { <td class=format!("col-{width} mono") title=title>{text}</td> }.into_any()
                    })
                    .collect_view();
                let open_row = row.clone();
                let key_row = row.clone();
                let selected_id = id.clone();
                let class_id = id.clone();
                let entity_pk = entity.primary_key.clone();
                let class_pk = entity.primary_key.clone();
                let is_selected = move || {
                    detail_row.with(|d| d.as_ref().is_some_and(|d| row_id(d, &entity_pk) == selected_id))
                };
                let malformed_row = malformed.is_some();
                view! {
                    <tr
                        data-row="true"
                        tabindex="0"
                        class=move || {
                            let selected = detail_row.with(|d| d.as_ref().is_some_and(|d| row_id(d, &class_pk) == class_id));
                            match (selected, malformed_row) {
                                (true, _) => "clickable selected",
                                (false, true) => "clickable row-fault",
                                (false, false) => "clickable",
                            }
                        }
                        aria-selected=move || is_selected().to_string()
                        on:click=move |_| detail_row.set(Some(open_row.clone()))
                        on:keydown=move |ev: web_sys::KeyboardEvent| {
                            if ev.key() == "Enter" {
                                detail_row.set(Some(key_row.clone()));
                            }
                        }
                    >
                        {cells}
                    </tr>
                }
            })
            .collect_view()
            .into_any()
    };

    let entity_options = move || {
        entities
            .get()
            .into_iter()
            .map(|entity| {
                let slug = entity.slug.clone();
                view! {
                    <option value=entity.slug.clone() selected=move || selected_slug.get() == slug>
                        {format!("{} ({})", to_title_case(&entity.label), number_string(entity.count as f64))}
                    </option>
                }
            })
            .collect_view()
    };
    let entity_list = move || {
        entities
            .get()
            .into_iter()
            .map(|entity| {
                let slug = entity.slug.clone();
                let current_slug = slug.clone();
                let click_slug = slug.clone();
                view! {
                    <button
                        class=move || if selected_slug.get() == slug { "row dense entity-item selected" } else { "row dense entity-item" }
                        aria-current=move || (selected_slug.get() == current_slug).then_some("true")
                        type="button"
                        on:click=move |_| selected_slug.set(click_slug.clone())
                    >
                        <span class="row-main">
                            <span class="row-title truncate">{to_title_case(&entity.label)}</span>
                            <span class="row-sub num">{format_bytes(entity.storage_bytes as f64)}</span>
                        </span>
                        <span class="row-end num">{locale_number(entity.count as f64)}</span>
                    </button>
                }
            })
            .collect_view()
    };

    let docked = Signal::derive(move || wide.get());
    let detail = move || {
        let row = detail_row.get()?;
        let entity = selected.get()?;
        let id = row_id(&row, &entity.primary_key);
        let delete_target = row.clone();
        Some(view! {
            <RowInspector
                entity
                row
                docked
                deleting=Signal::derive(move || deleting_id.get().as_ref() == Some(&id))
                on_close=Callback::new(move |()| detail_row.set(None))
                on_delete=Callback::new(move |()| delete_row(delete_target.clone()))
            />
        })
    };

    let row_count = move || {
        let visible = visible_rows.with(Vec::len);
        let total = rows.with(Vec::len);
        if visible == total {
            format!("{} rows", locale_number(total as f64))
        } else {
            format!(
                "{} of {} rows",
                number_string(visible as f64),
                number_string(total as f64)
            )
        }
    };

    let browser = move || {
        let entity = selected.get()?;
        Some(view! {
            <div class="stack">
                <div class="section-head">
                    <div>
                        <h2 class="section-title">{to_title_case(&entity.label)}</h2>
                        <p class="small dim">{entity.description.clone()}</p>
                    </div>
                    <span class="section-meta mono">{entity.slug.clone()}</span>
                </div>
                {entity.warning.clone().filter(|w| !w.is_empty()).map(|w| view! { <InlineNote tone=Tone::Warn>{w}</InlineNote> })}
                <ReadoutBand cols=3 aria_label="Entity size">
                    <Readout label="Rows" value=locale_number(entity.count as f64)/>
                    <Readout label="Payload" value=format_bytes(entity.storage_bytes as f64)/>
                    <Readout label="Primary key" value=entity.primary_key.join(", ") size=ReadoutSize::M/>
                </ReadoutBand>
                <div class="toolbar">
                    <SearchField value=query on_input=Callback::new(move |v| query.set(v)) placeholder="Search every field" aria_label="Search rows" shortcut=true/>
                    <span class="small dim num">{row_count}</span>
                    <span class="spacer"></span>
                    <Button
                        size=ButtonSize::Sm
                        icon=Icon::Download
                        disabled=Signal::derive(move || visible_rows.with(Vec::is_empty))
                        title=Signal::derive(move || Some(if query.get().trim().is_empty() { "Download all rows as JSON".to_owned() } else { "Download the filtered rows as JSON".to_owned() }))
                        on_click=download_rows
                    >
                        "JSON"
                    </Button>
                </div>
                {move || error.get().map(|e| view! {
                    <ErrorState title="Rows could not load" raw=e retry=Callback::new(move |()| load_rows())/>
                })}
                {move || {
                    if loading_rows.get() && rows.with(Vec::is_empty) {
                        view! { <SkeletonRows count=8 label="Loading rows"/> }.into_any()
                    } else if visible_rows.with(Vec::is_empty) {
                        let message = if rows.with(Vec::is_empty) { "No rows in this entity." } else { "No rows match your search." };
                        view! { <EmptyState compact=true message/> }.into_any()
                    } else {
                        view! {
                            <div class="table-wrap panel data-table-wrap">
                                <table class="table dense data-table" data-primary-rows="true">
                                    <thead><tr>{header_cells}</tr></thead>
                                    <tbody>{body_rows}</tbody>
                                </table>
                            </div>
                        }
                        .into_any()
                    }
                }}
                {move || paged.has_more.get().then(|| view! {
                    <ShowMoreButton remaining=paged.remaining noun="rows" on_click=Callback::new(move |()| paged.show_more())/>
                })}
            </div>
        })
    };

    // Only these transitions swap the page; everything else updates in place.
    let page_state = Memo::new(move |_| {
        if loading_entities.get() {
            Some(None)
        } else if entities.with(Vec::is_empty) {
            error.get().map(Some)
        } else {
            None
        }
    });
    move || {
        match page_state.get() {
            Some(None) => {
                return view! { <SkeletonRows count=10 label="Loading data"/> }.into_any();
            }
            Some(Some(err)) => {
                return view! {
                    <ErrorState title="Data could not load" raw=err retry=Callback::new(move |()| refresh_entities(true)) page=true/>
                }
                .into_any();
            }
            None => {}
        }
        let lede = Signal::derive(move || {
            storage.get().map(|s| {
                format!(
                    "{} database, {} in entities.",
                    format_bytes(s.database_size_bytes as f64),
                    format_bytes(s.entity_storage_bytes as f64)
                )
            })
        });
        view! {
            <PageHead
                title="Data"
                lede
                actions=ViewFn::from(move || view! {
                    <Button
                        icon=Icon::Refresh
                        busy=loading_rows
                        on_click=Callback::new(move |_| {
                            load_rows();
                            refresh_entities(false);
                        })
                    >
                        "Refresh"
                    </Button>
                })
            />
            <div class=move || if detail_row.with(Option::is_some) && wide.get() { "data-layout docked" } else { "data-layout" }>
                <aside class="data-entities" aria-label="Entities">
                    <label class="label only-phone" for="data-entity-select">"Entity"</label>
                    <select
                        id="data-entity-select"
                        class="select only-phone"
                        prop:value=move || selected_slug.get()
                        on:change=move |ev| selected_slug.set(event_target_value(&ev))
                    >
                        {entity_options}
                    </select>
                    <nav class="rows hide-phone" aria-label="Entities">{entity_list}</nav>
                </aside>
                <section class="data-browser">{browser}</section>
                {detail}
            </div>
        }
        .into_any()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn page_helpers() {
        assert_eq!(format_bytes(512.0), "512 B");
        assert_eq!(format_bytes(2048.0), "2.0 KB");
        assert_eq!(format_bytes(20.0 * 1024.0 * 1024.0), "20 MB");
        let row: DataRow =
            serde_json::from_value(json!({"id": "a", "n": 1.0, "o": {"x": [1, 2]}})).unwrap();
        assert_eq!(json_row(&row), r#"{"id":"a","n":1,"o":{"x":[1,2]}}"#);
        assert_eq!(display_value(row.get("o")).0, "{1 field}");
        assert_eq!(
            display_value(row.get("missing")),
            (String::new(), "Missing".into())
        );
        assert_eq!(row_id(&row, &["id".into()]), r#"{"id":"a"}"#);
        let rows = vec![row];
        assert_eq!(default_sort(&rows), None);
    }
}
