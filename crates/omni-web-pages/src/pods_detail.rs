//! One PressPods episode: player with chapter seek, transcript, chunk
//! diagnostics, retriever attempts and costs.

use leptos::html::Audio;
use leptos::prelude::*;
use omni_api::presspods::{
    PressPodsChapter, PressPodsChunkStat, PressPodsEpisodeDetail, PressPodsRetrieverAttempt,
    PressPodsTokenCounts,
};
use omni_api::runs::Run;
use omni_web_kit::api;
use omni_web_kit::chrome::use_page_label;
use omni_web_kit::components::{
    Button, ButtonLink, ButtonSize, ButtonVariant, Chip, ConfirmButton, Disclosure, ErrorState,
    Glyph, Icon, IconSize, LogViewer, Meter, Panel, Readout, ReadoutSize, Skeleton, SkeletonKind,
    Status, StatusKind, Tag, ToastKind, Tone, use_toast,
};
use omni_web_kit::router::navigate;
use omni_web_kit::task::{spawn_detached, spawn_scoped};
use omni_web_kit::utils::format::{format_absolute_with_year, format_cents};
use omni_web_kit::utils::js::{
    js_round, locale_number, number_string, to_fixed, utf16_len, utf16_slice,
};

use crate::pods::{EpisodeArt, attempt_success, episode_source, format_audio_duration, open_logs};

// Higgs duration-band bounds (the fallback verifier), scaled for the +10%
// playback speed-up. STT `coverage` is authoritative when present; duration
// is only meaningful without it.
const SEC_PER_CHAR_MIN: f64 = 0.03 / 1.1;
const SEC_PER_CHAR_MAX: f64 = 0.15 / 1.1;
// Mirror `DEFAULT_CONTENT_BOUNDS` in `omni-presspods`: the backend's accept test
// rejects on either bound, so the UI flag must too.
const MIN_COVERAGE: f64 = 0.75;
const MIN_COVERAGE_WITH_HEALTHY_RATIO: f64 = 0.68;
const MIN_HEALTHY_WORD_RATIO: f64 = 0.78;
const MAX_HEALTHY_RATIO_EXPECTED_WORDS: f64 = 60.0;
const MAX_WORD_RATIO: f64 = 1.8;

/// `512 B`, `1.5 KB`, `3.2 MB`, `1.0 GB`.
pub fn format_bytes(bytes: i64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let units = ["KB", "MB", "GB"];
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < units.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{} {}", to_fixed(value, 1), units[unit])
}

/// Whether a chunk needs review: coverage bounds when recorded, else retries
/// or a duration outside the expected band.
pub fn is_chunk_problematic(chunk: &PressPodsChunkStat) -> bool {
    if let Some(coverage) = chunk.coverage {
        if chunk.word_ratio.is_some_and(|r| r > MAX_WORD_RATIO) {
            return true;
        }
        // Older rows do not store expectedWords; only use the relaxed verdict
        // when the persisted stat proves it.
        let secondary_pass = coverage >= MIN_COVERAGE_WITH_HEALTHY_RATIO
            && chunk
                .word_ratio
                .is_some_and(|r| r >= MIN_HEALTHY_WORD_RATIO)
            && chunk
                .expected_words
                .is_some_and(|w| w <= MAX_HEALTHY_RATIO_EXPECTED_WORDS);
        return coverage < MIN_COVERAGE && !secondary_pass;
    }
    chunk.attempts > 1
        || chunk.sec_per_char < SEC_PER_CHAR_MIN
        || chunk.sec_per_char > SEC_PER_CHAR_MAX
}

/// JS `\s` (Rust's `is_whitespace` plus the BOM).
fn js_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// A JS `.` (anything but a line terminator).
fn js_dot(c: char) -> bool {
    !matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// A run of plain text or `*emphasis*` / `_emphasis_`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    Em(String),
}

/// `/([*_])(?=\S)(.+?)(?<=\S)\1/g` over `text`.
pub fn parse_inline(text: &str) -> Vec<Inline> {
    let chars: Vec<char> = text.chars().collect();
    let mut parts = Vec::new();
    let mut last = 0;
    let mut i = 0;
    while i < chars.len() {
        let marker = chars[i];
        let close = if (marker == '*' || marker == '_')
            && chars.get(i + 1).is_some_and(|c| !js_space(*c))
        {
            let mut found = None;
            let mut j = i + 1;
            while j < chars.len() && js_dot(chars[j]) {
                if j > i + 1 && chars[j] == marker && !js_space(chars[j - 1]) {
                    found = Some(j);
                    break;
                }
                j += 1;
            }
            found
        } else {
            None
        };
        match close {
            Some(j) => {
                if i > last {
                    parts.push(Inline::Text(chars[last..i].iter().collect()));
                }
                parts.push(Inline::Em(chars[i + 1..j].iter().collect()));
                last = j + 1;
                i = j + 1;
            }
            None => i += 1,
        }
    }
    if last < chars.len() {
        parts.push(Inline::Text(chars[last..].iter().collect()));
    }
    parts
}

/// A transcript block: a `## ` heading or a paragraph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TranscriptBlock {
    Heading(String),
    Paragraph(String),
}

/// `/^##\s+(.+?)\s*$/` on one line: the heading text.
fn heading_text(line: &str) -> Option<String> {
    let rest: Vec<char> = line.strip_prefix("##")?.chars().collect();
    if !rest.iter().all(|c| js_dot(*c)) {
        return None;
    }
    let lead = rest.iter().take_while(|c| js_space(**c)).count();
    if lead == 0 {
        return None;
    }
    if lead < rest.len() {
        let body: String = rest[lead..].iter().collect();
        return Some(body.trim_end_matches(js_space).to_owned());
    }
    // All whitespace: `\s+` backs off one character for `.+?`.
    (lead >= 2).then(|| rest[lead - 1].to_string())
}

/// `## ` markers become headings; blank-line separated blocks become
/// paragraphs (lines joined with spaces).
pub fn transcript_blocks(content: &str) -> Vec<TranscriptBlock> {
    let mut blocks = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let flush = |para: &mut Vec<&str>, blocks: &mut Vec<TranscriptBlock>| {
        let text = para.join(" ");
        let text = text.trim();
        if !text.is_empty() {
            blocks.push(TranscriptBlock::Paragraph(text.to_owned()));
        }
        para.clear();
    };
    for line in content.split('\n') {
        if let Some(heading) = heading_text(line) {
            flush(&mut para, &mut blocks);
            blocks.push(TranscriptBlock::Heading(heading));
        } else if line.trim().is_empty() {
            flush(&mut para, &mut blocks);
        } else {
            para.push(line);
        }
    }
    flush(&mut para, &mut blocks);
    blocks
}

/// The chapter playing at `seconds`: the last one that has started.
pub fn current_chapter(chapters: &[PressPodsChapter], seconds: f64) -> Option<usize> {
    chapters
        .iter()
        .rposition(|chapter| chapter.start_time_seconds <= seconds + 0.25)
}

/// Transcripts longer than this start collapsed behind "Read full transcript".
const TRANSCRIPT_PREVIEW_CHARS: usize = 1800;

fn render_inline(text: &str) -> impl IntoView + use<> {
    parse_inline(text)
        .into_iter()
        .map(|part| match part {
            Inline::Text(text) => text.into_any(),
            Inline::Em(text) => view! { <em>{text}</em> }.into_any(),
        })
        .collect_view()
}

fn transcript_body(content: &str) -> impl IntoView + use<> {
    transcript_blocks(content)
        .into_iter()
        .map(|block| match block {
            TranscriptBlock::Heading(text) => view! { <h3>{render_inline(&text)}</h3> }.into_any(),
            TranscriptBlock::Paragraph(text) => view! { <p>{render_inline(&text)}</p> }.into_any(),
        })
        .collect_view()
}

fn attempt_name(attempt: &PressPodsRetrieverAttempt) -> &str {
    match attempt {
        PressPodsRetrieverAttempt::Success { name, .. }
        | PressPodsRetrieverAttempt::Failure { name, .. } => name,
    }
}

fn retriever_row(attempt: &PressPodsRetrieverAttempt, is_winner: bool) -> impl IntoView + use<> {
    let success = attempt_success(attempt);
    let detail = match attempt {
        PressPodsRetrieverAttempt::Success {
            content_rating,
            text_chars,
            ..
        } => format!(
            "Rating {}/10 · {} chars",
            number_string(*content_rating),
            locale_number(*text_chars as f64)
        ),
        PressPodsRetrieverAttempt::Failure { error, .. } => error.clone(),
    };
    view! {
        <div class="row">
            {if success {
                view! { <Status kind=StatusKind::Ok dot_only=true label="Succeeded"/> }.into_any()
            } else {
                view! { <Status kind=StatusKind::Fault dot_only=true label="Failed"/> }.into_any()
            }}
            <div class="row-main">
                <span class="row-title">{attempt_name(attempt).to_owned()}</span>
                <span class=if success { "row-sub" } else { "row-sub text-fault" } title=detail.clone()>
                    {detail.clone()}
                </span>
            </div>
            {is_winner.then(|| view! { <span class="row-end"><Tag tone=Tone::Signal>"Winner"</Tag></span> })}
        </div>
    }
}

#[component]
fn ChunkRow(chunk: PressPodsChunkStat, seek: Callback<f64>) -> impl IntoView {
    let expanded = RwSignal::new(false);
    let problematic = is_chunk_problematic(&chunk);
    let long = utf16_len(&chunk.text) > 240;
    let text = chunk.text.clone();
    let preview = move || {
        if long && !expanded.get() {
            format!("{}…", utf16_slice(&text, 240))
        } else {
            text.clone()
        }
    };
    let resplit = chunk.resplit.unwrap_or(false).then(|| {
        let depth = chunk
            .resplit_depth
            .filter(|d| *d > 1)
            .map(|d| format!(" ×{d}"))
            .unwrap_or_default();
        view! {
            <Tag title="A larger chunk kept failing verification and was re-split into smaller pieces to recover">
                "re-split"
                {depth}
            </Tag>
        }
    });
    let start = chunk.start_time_seconds;
    let at = format_audio_duration(Some(start)).unwrap_or_default();
    let coverage = chunk.coverage.map_or_else(
        || "—".to_owned(),
        |c| format!("{}%", number_string(js_round(c * 100.0))),
    );
    view! {
        <tr class=if problematic { "flagged" } else { "" }>
            <td class="numeric dim">{chunk.index}</td>
            <td class="grow">
                <div class="pod-chunk-head">
                    {problematic.then(|| view! { <Tag tone=Tone::Warn>"needs review"</Tag> })}
                    {resplit}
                    {chunk
                        .section_title
                        .clone()
                        .filter(|t| !t.is_empty())
                        .map(|t| view! { <span class="pod-chunk-section truncate">{t}</span> })}
                </div>
                <p class="pod-chunk-text">{preview}</p>
                {long.then(|| view! {
                    <button
                        type="button"
                        class="textlink"
                        aria-expanded=move || expanded.get().to_string()
                        on:click=move |_| expanded.update(|v| *v = !*v)
                    >
                        {move || if expanded.get() { "Show less" } else { "Show full text" }}
                    </button>
                })}
            </td>
            <td class="numeric">
                <button
                    type="button"
                    class="textlink num"
                    aria-label=format!("Play chunk {} from {at}", chunk.index)
                    on:click=move |_| seek.run(start)
                >
                    {at.clone()}
                </button>
            </td>
            <td class="numeric hide-below-desk">{format!("{}s", to_fixed(chunk.duration_seconds, 1))}</td>
            <td class="numeric hide-below-wide">{locale_number(chunk.char_count as f64)}</td>
            <td class="numeric hide-below-wide">{to_fixed(chunk.sec_per_char, 3)}</td>
            <td class="numeric hide-below-desk">{chunk.attempts}</td>
            <td class="numeric hide-below-desk">{coverage}</td>
        </tr>
    }
}

fn chunk_table(
    chunks: Vec<PressPodsChunkStat>,
    flagged_only: RwSignal<bool>,
    seek: Callback<f64>,
) -> impl IntoView {
    view! {
        <div class="table-wrap">
            <table class="table dense pod-chunks">
                <thead>
                    <tr>
                        <th class="numeric">"#"</th>
                        <th>"Text"</th>
                        <th class="numeric">"Start"</th>
                        <th class="numeric hide-below-desk">"Length"</th>
                        <th class="numeric hide-below-wide">"Chars"</th>
                        <th class="numeric hide-below-wide">"s/char"</th>
                        <th class="numeric hide-below-desk">"Tries"</th>
                        <th class="numeric hide-below-desk">"Coverage"</th>
                    </tr>
                </thead>
                <tbody>
                    {move || {
                        let only = flagged_only.get();
                        chunks
                            .iter()
                            .filter(|c| !only || is_chunk_problematic(c))
                            .cloned()
                            .map(|chunk| view! { <ChunkRow chunk seek/> })
                            .collect_view()
                    }}
                </tbody>
            </table>
        </div>
    }
}

fn cost_section(episode: &PressPodsEpisodeDetail) -> impl IntoView + use<> {
    let Some(costs) = episode.costs.clone() else {
        return view! { <p class="panel-body dim">"No cost data recorded."</p> }.into_any();
    };
    let dash = || "—".to_owned();
    let keys = cost_keys(episode);
    let largest = costs
        .detail_cents
        .0
        .iter()
        .map(|(_, v)| *v)
        .fold(0.0_f64, f64::max);
    let rows = keys
        .into_iter()
        .map(|key| {
            let tokens: Option<PressPodsTokenCounts> = costs
                .detail_tokens
                .0
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.clone());
            let chars = costs.detail_chars.0.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
            let cents = costs.detail_cents.0.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
            let usage = [
                tokens.map(|t| {
                    format!(
                        "{} / {} tokens",
                        locale_number(t.input),
                        locale_number(t.output)
                    )
                }),
                chars.map(|c| format!("{} chars", locale_number(c))),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
            view! {
                <tr>
                    <td class="grow">
                        <span class="mono">{key.clone()}</span>
                        {(!usage.is_empty()).then(|| view! { <span class="pod-cost-usage">{usage}</span> })}
                    </td>
                    <td class="hide-below-desk">
                        {cents.filter(|_| largest > 0.0).map(|c| view! {
                            <Meter value=c max=largest width=72 label=format!("{key} share")/>
                        })}
                    </td>
                    <td class="numeric">{format_cents(cents).unwrap_or_else(dash)}</td>
                </tr>
            }
        })
        .collect_view();
    view! {
        <div class="pod-cost-sum">
            <Readout size=ReadoutSize::M label="LLM" value=format_cents(Some(costs.llm_cents)).unwrap_or_else(dash)/>
            <Readout size=ReadoutSize::M label="Speech" value=format_cents(Some(costs.tts_cents)).unwrap_or_else(dash)/>
            <Readout size=ReadoutSize::M label="Total" value=format_cents(episode.episode.cost_cents).unwrap_or_else(dash)/>
        </div>
        <div class="table-wrap">
            <table class="table dense">
                <tbody>{rows}</tbody>
            </table>
        </div>
    }
    .into_any()
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// `/pods/:id`.
#[component]
pub fn PodsDetailPage(#[prop(into)] id: String) -> impl IntoView {
    let episode = RwSignal::new(None::<PressPodsEpisodeDetail>);
    let error = RwSignal::new(None::<String>);
    let transcript_expanded = RwSignal::new(false);
    let audio_ref = NodeRef::<Audio>::new();
    let position = RwSignal::new(0.0_f64);
    let retrying = RwSignal::new(false);
    let deleting = RwSignal::new(false);
    let flagged_only = RwSignal::new(false);
    let log_run = RwSignal::new(None::<Run>);
    let toast = use_toast();

    use_page_label(move || episode.with(|e| e.as_ref().map(|e| e.episode.title.clone())));

    let load = move || {
        let id = id.clone();
        spawn_scoped(async move {
            match api::fetch_press_pods_episode(&id).await {
                Ok(res) => {
                    episode.set(Some(res.episode));
                    error.set(None);
                }
                Err(err) => error.set(Some(err.message().to_owned())),
            }
        });
    };
    load();

    let on_retry = Callback::new(move |()| {
        let Some(episode_id) =
            episode.with_untracked(|e| e.as_ref().map(|e| e.episode.episode_id.clone()))
        else {
            return;
        };
        if retrying.get_untracked() {
            return;
        }
        retrying.set(true);
        spawn_detached(async move {
            match api::retry_press_pods_episode(&episode_id).await {
                // A successful regeneration replaces this episode row once the
                // new one lands, so send the user back to the list where the
                // in-progress job is visible.
                Ok(_) => {
                    toast.show("Re-queued for regeneration", ToastKind::Info);
                    navigate("/pods");
                }
                Err(err) => toast.show(err.message(), ToastKind::Error),
            }
            retrying.set(false);
        });
    });

    let on_delete = Callback::new(move |()| {
        let Some(episode_id) =
            episode.with_untracked(|e| e.as_ref().map(|e| e.episode.episode_id.clone()))
        else {
            return;
        };
        if deleting.get_untracked() {
            return;
        }
        deleting.set(true);
        spawn_detached(async move {
            match api::delete_press_pods_episode(&episode_id).await {
                Ok(_) => {
                    toast.show("Episode deleted", ToastKind::Info);
                    navigate("/pods");
                }
                Err(err) => {
                    toast.show(err.message(), ToastKind::Error);
                    deleting.set(false);
                }
            }
        });
    });

    let seek = Callback::new(move |seconds: f64| {
        if let Some(audio) = audio_ref.get_untracked() {
            audio.set_current_time(seconds);
            position.set(seconds);
            let _ = audio.focus();
        }
    });
    let sync_position = move || {
        if let Some(audio) = audio_ref.get_untracked() {
            position.set(audio.current_time());
        }
    };

    let page = move || {
        if let Some(message) = error.get() {
            let retry = load.clone();
            return view! {
                <ErrorState
                    page=true
                    title="This episode did not load"
                    raw=message
                    retry=Callback::new(move |()| retry())
                    link=("All episodes".to_owned(), "/pods".to_owned())
                />
            }
            .into_any();
        }
        let Some(detail) = episode.get() else {
            return view! {
                <div class="pod-head" role="status" aria-label="Loading episode">
                    <span class="pod-art"><span class="skel pod-art-skel"></span></span>
                    <div class="pod-head-text">
                        <Skeleton width="20%"/>
                        <Skeleton kind=SkeletonKind::Title width="80%"/>
                        <Skeleton width="40%"/>
                    </div>
                </div>
            }
            .into_any();
        };
        let ep = detail.episode.clone();
        let source = episode_source(&ep);
        let problem_count = detail
            .chunks
            .as_ref()
            .map_or(0, |c| c.iter().filter(|c| is_chunk_problematic(c)).count());
        let gender = detail
            .author_gender
            .clone()
            .filter(|g| !g.is_empty())
            .map(|g| format!(" ({g})"))
            .unwrap_or_default();
        let byline = [
            ep.author
                .clone()
                .filter(|a| !a.is_empty())
                .map(|a| format!("{a}{gender}")),
            ep.publication.clone().filter(|p| !p.is_empty()),
            ep.published_at
                .map(|p| format!("Published {}", format_absolute_with_year(p as f64))),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        let voice = ep
            .voice_name
            .clone()
            .filter(|v| !v.is_empty())
            .map(
                |voice| match detail.voice_provider.clone().filter(|p| !p.is_empty()) {
                    Some(provider) => format!("{voice} · {provider}"),
                    None => voice,
                },
            );
        let duration = format_audio_duration(ep.duration_seconds);
        let cost = format_cents(ep.cost_cents);

        let chapters = ep.chapters.clone().unwrap_or_default();
        let current = {
            let chapters = chapters.clone();
            Memo::new(move |_| current_chapter(&chapters, position.get()))
        };
        let now_playing = {
            let chapters = chapters.clone();
            let source = source.clone();
            move || {
                current
                    .get()
                    .and_then(|i| chapters.get(i))
                    .map_or_else(|| source.clone(), |c| c.title.clone())
            }
        };
        let body_class = if chapters.is_empty() {
            "pod-body no-chapters"
        } else {
            "pod-body"
        };
        let chapter_panel = (!chapters.is_empty()).then(|| {
            let count = chapters.len();
            view! {
                <Panel title="Chapters" class="pod-chapters" head_end=ViewFn::from(move || view! { <span class="num">{count}</span> })>
                    <ol class="chapter-list">
                        {chapters
                            .into_iter()
                            .enumerate()
                            .map(|(index, chapter)| {
                                let at = format_audio_duration(Some(chapter.start_time_seconds))
                                    .unwrap_or_else(|| "null".to_owned());
                                let start = chapter.start_time_seconds;
                                let here = Memo::new(move |_| current.get() == Some(index));
                                view! {
                                    <li>
                                        <button
                                            type="button"
                                            class=move || if here.get() { "chapter current" } else { "chapter" }
                                            aria-current=move || here.get().then_some("true")
                                            aria-label=format!("Jump to {} at {at}", chapter.title)
                                            on:click=move |_| seek.run(start)
                                        >
                                            <span class="num">{at.clone()}</span>
                                            <span>{chapter.title.clone()}</span>
                                        </button>
                                    </li>
                                }
                            })
                            .collect_view()}
                    </ol>
                </Panel>
            }
        });

        let long_transcript = detail.content.chars().count() > TRANSCRIPT_PREVIEW_CHARS;
        let words = detail.content.split_whitespace().count();
        let mut facts = vec![("Created", format_absolute_with_year(ep.created_at as f64))];
        facts.push(("File size", format_bytes(ep.file_bytes)));
        if ep.synthesized_seconds.is_some() {
            facts.push((
                "Synthesized",
                format_audio_duration(ep.synthesized_seconds).unwrap_or_default(),
            ));
        }
        if let Some(seconds) = ep.retriever_seconds {
            facts.push(("Retrieval", format!("{}s", to_fixed(seconds, 1))));
        }
        if let Some(name) = ep.retriever_name.clone().filter(|n| !n.is_empty()) {
            facts.push(("Retriever", name));
        }

        let chunk_body = match detail.chunks.clone() {
            None => view! {
                <p class="panel-body dim">"This episode was synthesized before per-chunk stats were recorded."</p>
            }
            .into_any(),
            Some(chunks) if chunks.is_empty() => {
                view! { <p class="panel-body dim">"No chunk data recorded."</p> }.into_any()
            }
            Some(chunks) => chunk_table(chunks, flagged_only, seek).into_any(),
        };
        let chunk_count = detail.chunks.as_ref().map_or(0, Vec::len);
        let processing_meta = if problem_count > 0 {
            format!(
                "{} · {problem_count} flagged",
                plural(chunk_count, "chunk", "chunks")
            )
        } else {
            plural(chunk_count, "chunk", "chunks")
        };
        let retrievers = ep
            .retriever_attempts
            .clone()
            .filter(|a| !a.is_empty())
            .map(|attempts| {
                let winner = ep.retriever_name.clone();
                view! {
                    <Panel title="Retriever attempts">
                        <div class="rows dense">
                            {attempts
                                .iter()
                                .map(|attempt| {
                                    retriever_row(attempt, winner.as_deref() == Some(attempt_name(attempt)))
                                })
                                .collect_view()}
                        </div>
                    </Panel>
                }
            });
        let run_id = ep.run_id.clone().filter(|r| !r.is_empty());
        let title = ep.title.clone();

        view! {
            <header class="pod-head">
                <EpisodeArt src=ep.lead_image_url.clone() source=source.clone()/>
                <div class="pod-head-text">
                    <a class="pod-source" href=ep.article_url.clone() target="_blank" rel="noreferrer">
                        {source.clone()}
                        <Glyph icon=Icon::External size=IconSize::Small/>
                    </a>
                    <h1 class="page-title">{ep.title.clone()}</h1>
                    {(!byline.is_empty()).then(|| view! { <p class="pod-byline">{byline}</p> })}
                    <div class="cluster">
                        {duration.map(|d| view! { <Tag>{d}</Tag> })}
                        {voice.map(|v| view! { <Tag>{v}</Tag> })}
                        {cost.map(|c| view! { <Tag>{c}</Tag> })}
                        {(problem_count > 0).then(|| view! {
                            <Tag tone=Tone::Warn title="Audio chunks that failed or nearly failed verification">
                                {format!("{} flagged", plural(problem_count, "chunk", "chunks"))}
                            </Tag>
                        })}
                    </div>
                </div>
                <div class="page-actions">
                    <ButtonLink to=ep.article_url.clone() external=true>"Read article"</ButtonLink>
                </div>
            </header>

            <div class="pod-player-bar">
                <div class="pod-player-meta">
                    <span class="pod-player-title truncate">{title.clone()}</span>
                    <span class="small dim truncate">{now_playing}</span>
                </div>
                <audio
                    class="pods-detail-audio"
                    node_ref=audio_ref
                    aria-label=format!("Listen to {title}")
                    controls=true
                    preload="metadata"
                    src=ep.audio_url.clone()
                    on:timeupdate=move |_| sync_position()
                    on:seeked=move |_| sync_position()
                ></audio>
                <a
                    class="btn ghost icon-only"
                    href=ep.audio_url.clone()
                    target="_blank"
                    rel="noreferrer"
                    download=""
                    aria-label="Download audio"
                    title="Download audio"
                >
                    <Glyph icon=Icon::Download/>
                </a>
            </div>

            <div class=body_class>
                {chapter_panel}
                <section class="pod-transcript" aria-labelledby="pod-transcript-title">
                    <div class="section-head">
                        <h2 class="section-title" id="pod-transcript-title">"Transcript"</h2>
                        <span class="section-meta num">{format!("{} words", locale_number(words as f64))}</span>
                    </div>
                    <div
                        id="episode-transcript"
                        class=move || {
                            if !long_transcript || transcript_expanded.get() {
                                "prose transcript expanded"
                            } else {
                                "prose transcript"
                            }
                        }
                    >
                        {transcript_body(&detail.content)}
                    </div>
                    {long_transcript.then(|| view! {
                        <Button
                            variant=ButtonVariant::Ghost
                            size=ButtonSize::Sm
                            icon=Icon::ChevronDown
                            class=Signal::derive(move || Some(if transcript_expanded.get() { "transcript-toggle open".to_owned() } else { "transcript-toggle".to_owned() }))
                            aria_expanded=Signal::derive(move || Some(transcript_expanded.get().to_string()))
                            aria_controls="episode-transcript"
                            on_click=Callback::new(move |_| transcript_expanded.update(|v| *v = !*v))
                        >
                            {move || if transcript_expanded.get() { "Collapse transcript" } else { "Read full transcript" }}
                        </Button>
                    })}
                </section>
                <Panel title="Details" class="pod-facts" pad=true>
                    <dl class="kv">
                        {facts
                            .into_iter()
                            .map(|(k, v)| view! { <dt>{k}</dt><dd class="num">{v}</dd> })
                            .collect_view()}
                    </dl>
                </Panel>
                <Panel title="Manage" class="pod-manage" pad=true>
                    <div class="cluster">
                        {run_id.map(|run_id| view! {
                            <Button
                                size=ButtonSize::Sm
                                variant=ButtonVariant::Ghost
                                icon=Icon::Logs
                                on_click=Callback::new(move |_| open_logs(run_id.clone(), log_run, toast))
                            >
                                "Logs"
                            </Button>
                        })}
                        <ConfirmButton
                            label="Regenerate"
                            confirm_label="Confirm regenerate"
                            size=ButtonSize::Sm
                            icon=Icon::Refresh
                            busy=retrying
                            title="Queue this article again; the new episode replaces this one"
                            on_confirm=on_retry
                        />
                        <ConfirmButton
                            label="Delete"
                            size=ButtonSize::Sm
                            variant=ButtonVariant::Danger
                            icon=Icon::Trash
                            destructive=true
                            busy=deleting
                            title="Removes the episode and its audio permanently"
                            on_confirm=on_delete
                        />
                    </div>
                </Panel>
            </div>

            <section class="section" aria-labelledby="pod-processing-title">
                <div class="section-head">
                    <h2 class="section-title" id="pod-processing-title">"Processing"</h2>
                    <span class="section-meta">{processing_meta}</span>
                </div>
                <div class="stack-lg">
                    <Panel
                        title="Audio chunks"
                        head_end=ViewFn::from(move || {
                            (problem_count > 0).then(|| view! {
                                <Chip
                                    pressed=flagged_only
                                    count=problem_count
                                    on_click=Callback::new(move |()| flagged_only.update(|v| *v = !*v))
                                >
                                    "Flagged only"
                                </Chip>
                            })
                        })
                    >
                        <Disclosure
                            summary=format!("Show {}", plural(chunk_count, "chunk", "chunks"))
                            open={ problem_count > 0 }
                            class="pod-chunks-disc"
                        >
                            {chunk_body}
                        </Disclosure>
                    </Panel>
                    <div class="grid-2">
                        {retrievers}
                        <Panel title="Cost breakdown">{cost_section(&detail)}</Panel>
                    </div>
                </div>
            </section>
        }
        .into_any()
    };

    view! {
        <article class="pod-detail">{page}</article>
        {move || {
            log_run
                .get()
                .map(|run| view! { <LogViewer run=run on_close=Callback::new(move |()| log_run.set(None)) /> })
        }}
    }
}

/// Union of the detail keys in JS `Array.prototype.sort` order (UTF-16 code units).
pub fn cost_keys(episode: &PressPodsEpisodeDetail) -> Vec<String> {
    let Some(costs) = &episode.costs else {
        return Vec::new();
    };
    let mut keys: Vec<String> = Vec::new();
    let all = costs
        .detail_cents
        .0
        .iter()
        .map(|(k, _)| k)
        .chain(costs.detail_tokens.0.iter().map(|(k, _)| k))
        .chain(costs.detail_chars.0.iter().map(|(k, _)| k));
    for key in all {
        if !keys.contains(key) {
            keys.push(key.clone());
        }
    }
    keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk() -> PressPodsChunkStat {
        PressPodsChunkStat {
            index: 0,
            section_index: 0,
            section_title: None,
            text: "Body".into(),
            char_count: 100,
            duration_seconds: 6.0,
            start_time_seconds: 0.0,
            sec_per_char: 0.06,
            attempts: 1,
            coverage: None,
            word_ratio: None,
            expected_words: None,
            resplit: None,
            resplit_depth: None,
        }
    }

    #[test]
    fn coverage_is_authoritative_when_recorded() {
        let healthy = PressPodsChunkStat {
            coverage: Some(0.9),
            word_ratio: Some(1.0),
            ..chunk()
        };
        assert!(!is_chunk_problematic(&healthy));
        let runaway = PressPodsChunkStat {
            word_ratio: Some(1.9),
            ..healthy.clone()
        };
        assert!(is_chunk_problematic(&runaway));
        let truncated = PressPodsChunkStat {
            coverage: Some(0.7),
            ..healthy.clone()
        };
        assert!(is_chunk_problematic(&truncated));
        let relaxed = PressPodsChunkStat {
            expected_words: Some(40.0),
            ..truncated.clone()
        };
        assert!(!is_chunk_problematic(&relaxed));
        let too_long = PressPodsChunkStat {
            expected_words: Some(61.0),
            ..truncated
        };
        assert!(is_chunk_problematic(&too_long));
    }

    #[test]
    fn duration_band_applies_without_coverage() {
        assert!(!is_chunk_problematic(&chunk()));
        assert!(is_chunk_problematic(&PressPodsChunkStat {
            attempts: 2,
            ..chunk()
        }));
        assert!(is_chunk_problematic(&PressPodsChunkStat {
            sec_per_char: 0.2,
            ..chunk()
        }));
        assert!(is_chunk_problematic(&PressPodsChunkStat {
            sec_per_char: 0.01,
            ..chunk()
        }));
    }

    #[test]
    fn inline_emphasis_pattern_cases() {
        assert_eq!(
            parse_inline("Read *The Book* and _this_ now"),
            vec![
                Inline::Text("Read ".into()),
                Inline::Em("The Book".into()),
                Inline::Text(" and ".into()),
                Inline::Em("this".into()),
                Inline::Text(" now".into()),
            ]
        );
        assert_eq!(
            parse_inline("a * b * c"),
            vec![Inline::Text("a * b * c".into())]
        );
        assert_eq!(parse_inline("**"), vec![Inline::Text("**".into())]);
        assert_eq!(parse_inline("*x*"), vec![Inline::Em("x".into())]);
        assert_eq!(
            parse_inline("*a *b*"),
            vec![Inline::Em("a *b".into())],
            "lazy match closes at the first marker preceded by non-space"
        );
    }

    #[test]
    fn transcript_blocks_split_headings_and_paragraphs() {
        let blocks = transcript_blocks("## Intro  \nfirst line\nsecond\n\n\nNext para\n##NoSpace");
        assert_eq!(
            blocks,
            vec![
                TranscriptBlock::Heading("Intro".into()),
                TranscriptBlock::Paragraph("first line second".into()),
                TranscriptBlock::Paragraph("Next para ##NoSpace".into()),
            ]
        );
    }

    #[test]
    fn bytes_format_with_one_decimal() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024 * 1024), "3072.0 GB");
    }

    #[test]
    fn current_chapter_is_the_last_one_started() {
        let chapters = vec![
            PressPodsChapter {
                start_time_seconds: 0.0,
                title: "Intro".into(),
            },
            PressPodsChapter {
                start_time_seconds: 60.0,
                title: "Body".into(),
            },
        ];
        assert_eq!(current_chapter(&chapters, 0.0), Some(0));
        assert_eq!(current_chapter(&chapters, 59.0), Some(0));
        assert_eq!(
            current_chapter(&chapters, 59.9),
            Some(1),
            "a seek lands just short"
        );
        assert_eq!(current_chapter(&chapters, 600.0), Some(1));
        assert_eq!(current_chapter(&[], 10.0), None);
    }
}
