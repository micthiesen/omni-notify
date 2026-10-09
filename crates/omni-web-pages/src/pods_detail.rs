//! One PressPods episode: player with chapter seek, transcript, chunk
//! diagnostics, retriever attempts and costs (`pages/PodsDetailPage.tsx`).

use leptos::html::Audio;
use leptos::prelude::*;
use omni_api::presspods::{
    PressPodsChunkStat, PressPodsEpisodeDetail, PressPodsRetrieverAttempt, PressPodsTokenCounts,
};
use omni_web_kit::api;
use omni_web_kit::components::{ImageWithFallback, Toast, ToastKind, use_toast};
use omni_web_kit::router::navigate;
use omni_web_kit::task::{spawn_detached, spawn_scoped};
use omni_web_kit::utils::format::{format_absolute_with_year, format_cents};
use omni_web_kit::utils::js::{
    js_round, locale_number, number_string, to_fixed, utf16_len, utf16_slice,
};

use crate::common::{BackLink, DetailField};
use crate::pods::{attempt_success, confirm_delete, format_audio_duration};

// Higgs duration-band bounds (the fallback verifier), scaled for the +10%
// playback speed-up. STT `coverage` is authoritative when present; duration
// is only meaningful without it.
const SEC_PER_CHAR_MIN: f64 = 0.03 / 1.1;
const SEC_PER_CHAR_MAX: f64 = 0.15 / 1.1;
// Mirror DEFAULT_CONTENT_BOUNDS in coverage.ts: the backend's accept test
// rejects on either bound, so the UI flag must too.
const MIN_COVERAGE: f64 = 0.75;
const MIN_COVERAGE_WITH_HEALTHY_RATIO: f64 = 0.68;
const MIN_HEALTHY_WORD_RATIO: f64 = 0.78;
const MAX_HEALTHY_RATIO_EXPECTED_WORDS: f64 = 60.0;
const MAX_WORD_RATIO: f64 = 1.8;

/// `formatBytes`: `512 B`, `1.5 KB`, `3.2 MB`, `1.0 GB`.
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
            TranscriptBlock::Heading(text) => {
                view! { <h3 class="pods-transcript-heading">{render_inline(&text)}</h3> }.into_any()
            }
            TranscriptBlock::Paragraph(text) => {
                view! { <p class="pods-transcript-p">{render_inline(&text)}</p> }.into_any()
            }
        })
        .collect_view()
}

fn retriever_row(attempt: &PressPodsRetrieverAttempt, is_winner: bool) -> impl IntoView + use<> {
    let success = attempt_success(attempt);
    let class = format!(
        "pods-retriever-row {} {}",
        if success {
            ""
        } else {
            "pods-retriever-row-failed"
        },
        if is_winner {
            "pods-retriever-row-winner"
        } else {
            ""
        }
    );
    let (name, body) = match attempt {
        PressPodsRetrieverAttempt::Success {
            name,
            content_rating,
            text_chars,
            ..
        } => (
            name.clone(),
            view! {
                <span class="meta-row pods-retriever-meta">
                    <span>{format!("Rating {}/10", number_string(*content_rating))}</span>
                    <span>{format!("{} chars", locale_number(*text_chars as f64))}</span>
                </span>
            }
            .into_any(),
        ),
        PressPodsRetrieverAttempt::Failure { name, error, .. } => (
            name.clone(),
            view! { <span class="pods-retriever-error">{error.clone()}</span> }.into_any(),
        ),
    };
    view! {
        <div class=class>
            <span class="pods-retriever-name">
                {name}
                {is_winner.then(|| view! { <span class="pods-retriever-winner-badge">"Winner"</span> })}
            </span>
            {body}
        </div>
    }
}

#[component]
fn ChunkCard(chunk: PressPodsChunkStat) -> impl IntoView {
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
            <span
                class="pods-chunk-resplit-badge"
                title="A larger chunk kept failing verification and was re-split into smaller pieces to recover"
            >
                "Re-split"
                {depth}
            </span>
        }
    });
    view! {
        <div class=format!("pods-chunk-card {}", if problematic { "pods-chunk-warn" } else { "" })>
            <div class="pods-chunk-header">
                <span class="pods-chunk-index">{format!("#{}", chunk.index)}</span>
                {chunk
                    .section_title
                    .clone()
                    .filter(|t| !t.is_empty())
                    .map(|t| view! { <span class="pods-chunk-section">{t}</span> })}
                {resplit}
                {problematic.then(|| view! { <span class="pods-chunk-warn-badge">"Needs Review"</span> })}
            </div>
            <div class="meta-row pods-chunk-meta">
                <span>{format!("{} chars", locale_number(chunk.char_count as f64))}</span>
                <span>{format!("{}s", to_fixed(chunk.duration_seconds, 1))}</span>
                <span>{format!("{} s/char", to_fixed(chunk.sec_per_char, 3))}</span>
                <span>
                    {format!("{} attempt{}", chunk.attempts, if chunk.attempts == 1 { "" } else { "s" })}
                </span>
                {chunk
                    .coverage
                    .map(|c| {
                        view! { <span>{format!("{}% coverage", number_string(js_round(c * 100.0)))}</span> }
                    })}
                <span>
                    {format!(
                        "starts at {}",
                        format_audio_duration(Some(chunk.start_time_seconds)).unwrap_or_else(|| "null".to_owned()),
                    )}
                </span>
            </div>
            <p class="pods-chunk-text">{preview}</p>
            {long
                .then(|| {
                    view! {
                        <button
                            type="button"
                            class="pods-chunk-text-toggle"
                            on:click=move |_| expanded.update(|v| *v = !*v)
                        >
                            {move || if expanded.get() { "Show Less" } else { "Show Full Text" }}
                        </button>
                    }
                })}
        </div>
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

fn cost_table(episode: &PressPodsEpisodeDetail) -> impl IntoView + use<> {
    let Some(costs) = episode.costs.clone() else {
        return view! { <div class="muted">"No cost data recorded."</div> }.into_any();
    };
    let dash = || "—".to_owned();
    let keys = cost_keys(episode);
    let rows = (!keys.is_empty()).then(|| {
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
                let usage = (tokens.is_some() || chars.is_some()).then(|| {
                    view! {
                        <div class="meta-row pods-cost-usage">
                            {tokens
                                .map(|t| {
                                    view! {
                                        <span>
                                            {format!(
                                                "{} / {} tokens",
                                                locale_number(t.input),
                                                locale_number(t.output),
                                            )}
                                        </span>
                                    }
                                })}
                            {chars.map(|c| view! { <span>{format!("{} chars", locale_number(c))}</span> })}
                        </div>
                    }
                });
                view! {
                    <div class="pods-cost-row">
                        <div class="pods-cost-row-head">
                            <span class="pods-cost-key">{key.clone()}</span>
                            <span class="pods-cost-amount">{format_cents(cents).unwrap_or_else(dash)}</span>
                        </div>
                        {usage}
                    </div>
                }
            })
            .collect_view();
        view! { <div class="pods-cost-list">{rows}</div> }
    });
    view! {
        <div class="meta-row pods-cost-summary">
            <span>{format!("LLM {}", format_cents(Some(costs.llm_cents)).unwrap_or_else(dash))}</span>
            <span>{format!("TTS {}", format_cents(Some(costs.tts_cents)).unwrap_or_else(dash))}</span>
            <span class="pods-cost-total">
                {format!("Total {}", format_cents(episode.episode.cost_cents).unwrap_or_else(dash))}
            </span>
        </div>
        {rows}
    }
    .into_any()
}

/// `/pods/:id`.
#[component]
pub fn PodsDetailPage(#[prop(into)] id: String) -> impl IntoView {
    let episode = RwSignal::new(None::<PressPodsEpisodeDetail>);
    let error = RwSignal::new(None::<String>);
    let transcript_expanded = RwSignal::new(false);
    let audio_ref = NodeRef::<Audio>::new();
    let retrying = RwSignal::new(false);
    let deleting = RwSignal::new(false);
    let toast = use_toast();

    spawn_scoped(async move {
        match api::fetch_press_pods_episode(&id).await {
            Ok(res) => episode.set(Some(res.episode)),
            Err(err) => error.set(Some(err.message().to_owned())),
        }
    });

    let on_retry = move |_| {
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
    };

    let on_delete = move |_| {
        let Some((episode_id, title)) = episode.with_untracked(|e| {
            e.as_ref()
                .map(|e| (e.episode.episode_id.clone(), e.episode.title.clone()))
        }) else {
            return;
        };
        if deleting.get_untracked() || !confirm_delete(&title) {
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
    };

    let seek = move |seconds: f64| {
        if let Some(audio) = audio_ref.get_untracked() {
            audio.set_current_time(seconds);
            let _ = audio.focus();
        }
    };

    move || {
        if let Some(error) = error.get() {
            return view! {
                <BackLink to="/pods" label="All Episodes" />
                <div class="error">
                    <div>"Failed to load this episode"</div>
                    <div class="error-detail">{error}</div>
                </div>
            }
            .into_any();
        }
        let Some(detail) = episode.get() else {
            return view! { <div class="loading">"Loading…"</div> }.into_any();
        };
        let ep = &detail.episode;
        let problem_count = detail
            .chunks
            .as_ref()
            .map_or(0, |c| c.iter().filter(|c| is_chunk_problematic(c)).count());
        let author = ep.author.clone().filter(|a| !a.is_empty());
        let publication = ep.publication.clone().filter(|p| !p.is_empty());
        let byline = (author.is_some() || publication.is_some()).then(|| {
            let gender = detail
                .author_gender
                .clone()
                .filter(|g| !g.is_empty())
                .map(|g| format!(" ({g})"))
                .unwrap_or_default();
            view! {
                <div class="meta-row pods-detail-byline">
                    {author.map(|a| view! { <span>{a}{gender}</span> })}
                    {publication.map(|p| view! { <span>{p}</span> })}
                </div>
            }
        });
        let voice = ep
            .voice_name
            .clone()
            .filter(|v| !v.is_empty())
            .map(|voice| {
                let provider = detail.voice_provider.clone().filter(|p| !p.is_empty());
                view! {
                    <span class="meta-row pods-detail-voice-badge">
                        <span>{voice}</span>
                        {provider.map(|p| view! { <span>{p}</span> })}
                    </span>
                }
            });
        let chapters = ep.chapters.clone().filter(|c| !c.is_empty()).map(|chapters| {
            view! {
                <section class="page-section">
                    <h2 class="section-title">"Chapters"</h2>
                    <ul class="pods-chapter-list">
                        {chapters
                            .into_iter()
                            .map(|chapter| {
                                let at = format_audio_duration(Some(chapter.start_time_seconds))
                                    .unwrap_or_else(|| "null".to_owned());
                                let start = chapter.start_time_seconds;
                                view! {
                                    <li>
                                        <button
                                            type="button"
                                            class="pods-chapter-row pods-chapter-button"
                                            aria-label=format!("Jump to {} at {at}", chapter.title)
                                            on:click=move |_| seek(start)
                                        >
                                            <span class="pods-chapter-time">{at.clone()}</span>
                                            <span class="pods-chapter-title">{chapter.title.clone()}</span>
                                        </button>
                                    </li>
                                }
                            })
                            .collect_view()}
                    </ul>
                </section>
            }
        });
        let created = format_absolute_with_year(ep.created_at as f64);
        let mut details =
            vec![view! { <DetailField label="Created">{created}</DetailField> }.into_any()];
        if let Some(published) = ep.published_at {
            let text = format_absolute_with_year(published as f64);
            details.push(view! { <DetailField label="Published">{text}</DetailField> }.into_any());
        }
        let size = format_bytes(ep.file_bytes);
        details.push(view! { <DetailField label="File Size">{size}</DetailField> }.into_any());
        if ep.synthesized_seconds.is_some() {
            let text = format_audio_duration(ep.synthesized_seconds).unwrap_or_default();
            details.push(
                view! { <DetailField label="Synthesized Audio">{text}</DetailField> }.into_any(),
            );
        }
        if let Some(seconds) = ep.retriever_seconds {
            let text = format!("{}s", to_fixed(seconds, 1));
            details.push(
                view! { <DetailField label="Retrieval Time">{text}</DetailField> }.into_any(),
            );
        }
        let chunk_section = match &detail.chunks {
            None => view! {
                <div class="muted">"This episode was synthesized before per-chunk stats were recorded."</div>
            }
            .into_any(),
            Some(chunks) if chunks.is_empty() => {
                view! { <div class="muted">"No chunk data recorded."</div> }.into_any()
            }
            Some(chunks) => view! {
                <div class="pods-chunk-list">
                    {chunks.iter().cloned().map(|chunk| view! { <ChunkCard chunk=chunk /> }).collect_view()}
                </div>
            }
            .into_any(),
        };
        let retrievers = ep
            .retriever_attempts
            .clone()
            .filter(|a| !a.is_empty())
            .map(|attempts| {
                let winner = ep.retriever_name.clone();
                view! {
                    <section class="page-section">
                        <h2 class="section-title">"Retriever Attempts"</h2>
                        <div class="pods-retriever-list">
                            {attempts
                                .iter()
                                .map(|attempt| {
                                    let name = match attempt {
                                        PressPodsRetrieverAttempt::Success { name, .. }
                                        | PressPodsRetrieverAttempt::Failure { name, .. } => name,
                                    };
                                    retriever_row(attempt, winner.as_deref() == Some(name.as_str()))
                                })
                                .collect_view()}
                        </div>
                    </section>
                }
            });
        let duration = format_audio_duration(ep.duration_seconds);
        let cost = format_cents(ep.cost_cents);
        let flagged = |suffix: &str| {
            format!(
                "{problem_count} audio chunk{} {suffix}",
                if problem_count == 1 { "" } else { "s" }
            )
        };
        let chunk_count = detail.chunks.as_ref().map(Vec::len);
        view! {
            <div class="pods-detail-back-row">
                <BackLink to="/pods" label="All Episodes" />
            </div>

            <div class="detail-head">
                <ImageWithFallback
                    src=ep.lead_image_url.clone()
                    alt=format!("{} lead image", ep.title)
                    class="detail-art"
                    placeholder_class="detail-art-placeholder"
                    placeholder=|| "🎙️"
                />
                <div class="detail-head-body">
                    <h1 class="detail-title">{ep.title.clone()}</h1>
                    {byline}
                    <div class="detail-badges">
                        {voice}
                        {duration.map(|d| view! { <span class="pods-detail-duration-badge">{d}</span> })}
                        {cost.map(|c| view! { <span class="pods-detail-cost-badge">{c}</span> })}
                        {(problem_count > 0)
                            .then(|| {
                                view! {
                                    <span class="pods-chunk-warn-count pods-detail-quality-warning">
                                        {flagged("flagged")}
                                    </span>
                                }
                            })}
                    </div>
                    <audio
                        class="pods-detail-audio"
                        node_ref=audio_ref
                        aria-label=format!("Listen to {}", ep.title)
                        controls=true
                        preload="metadata"
                        src=ep.audio_url.clone()
                    ></audio>
                    <nav class="detail-service-links" aria-label="Episode links">
                        <a
                            href=ep.article_url.clone()
                            target="_blank"
                            rel="noreferrer"
                            class="detail-service-link detail-service-link-article"
                        >
                            <span class="detail-service-link-label">"Read Article"</span>
                            <span class="detail-service-link-hint">
                                {format!(
                                    "Open on {}",
                                    ep.domain.clone().unwrap_or_else(|| "original site".to_owned()),
                                )}
                            </span>
                        </a>
                        <a
                            href=ep.audio_url.clone()
                            target="_blank"
                            rel="noreferrer"
                            class="detail-metadata-link pods-detail-audio-download"
                            download=""
                        >
                            "Download Audio"
                        </a>
                    </nav>
                </div>
            </div>

            <div class="detail-sections">
                <div class="pods-overview">
                    {chapters}
                    <section class="page-section">
                        <h2 class="section-title">"Transcript"</h2>
                        <div
                            id="episode-transcript"
                            class=move || {
                                format!(
                                    "pods-transcript {}",
                                    if transcript_expanded.get() { "pods-transcript-expanded" } else { "" },
                                )
                            }
                        >
                            {transcript_body(&detail.content)}
                        </div>
                        <button
                            type="button"
                            class="pods-transcript-toggle"
                            aria-expanded=move || transcript_expanded.get().to_string()
                            aria-controls="episode-transcript"
                            on:click=move |_| transcript_expanded.update(|v| *v = !*v)
                        >
                            {move || {
                                if transcript_expanded.get() {
                                    "Collapse Transcript"
                                } else {
                                    "Show Full Transcript"
                                }
                            }}
                        </button>
                    </section>
                    <section class="page-section">
                        <h2 class="section-title">"Details"</h2>
                        <dl class="detail-grid">{details}</dl>
                    </section>
                </div>

                <details class="content-disclosure processing-disclosure">
                    <summary>
                        "Processing Details"
                        {(problem_count > 0)
                            .then(|| view! { <span class="section-count">{format!("{problem_count} Flagged")}</span> })}
                    </summary>
                    <section class="page-section">
                        <h2 class="section-title">
                            "Audio Chunks"
                            {chunk_count.map(|n| view! { <span class="section-count">{n}</span> })}
                            {(problem_count > 0)
                                .then(|| {
                                    view! {
                                        <span class="pods-chunk-warn-count">{format!("{problem_count} flagged")}</span>
                                    }
                                })}
                        </h2>
                        {chunk_section}
                    </section>
                    {retrievers}
                    <section class="page-section">
                        <h2 class="section-title">"Cost Breakdown"</h2>
                        {cost_table(&detail)}
                    </section>
                </details>

                <section class="page-section pods-detail-admin">
                    <h2 class="section-title">"Episode Administration"</h2>
                    <div class="pods-detail-actions">
                        <button
                            type="button"
                            class="pods-card-logs"
                            on:click=on_retry
                            disabled=move || retrying.get()
                        >
                            {move || if retrying.get() { "Retrying…" } else { "Retry" }}
                        </button>
                        <button
                            type="button"
                            class="pods-card-logs pods-card-delete"
                            on:click=on_delete
                            disabled=move || deleting.get()
                        >
                            {move || if deleting.get() { "Deleting…" } else { "Delete" }}
                        </button>
                    </div>
                </section>
            </div>
            <Toast toast=toast.toast />
        }
        .into_any()
    }
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
    fn inline_emphasis_matches_the_ts_regex() {
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
}
