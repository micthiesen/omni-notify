//! PressPods episode list, submission form and job queue.

use leptos::prelude::*;
use omni_api::common::encode_uri_component;
use omni_api::presspods::{
    PressPodsEpisode, PressPodsJob, PressPodsJobStatus, PressPodsRetrieverAttempt,
};
use omni_api::runs::Run;
use omni_web_kit::api;
use omni_web_kit::components::tone::hue_class;
use omni_web_kit::components::{
    Button, ButtonSize, ButtonVariant, ConfirmButton, EmptyState, ErrorState, Glyph, Icon,
    IconSize, LogViewer, PageHead, ShowMoreButton, SkeletonRows, Status, StatusKind, Tag,
    ToastHandle, ToastKind, use_show_more, use_toast,
};
use omni_web_kit::hooks::use_now;
use omni_web_kit::live::{ConnectionState, use_live_data};
use omni_web_kit::router::Link;
use omni_web_kit::task::{spawn_detached, spawn_scoped};
use omni_web_kit::utils::format::{
    format_absolute, format_cents, format_countdown, format_date_only, format_duration,
    format_relative,
};
use omni_web_kit::utils::js::js_round;

use crate::rec_ui::MediaSwitch;

const PAGE_SIZE: usize = 20;

/// `m:ss` for an audio duration in seconds; `None` when unknown.
pub fn format_audio_duration(seconds: Option<f64>) -> Option<String> {
    let seconds = seconds.filter(|s| s.is_finite())?;
    let total = js_round(seconds) as i64;
    let mins = (total as f64 / 60.0).floor() as i64;
    // JS `%` keeps the dividend's sign, as Rust's does.
    let secs = total % 60;
    Some(format!("{mins}:{}", pad_start_2(secs)))
}

/// `n.toString().padStart(2, "0")`.
fn pad_start_2(n: i64) -> String {
    let text = n.to_string();
    if text.len() >= 2 {
        text
    } else {
        format!("0{text}")
    }
}

/// The host of an article URL without `www.` (`example.com`), or the URL
/// itself when it has no host.
pub(crate) fn source_host(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .rsplit('@')
        .next()
        .unwrap_or_default();
    let host = host.strip_prefix("www.").unwrap_or(host);
    if host.is_empty() {
        url.to_owned()
    } else {
        host.to_owned()
    }
}

/// The episode's source label: its stored domain, else the article host.
pub(crate) fn episode_source(episode: &PressPodsEpisode) -> String {
    episode
        .domain
        .clone()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| source_host(&episode.article_url))
}

fn job_status(status: PressPodsJobStatus) -> (StatusKind, &'static str) {
    match status {
        PressPodsJobStatus::Queued => (StatusKind::Info, "Queued"),
        PressPodsJobStatus::Processing => (StatusKind::Running, "Processing"),
        PressPodsJobStatus::Failed => (StatusKind::Fault, "Failed"),
    }
}

pub(crate) fn attempt_success(attempt: &PressPodsRetrieverAttempt) -> bool {
    match attempt {
        PressPodsRetrieverAttempt::Success { success, .. }
        | PressPodsRetrieverAttempt::Failure { success, .. } => *success,
    }
}

/// The winning retriever and how many succeeded.
pub fn retriever_summary(episode: &PressPodsEpisode) -> Option<String> {
    let Some(attempts) = &episode.retriever_attempts else {
        return episode.retriever_name.clone();
    };
    let ok = attempts.iter().filter(|a| attempt_success(a)).count();
    Some(format!(
        "{} ({ok}/{} retrievers)",
        episode.retriever_name.as_deref().unwrap_or("null"),
        attempts.len()
    ))
}

/// Opens a run's log viewer, or toasts when its logs are gone.
pub(crate) fn open_logs(run_id: String, log_run: RwSignal<Option<Run>>, toast: ToastHandle) {
    spawn_detached(async move {
        match api::fetch_run_logs(&run_id).await {
            Ok(logs) => log_run.set(Some(logs.run)),
            Err(_) => toast.show(
                "Logs for this episode are no longer available",
                ToastKind::Error,
            ),
        }
    });
}

/// Square episode art: the article's lead image, else the source's
/// initial on a gradient derived from the source.
#[component]
pub(crate) fn EpisodeArt(src: Option<String>, source: String) -> impl IntoView {
    let broken = RwSignal::new(false);
    let hue = hue_class(&source);
    let initial = source
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default();
    view! {
        <span class="pod-art" aria-hidden="true">
            {move || match (&src, broken.get()) {
                (Some(src), false) => view! {
                    <img src=src.clone() alt="" loading="lazy" decoding="async" on:error=move |_| broken.set(true)/>
                }
                .into_any(),
                _ => view! { <span class=format!("poster-fallback {hue}")>{initial.clone()}</span> }.into_any(),
            }}
        </span>
    }
}

#[component]
pub fn PodsPage() -> impl IntoView {
    let live = use_live_data();
    let episodes = RwSignal::new(None::<Vec<PressPodsEpisode>>);
    let jobs = RwSignal::new(Vec::<PressPodsJob>::new());
    let error = RwSignal::new(None::<String>);
    let url = RwSignal::new(String::new());
    let submitting = RwSignal::new(false);
    let submit_error = RwSignal::new(None::<String>);
    let log_run = RwSignal::new(None::<Run>);
    let playing = RwSignal::new(None::<String>);
    let options_open = RwSignal::new(None::<String>);
    let busy_id = RwSignal::new(None::<String>);
    let toast = use_toast();
    let now = use_now(1000);

    let apply =
        move |result: Result<omni_api::presspods::PressPodsListResponse, api::ApiClientError>| {
            match result {
                Ok(res) => {
                    episodes.set(Some(res.episodes));
                    jobs.set(res.jobs);
                    error.set(None);
                }
                Err(err) => error.set(Some(err.message().to_owned())),
            }
        };
    // A reload requested by an action; it outlives the action's handler.
    let reload = move || {
        spawn_detached(async move { apply(api::fetch_press_pods().await) });
    };

    spawn_scoped(async move { apply(api::fetch_press_pods().await) });

    // Refresh whenever a PressPods run settles (episodes/jobs will have changed).
    let run_key = Memo::new(move |_| {
        live.snapshot.with(|s| {
            s.as_ref()
                .and_then(|s| s.tasks.iter().find(|t| t.name == "PressPods"))
                .and_then(|t| t.last_run.as_ref())
                .map(|r| format!("{}:{:?}", r.run_id, r.status))
        })
    });
    Effect::new(move |previous: Option<Option<String>>| {
        let key = run_key.get();
        // The mount already loaded; reload on later settled runs only.
        if key.is_some() && previous.is_some() {
            spawn_scoped(async move { apply(api::fetch_press_pods().await) });
        }
        key
    });

    // Episode/job data rides its own endpoint, not the SSE snapshot, and jobs
    // can change during a stream outage. Re-fetch on every reconnect.
    Effect::new(move |previous: Option<ConnectionState>| {
        let connection = live.connection.get();
        if connection == ConnectionState::Live
            && previous.is_some_and(|p| p != ConnectionState::Live)
        {
            spawn_scoped(async move { apply(api::fetch_press_pods().await) });
        }
        connection
    });

    let on_submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let trimmed = url.get_untracked().trim().to_owned();
        if trimmed.is_empty() || submitting.get_untracked() {
            return;
        }
        submitting.set(true);
        submit_error.set(None);
        spawn_detached(async move {
            match api::submit_press_pods_url(&trimmed).await {
                Ok(_) => {
                    url.set(String::new());
                    toast.show(format!("Queued {}", source_host(&trimmed)), ToastKind::Info);
                    reload();
                }
                Err(err) => submit_error.set(Some(err.message().to_owned())),
            }
            submitting.set(false);
        });
    };

    let on_retry = move |job_id: String| {
        spawn_detached(async move {
            match api::retry_press_pods_job(&job_id).await {
                Ok(_) => reload(),
                Err(err) => toast.show(err.message(), ToastKind::Error),
            }
        });
    };
    let on_dismiss = move |job_id: String| {
        spawn_detached(async move {
            match api::dismiss_press_pods_job(&job_id).await {
                Ok(_) => reload(),
                Err(err) => toast.show(err.message(), ToastKind::Error),
            }
        });
    };
    let on_retry_episode = move |episode_id: String| {
        busy_id.set(Some(episode_id.clone()));
        spawn_detached(async move {
            match api::retry_press_pods_episode(&episode_id).await {
                Ok(_) => {
                    reload();
                    toast.show("Re-queued for regeneration", ToastKind::Info);
                }
                Err(err) => toast.show(err.message(), ToastKind::Error),
            }
            busy_id.set(None);
        });
    };
    let on_delete_episode = move |episode_id: String| {
        busy_id.set(Some(episode_id.clone()));
        spawn_detached(async move {
            match api::delete_press_pods_episode(&episode_id).await {
                Ok(_) => {
                    if playing.get_untracked().as_deref() == Some(episode_id.as_str()) {
                        playing.set(None);
                    }
                    episodes.update(|list| {
                        if let Some(list) = list {
                            list.retain(|e| e.episode_id != episode_id);
                        }
                    });
                    toast.show("Episode deleted", ToastKind::Info);
                }
                Err(err) => toast.show(err.message(), ToastKind::Error),
            }
            busy_id.set(None);
        });
    };

    let show = use_show_more(
        Signal::derive(move || episodes.get().unwrap_or_default()),
        PAGE_SIZE,
        Signal::stored(String::new()),
    );
    let episode_count = Memo::new(move |_| episodes.with(|e| e.as_ref().map_or(0, Vec::len)));

    let job_row = move |job: PressPodsJob| {
        let failed = job.status == PressPodsJobStatus::Failed;
        let processing = job.status == PressPodsJobStatus::Processing;
        let (kind, word) = job_status(job.status);
        let retry_id = job.job_id.clone();
        let dismiss_id = job.job_id.clone();
        let created = job.created_at as f64;
        let updated = job.updated_at as f64;
        let next_attempt = job.next_attempt_at.map(|t| t as f64);
        let timing = move || {
            let now = now.get();
            if processing {
                format!("running {}", format_duration((now - updated).max(0.0)))
            } else if let Some(next) = next_attempt.filter(|t| *t > now) {
                format!("retries in {}", format_countdown(next - now))
            } else {
                format_relative(created)
            }
        };
        let error = job.last_error.clone().filter(|e| !e.is_empty());
        let logs = job.last_run_id.clone().filter(|r| !r.is_empty());
        view! {
            <li class=if processing { "pod-job running" } else { "pod-job" }>
                {processing.then(|| view! { <span class="progress-line" role="presentation"></span> })}
                <Status kind=kind label=word.to_owned()/>
                <div class="row-main">
                    <span class="row-title">{source_host(&job.url)}</span>
                    <a class="row-sub mono" href=job.url.clone() target="_blank" rel="noreferrer" title=job.url.clone()>
                        {job.url.clone()}
                    </a>
                    {error.map(|e| view! { <span class="pod-job-error clamp-2" title=e.clone()>{e.clone()}</span> })}
                </div>
                <div class="pod-job-end">
                    <span class="num dim" title=format_absolute(created)>{timing}</span>
                    {(job.attempts > 1).then(|| view! { <Tag>{format!("attempt {}", job.attempts)}</Tag> })}
                    <div class="cluster">
                        {logs.map(|run_id| view! {
                            <Button
                                size=ButtonSize::Sm
                                variant=ButtonVariant::Ghost
                                icon=Icon::Logs
                                on_click=Callback::new(move |_| open_logs(run_id.clone(), log_run, toast))
                            >
                                "Logs"
                            </Button>
                        })}
                        {failed.then(|| view! {
                            <Button
                                size=ButtonSize::Sm
                                icon=Icon::Refresh
                                on_click=Callback::new(move |_| on_retry(retry_id.clone()))
                            >
                                "Retry"
                            </Button>
                            <ConfirmButton
                                label="Dismiss"
                                confirm_label="Confirm dismiss"
                                size=ButtonSize::Sm
                                variant=ButtonVariant::Ghost
                                title="Remove this failed job from the queue"
                                on_confirm=Callback::new(move |()| on_dismiss(dismiss_id.clone()))
                            />
                        })}
                    </div>
                </div>
            </li>
        }
    };

    let episode_row = move |episode: PressPodsEpisode| {
        let id = episode.episode_id.clone();
        let detail_path = format!("/pods/{}", encode_uri_component(&id));
        let source = episode_source(&episode);
        let byline = [episode.author.clone(), episode.publication.clone()]
            .into_iter()
            .flatten()
            .filter(|p| !p.is_empty() && *p != source)
            .collect::<Vec<_>>();
        let sub = std::iter::once(source.clone())
            .chain(byline)
            .collect::<Vec<_>>()
            .join(" · ");
        let duration = format_audio_duration(episode.duration_seconds);
        let created = episode.created_at as f64;
        let player_id = format!("pod-player-{id}");
        let options_id = format!("pod-options-{id}");
        let open = {
            let id = id.clone();
            Memo::new(move |_| playing.with(|p| p.as_deref() == Some(id.as_str())))
        };
        let options = {
            let id = id.clone();
            Memo::new(move |_| options_open.with(|p| p.as_deref() == Some(id.as_str())))
        };
        let busy = {
            let id = id.clone();
            Signal::derive(move || busy_id.with(|b| b.as_deref() == Some(id.as_str())))
        };
        let toggle_play = {
            let id = id.clone();
            move |_| {
                let next = (!open.get_untracked()).then(|| id.clone());
                playing.set(next);
            }
        };
        let toggle_options = {
            let id = id.clone();
            move |_| {
                let next = (!options.get_untracked()).then(|| id.clone());
                options_open.set(next);
            }
        };
        let title = episode.title.clone();
        let listen_label = format!("Listen to {}", episode.title);
        let audio_url = episode.audio_url.clone();
        let facts = [
            episode.voice_name.clone().filter(|v| !v.is_empty()),
            format_cents(episode.cost_cents),
            retriever_summary(&episode).filter(|s| !s.is_empty()),
            Some(format!("Made {}", format_absolute(created))),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        let run_id = episode.run_id.clone().filter(|r| !r.is_empty());
        let retry_id = id.clone();
        let delete_id = id.clone();
        let excerpt = episode.excerpt.clone().filter(|e| !e.is_empty());
        let article_url = episode.article_url.clone();
        let player_ref = player_id.clone();
        let options_ref = options_id.clone();
        view! {
            <li class="pod">
                <div class="pod-row">
                    <button
                        type="button"
                        class=move || if open.get() { "play-btn open" } else { "play-btn" }
                        aria-label=move || {
                            if open.get() { format!("Stop {title}") } else { format!("Play {title}") }
                        }
                        aria-expanded=move || open.get().to_string()
                        aria-controls=player_ref.clone()
                        on:click=toggle_play
                    >
                        {move || {
                            let icon = if open.get() { Icon::Pause } else { Icon::Play };
                            view! { <Glyph icon size=IconSize::Small/> }
                        }}
                    </button>
                    <EpisodeArt src=episode.lead_image_url.clone() source=source.clone()/>
                    <div class="row-main">
                        <Link to=detail_path class="row-title pod-title">{episode.title.clone()}</Link>
                        <span class="row-sub">{sub}</span>
                    </div>
                    <div class="pod-end">
                        {duration.map(|d| view! { <span class="num">{d}</span> })}
                        <span class="dim small hide-phone" title=format_absolute(created)>
                            {format_date_only(created)}
                        </span>
                        <Button
                            size=ButtonSize::Sm
                            variant=ButtonVariant::Ghost
                            icon_only=true
                            icon=Icon::ChevronDown
                            class=Signal::derive(move || if options.get() { "pod-more open".to_owned() } else { "pod-more".to_owned() })
                            aria_label="Episode options"
                            aria_expanded=Signal::derive(move || Some(options.get().to_string()))
                            aria_controls=options_ref.clone()
                            on_click=Callback::new(toggle_options)
                        />
                    </div>
                </div>
                {move || {
                    open.get()
                        .then(|| {
                            view! {
                                <div class="pod-player" id=player_id.clone()>
                                    <audio
                                        aria-label=listen_label.clone()
                                        controls=true
                                        autoplay=true
                                        preload="metadata"
                                        src=audio_url.clone()
                                    ></audio>
                                </div>
                            }
                        })
                }}
                {move || {
                    options.get()
                        .then(|| {
                            let run_id = run_id.clone();
                            let retry_id = retry_id.clone();
                            let delete_id = delete_id.clone();
                            view! {
                                <div class="pod-options" id=options_id.clone()>
                                    {excerpt.clone().map(|e| view! { <p class="pod-excerpt clamp-2">{e}</p> })}
                                    <p class="small dim">{facts.clone()}</p>
                                    <div class="cluster">
                                        <a class="btn sm ghost" href=article_url.clone() target="_blank" rel="noreferrer">
                                            "Article" <Glyph icon=Icon::External size=IconSize::Small/>
                                        </a>
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
                                        <span class="spacer"></span>
                                        <ConfirmButton
                                            label="Regenerate"
                                            confirm_label="Confirm regenerate"
                                            size=ButtonSize::Sm
                                            icon=Icon::Refresh
                                            busy=busy
                                            title="Queue this article again; the new episode replaces this one"
                                            on_confirm=Callback::new(move |()| on_retry_episode(retry_id.clone()))
                                        />
                                        <ConfirmButton
                                            label="Delete"
                                            size=ButtonSize::Sm
                                            variant=ButtonVariant::Danger
                                            icon=Icon::Trash
                                            destructive=true
                                            busy=busy
                                            title="Removes the episode and its audio permanently"
                                            on_confirm=Callback::new(move |()| on_delete_episode(delete_id.clone()))
                                        />
                                    </div>
                                </div>
                            }
                        })
                }}
            </li>
        }
    };

    // Gate on the body's shape only: rebuilding the list on every reload
    // would recreate each row and restart a playing episode.
    let body_state = Memo::new(move |_| {
        if episodes.with(Option::is_none) {
            return match error.get() {
                Some(message) => EpisodesBody::Error(message),
                None => EpisodesBody::Loading,
            };
        }
        if episode_count.get() == 0 {
            EpisodesBody::Empty
        } else {
            EpisodesBody::List
        }
    });
    let episodes_body = move || {
        match body_state.get() {
            EpisodesBody::Error(message) => {
                return view! {
                    <ErrorState
                        title="Episodes did not load"
                        raw=message
                        retry=Callback::new(move |()| reload())
                    />
                }
                .into_any();
            }
            EpisodesBody::Loading => {
                return view! { <section class="panel"><SkeletonRows count=5 label="Loading episodes"/></section> }
                    .into_any();
            }
            EpisodesBody::Empty => {
                return view! {
                    <section class="panel">
                        <EmptyState icon=Icon::Mic message="Paste an article URL to make your first episode."/>
                    </section>
                }
                .into_any();
            }
            EpisodesBody::List => {}
        }
        view! {
            <section class="panel">
                <ul class="pod-list" aria-label="Episodes">
                    <For
                        each=move || show.visible.get()
                        key=|episode| format!("{}|{episode:?}", episode.episode_id)
                        children=episode_row
                    />
                </ul>
            </section>
            {move || {
                show.has_more
                    .get()
                    .then(|| {
                        view! {
                            <ShowMoreButton
                                remaining=show.remaining
                                on_click=Callback::new(move |()| show.show_more())
                                noun="episodes"
                            />
                        }
                    })
            }}
        }
        .into_any()
    };

    // Job rows are keyed by content, so a reload keeps unchanged rows (and
    // an armed Dismiss confirmation) instead of rebuilding the queue.
    let has_jobs = Memo::new(move |_| jobs.with(|list| !list.is_empty()));
    let queue_meta = move || {
        jobs.with(|list| {
            let failed = list
                .iter()
                .filter(|j| j.status == PressPodsJobStatus::Failed)
                .count();
            if failed > 0 {
                format!("{} in progress · {failed} failed", list.len() - failed)
            } else {
                format!("{} in progress", list.len())
            }
        })
    };

    let lede = move || -> Option<String> {
        Some(match episode_count.get() {
            0 => "Articles read aloud, ready for your podcast app.".to_owned(),
            1 => "1 episode, read aloud from the article you sent.".to_owned(),
            n => format!("{n} episodes, read aloud from the articles you sent."),
        })
    };

    view! {
        <PageHead title="PressPods" lede=Signal::derive(lede)>
            <MediaSwitch/>
        </PageHead>

        <form class="panel pods-compose" on:submit=on_submit>
            <label class="field-label" for="article-url">"Turn an article into an episode"</label>
            <div class="pods-compose-row">
                <input
                    type="url"
                    id="article-url"
                    class="input"
                    inputmode="url"
                    autocomplete="off"
                    aria-describedby=move || submit_error.with(Option::is_some).then_some("article-url-error")
                    aria-invalid=move || submit_error.with(Option::is_some).then_some("true")
                    placeholder="https://example.com/article"
                    prop:value=move || url.get()
                    on:input=move |event| url.set(event_target_value(&event))
                    disabled=move || submitting.get()
                />
                <Button
                    variant=ButtonVariant::Primary
                    submit=true
                    icon=Icon::Mic
                    busy=submitting
                    disabled=Signal::derive(move || url.with(|u| u.trim().is_empty()))
                    disabled_reason="Paste an article URL first"
                >
                    "Make episode"
                </Button>
            </div>
            {move || {
                submit_error
                    .get()
                    .map(|e| {
                        view! {
                            <p id="article-url-error" role="alert" class="field-error">
                                {e}
                            </p>
                        }
                    })
            }}
        </form>

        {move || {
            has_jobs
                .get()
                .then(|| {
                    view! {
                        <section class="section" aria-labelledby="pods-queue-title">
                            <div class="section-head">
                                <h2 class="section-title" id="pods-queue-title">"Queue"</h2>
                                <span class="section-meta">{queue_meta}</span>
                            </div>
                            <section class="panel">
                                <ul class="pod-jobs">
                                    <For
                                        each=move || jobs.get()
                                        key=|job| format!("{job:?}")
                                        children=job_row
                                    />
                                </ul>
                            </section>
                        </section>
                    }
                })
        }}

        <section class="section" aria-labelledby="pods-episodes-title">
            <div class="section-head">
                <h2 class="section-title" id="pods-episodes-title">"Episodes"</h2>
                {move || {
                    let n = episode_count.get();
                    (n > 0).then(|| view! { <span class="section-meta num">{n}</span> })
                }}
            </div>
            {episodes_body}
        </section>

        {move || {
            log_run
                .get()
                .map(|run| view! { <LogViewer run=run on_close=Callback::new(move |()| log_run.set(None)) /> })
        }}
    }
}

/// What the episodes section shows.
#[derive(Clone, Debug, PartialEq)]
enum EpisodesBody {
    Error(String),
    Loading,
    Empty,
    List,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_durations_are_minutes_and_padded_seconds() {
        assert_eq!(format_audio_duration(None), None);
        assert_eq!(format_audio_duration(Some(f64::NAN)), None);
        assert_eq!(format_audio_duration(Some(0.0)).as_deref(), Some("0:00"));
        assert_eq!(format_audio_duration(Some(61.4)).as_deref(), Some("1:01"));
        assert_eq!(format_audio_duration(Some(59.5)).as_deref(), Some("1:00"));
        assert_eq!(
            format_audio_duration(Some(3725.0)).as_deref(),
            Some("62:05")
        );
    }

    #[test]
    fn source_host_drops_scheme_path_and_www() {
        assert_eq!(
            source_host("https://www.example.com/a/b?c=1"),
            "example.com"
        );
        assert_eq!(source_host("http://user@news.site.org#x"), "news.site.org");
        assert_eq!(source_host("example.net/path"), "example.net");
        assert_eq!(source_host(""), "");
    }

    #[test]
    fn job_statuses_have_distinct_shapes() {
        assert_eq!(
            job_status(PressPodsJobStatus::Processing).0,
            StatusKind::Running
        );
        assert_eq!(job_status(PressPodsJobStatus::Failed).0, StatusKind::Fault);
        assert_eq!(job_status(PressPodsJobStatus::Queued).0, StatusKind::Info);
    }
}
