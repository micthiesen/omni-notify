//! Browse and delete stored entity rows.

use std::collections::HashMap;

use leptos::prelude::*;
use omni_web_kit::api::{self, DataRow, ManagedDataSummary, ManagedEntitySummary};
use omni_web_kit::components::{ShowMoreButton, Toast, ToastKind, use_show_more, use_toast};
use omni_web_kit::hooks::use_modal;
use omni_web_kit::task::{spawn_detached, spawn_scoped};
use omni_web_kit::utils::download::download_file;
use omni_web_kit::utils::format::to_title_case;
use omni_web_kit::utils::js::{
    locale_compare, locale_compare_numeric, locale_lowercase, number_string, to_fixed, utf16_len,
    utf16_slice,
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
fn RowDetail(
    entity: ManagedEntitySummary,
    row: DataRow,
    #[prop(into)] deleting: Signal<bool>,
    on_close: Callback<()>,
    on_delete: Callback<()>,
) -> impl IntoView {
    let modal_ref = use_modal(move || on_close.run(()));
    let key = json_row(&key_for(&row, &entity.primary_key));
    let pretty = serde_json::to_string_pretty(&Value::Object(row.clone())).unwrap_or_default();
    view! {
        <div class="modal-root">
            <button
                class="modal-backdrop"
                tabindex="-1"
                type="button"
                on:click=move |_| on_close.run(())
                aria-label="Close"
            ></button>
            <div
                class="data-detail-modal"
                node_ref=modal_ref
                tabindex="-1"
                role="dialog"
                aria-modal="true"
                aria-label="Row detail"
            >
                <div class="data-detail-header">
                    <div>
                        <div class="data-detail-label">{to_title_case(&entity.label)}</div>
                        <code>{key}</code>
                    </div>
                    <button
                        class="log-modal-close"
                        type="button"
                        on:click=move |_| on_close.run(())
                        aria-label="Close"
                    >
                        "✕"
                    </button>
                </div>
                <pre class="data-detail-json">{pretty}</pre>
                <div class="data-detail-footer">
                    <button
                        class="data-delete-btn"
                        type="button"
                        disabled=move || deleting.get()
                        on:click=move |_| on_delete.run(())
                    >
                        {move || if deleting.get() { "Deleting…" } else { "Delete Row" }}
                    </button>
                </div>
            </div>
        </div>
    }
}

#[component]
pub fn DataPage() -> impl IntoView {
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

    let download_rows = move |_| {
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
    };
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
        let label = malformed_metadata(&row).map_or_else(|| json_row(&key), |m| m.raw_key);
        let message = format!(
            "Delete {} row {label}? This cannot be undone.",
            entity.label
        );
        if !window().confirm_with_message(&message).unwrap_or(false) {
            return;
        }
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
                        class=format!("data-col-{width} {}", if is_pk { "data-pk-column" } else { "" })
                    >
                        <button type="button" on:click=move |_| select_sort(sort_column.clone())>
                            {if column == MALFORMED_ROW_KEY { "Malformed Record".to_owned() } else { column.clone() }}
                            {move || match sort.get() {
                                Some(s) if s.column == label_column => Some(view! {
                                    <span>{if s.direction == Direction::Asc { " ↑" } else { " ↓" }}</span>
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
                                <td class="data-malformed-cell" title=m.error.clone()>
                                    {format!("Malformed: {}", m.error)}
                                </td>
                            }
                            .into_any();
                        }
                        let (text, title) = display_value(row.get(column));
                        let width = widths.get(column).copied().unwrap_or("medium");
                        view! { <td class=format!("data-col-{width}") title=title>{text}</td> }.into_any()
                    })
                    .collect_view();
                let open_row = row.clone();
                let view_row = row.clone();
                let delete_target = row.clone();
                let busy_id = id.clone();
                let disabled_id = id.clone();
                view! {
                    <tr
                        class=if malformed.is_some() { "data-row-malformed" } else { "" }
                        on:click=move |_| detail_row.set(Some(open_row.clone()))
                    >
                        {cells}
                        <td class="data-actions-column">
                            <button
                                class="data-view-btn"
                                type="button"
                                on:click=move |event| {
                                    event.stop_propagation();
                                    detail_row.set(Some(view_row.clone()));
                                }
                            >
                                "View"
                            </button>
                            <button
                                class="data-trash-btn"
                                type="button"
                                title="Delete row"
                                aria-label="Delete row"
                                disabled=move || deleting_id.get().as_ref() == Some(&disabled_id)
                                on:click=move |event| {
                                    event.stop_propagation();
                                    delete_row(delete_target.clone());
                                }
                            >
                                {move || if deleting_id.get().as_ref() == Some(&busy_id) { "…" } else { "×" }}
                            </button>
                        </td>
                    </tr>
                }
            })
            .collect_view()
            .into_any()
    };

    let selected_present = Memo::new(move |_| selected.with(Option::is_some));
    let browser = move || {
        selected_present.get().then(|| {
            let entity = move || selected.get().unwrap_or_else(empty_entity);
            let row_count = move || {
                let visible = visible_rows.with(Vec::len);
                let total = rows.with(Vec::len);
                if visible == total {
                    format!("{total} rows")
                } else {
                    format!("{visible} of {total} rows")
                }
            };
            view! {
                <div class="data-browser-heading">
                    <div>
                        <h2>{move || to_title_case(&entity().label)}</h2>
                        <p>{move || entity().description}</p>
                    </div>
                    <div class="data-browser-aside">
                        <code>{move || entity().slug}</code>
                        <span class="data-selected-size">
                            {move || format!("{} payload", format_bytes(entity().storage_bytes as f64))}
                        </span>
                    </div>
                </div>
                {move || {
                    entity()
                        .warning
                        .filter(|w| !w.is_empty())
                        .map(|w| view! { <div class="data-warning">{w}</div> })
                }}
                <div class="data-controls">
                    <input
                        class="data-search"
                        type="search"
                        prop:value=move || query.get()
                        on:input=move |ev| query.set(event_target_value(&ev))
                        placeholder="Search every field…"
                        aria-label="Search rows"
                    />
                    <span class="data-row-count">{row_count}</span>
                    <button
                        class="data-download-btn"
                        type="button"
                        disabled=move || visible_rows.with(Vec::is_empty)
                        title=move || {
                            if query.get().trim().is_empty() {
                                "Download all rows as JSON"
                            } else {
                                "Download the filtered rows as JSON"
                            }
                        }
                        on:click=download_rows
                    >
                        "Download JSON"
                    </button>
                </div>
                {move || error.get().map(|e| view! { <div class="error-inline">{e}</div> })}
                <div class="data-table-wrap">
                    {move || {
                        if loading_rows.get() && rows.with(Vec::is_empty) {
                            view! { <div class="loading-inline">"Loading rows…"</div> }.into_any()
                        } else if visible_rows.with(Vec::is_empty) {
                            let text = if rows.with(Vec::is_empty) {
                                "No rows in this entity."
                            } else {
                                "No rows match your search."
                            };
                            view! { <div class="data-empty">{text}</div> }.into_any()
                        } else {
                            view! {
                                <table class="data-table">
                                    <thead>
                                        <tr>
                                            {header_cells}
                                            <th class="data-actions-column">
                                                <span class="sr-only">"Actions"</span>
                                            </th>
                                        </tr>
                                    </thead>
                                    <tbody>{body_rows}</tbody>
                                </table>
                            }
                            .into_any()
                        }
                    }}
                </div>
                {move || {
                    paged.has_more.get().then(|| view! {
                        <ShowMoreButton
                            remaining=paged.remaining
                            on_click=Callback::new(move |()| paged.show_more())
                        />
                    })
                }}
            }
        })
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
                let click_slug = slug.clone();
                view! {
                    <button
                        class=move || {
                            format!(
                                "data-entity-item {}",
                                if selected_slug.get() == slug { "active" } else { "" },
                            )
                        }
                        type="button"
                        on:click=move |_| selected_slug.set(click_slug.clone())
                    >
                        <span class="data-entity-name">{to_title_case(&entity.label)}</span>
                        <span class="data-entity-meta">
                            <span>{format_bytes(entity.storage_bytes as f64)}</span>
                            <span class="data-entity-count">{number_string(entity.count as f64)}</span>
                        </span>
                    </button>
                }
            })
            .collect_view()
    };

    let detail = move || {
        let row = detail_row.get()?;
        let entity = selected.get()?;
        let id = row_id(&row, &entity.primary_key);
        let delete_target = row.clone();
        Some(view! {
            <RowDetail
                entity
                row
                deleting=Signal::derive(move || deleting_id.get().as_ref() == Some(&id))
                on_close=Callback::new(move |()| detail_row.set(None))
                on_delete=Callback::new(move |()| delete_row(delete_target.clone()))
            />
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
            Some(None) => return view! { <div class="loading">"Loading data…"</div> }.into_any(),
            Some(Some(err)) => return view! { <div class="error">{err}</div> }.into_any(),
            None => {}
        }
        view! {
            <div class="page-header data-page-header">
                <div class="page-header-stack">
                    <h1>"Data"</h1>
                    <p class="page-subtitle">"Browse and remove records stored by mitools Entities."</p>
                </div>
                <div class="data-header-actions">
                    {move || storage.get().map(|storage| view! {
                        <div class="data-storage-summary">
                            <span title="SQLite allocated pages, including relational tables and indexes">
                                <strong>{format_bytes(storage.database_size_bytes as f64)}</strong>
                                " database"
                            </span>
                            <span title="Encoded payload bytes across registered mitools Entities">
                                <strong>{format_bytes(storage.entity_storage_bytes as f64)}</strong>
                                " entities"
                            </span>
                        </div>
                    })}
                    <button
                        class="run-btn"
                        type="button"
                        on:click=move |_| {
                            load_rows();
                            refresh_entities(false);
                        }
                        disabled=move || loading_rows.get()
                    >
                        {move || if loading_rows.get() { "Refreshing…" } else { "Refresh" }}
                    </button>
                </div>
            </div>
            <div class="data-layout">
                <aside class="data-entity-panel" aria-label="Entities">
                    <label class="data-mobile-select-label" for="data-entity-select">"Entity"</label>
                    <select
                        id="data-entity-select"
                        class="data-entity-select"
                        prop:value=move || selected_slug.get()
                        on:change=move |ev| selected_slug.set(event_target_value(&ev))
                    >
                        {entity_options}
                    </select>
                    <div class="data-entity-list">{entity_list}</div>
                </aside>
                <section class="data-browser">{browser}</section>
            </div>
            {detail}
            <Toast toast=toast.toast/>
        }
        .into_any()
    }
}

fn empty_entity() -> ManagedEntitySummary {
    ManagedEntitySummary {
        slug: String::new(),
        label: String::new(),
        description: String::new(),
        warning: None,
        primary_key: Vec::new(),
        count: 0,
        storage_bytes: 0,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn helpers_match_the_ts_page() {
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
