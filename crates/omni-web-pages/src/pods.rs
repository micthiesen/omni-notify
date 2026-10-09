//! PressPods episode list, submission form and job queue (`pages/PodsPage.tsx`).

use leptos::prelude::*;
use omni_api::common::encode_uri_component;
use omni_api::presspods::{
    PressPodsEpisode, PressPodsJob, PressPodsJobStatus, PressPodsRetrieverAttempt,
};
use omni_api::runs::Run;
use omni_web_kit::api;
use omni_web_kit::components::{
    ImageWithFallback, LogViewer, ShowMoreButton, Toast, ToastHandle, ToastKind, use_show_more,
    use_toast,
};
use omni_web_kit::live::{ConnectionState, use_live_data};
use omni_web_kit::router::Link;
use omni_web_kit::task::{spawn_detached, spawn_scoped};
use omni_web_kit::utils::format::{format_absolute, format_cents};
use omni_web_kit::utils::js::js_round;

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

fn job_status_label(status: PressPodsJobStatus) -> &'static str {
    match status {
        PressPodsJobStatus::Queued => "Queued",
        PressPodsJobStatus::Processing => "Processing",
        PressPodsJobStatus::Failed => "Failed",
    }
}

fn job_status_str(status: PressPodsJobStatus) -> &'static str {
    match status {
        PressPodsJobStatus::Queued => "queued",
        PressPodsJobStatus::Processing => "processing",
        PressPodsJobStatus::Failed => "failed",
    }
}

fn job_status_dot(status: PressPodsJobStatus) -> &'static str {
    match status {
        PressPodsJobStatus::Failed => "error",
        PressPodsJobStatus::Processing => "running",
        PressPodsJobStatus::Queued => "none",
    }
}

pub(crate) fn attempt_success(attempt: &PressPodsRetrieverAttempt) -> bool {
    match attempt {
        PressPodsRetrieverAttempt::Success { success, .. }
        | PressPodsRetrieverAttempt::Failure { success, .. } => *success,
    }
}

/// `retrieverSummary`: the winning retriever and how many succeeded.
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

fn delete_message(title: &str) -> String {
    format!("Delete \"{title}\"? This removes the episode and its audio permanently.")
}

pub(crate) fn confirm_delete(title: &str) -> bool {
    window()
        .confirm_with_message(&delete_message(title))
        .unwrap_or(false)
}

fn mic_placeholder() -> impl IntoView {
    view! {
        <svg
            width="24"
            height="24"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="1.5"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
        >
            <rect x="9" y="2" width="6" height="12" rx="3"></rect>
            <path d="M5 10a7 7 0 0 0 14 0"></path>
            <line x1="12" y1="17" x2="12" y2="21"></line>
            <line x1="8" y1="21" x2="16" y2="21"></line>
        </svg>
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
    let toast = use_toast();

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
        spawn_detached(async move {
            match api::retry_press_pods_episode(&episode_id).await {
                Ok(_) => {
                    reload();
                    toast.show("Re-queued for regeneration", ToastKind::Info);
                }
                Err(err) => toast.show(err.message(), ToastKind::Error),
            }
        });
    };
    let on_delete_episode = move |episode_id: String, title: String| {
        if !confirm_delete(&title) {
            return;
        }
        spawn_detached(async move {
            match api::delete_press_pods_episode(&episode_id).await {
                Ok(_) => {
                    episodes.update(|list| {
                        if let Some(list) = list {
                            list.retain(|e| e.episode_id != episode_id);
                        }
                    });
                    toast.show("Episode deleted", ToastKind::Info);
                }
                Err(err) => toast.show(err.message(), ToastKind::Error),
            }
        });
    };

    let show = use_show_more(
        Signal::derive(move || episodes.get().unwrap_or_default()),
        20,
        Signal::stored(String::new()),
    );

    let job_rows = move || {
        jobs.get()
            .into_iter()
            .map(|job| {
                let failed = job.status == PressPodsJobStatus::Failed;
                let retry_id = job.job_id.clone();
                let dismiss_id = job.job_id.clone();
                view! {
                    <div class=format!("pods-job pods-job-{}", job_status_str(job.status))>
                        <div class="pods-job-main">
                            <span class=format!("status-dot status-{}", job_status_dot(job.status))></span>
                            <span class="pods-job-url" title=job.url.clone()>{job.url.clone()}</span>
                        </div>
                        <div class="meta-row pods-job-meta">
                            <span>{job_status_label(job.status)}</span>
                            <span>{format_absolute(job.created_at as f64)}</span>
                            {(job.attempts > 0).then(|| view! { <span>{format!("Attempt {}", job.attempts)}</span> })}
                        </div>
                        {job
                            .last_error
                            .clone()
                            .filter(|e| !e.is_empty())
                            .map(|e| view! { <div class="run-error">{e}</div> })}
                        <div class="pods-job-actions">
                            {failed
                                .then(|| {
                                    view! {
                                        <button type="button" class="chip-btn" on:click=move |_| on_retry(retry_id.clone())>
                                            "Retry"
                                        </button>
                                        <button type="button" class="chip-btn" on:click=move |_| on_dismiss(dismiss_id.clone())>
                                            "Dismiss"
                                        </button>
                                    }
                                })}
                            {job
                                .last_run_id
                                .clone()
                                .filter(|r| !r.is_empty())
                                .map(|run_id| {
                                    view! {
                                        <button
                                            type="button"
                                            class="chip-btn"
                                            on:click=move |_| open_logs(run_id.clone(), log_run, toast)
                                        >
                                            "Logs"
                                        </button>
                                    }
                                })}
                        </div>
                    </div>
                }
            })
            .collect_view()
    };

    let episode_card = move |episode: PressPodsEpisode| {
        let detail_path = format!("/pods/{}", encode_uri_component(&episode.episode_id));
        let title = episode.title.clone();
        let author = episode.author.clone().filter(|a| !a.is_empty());
        let publication = episode.publication.clone().filter(|p| !p.is_empty());
        let byline = (author.is_some() || publication.is_some()).then(|| {
            view! {
                <div class="meta-row pods-card-byline">
                    {author.map(|a| view! { <span>{a}</span> })}
                    {publication.map(|p| view! { <span>{p}</span> })}
                </div>
            }
        });
        let duration = format_audio_duration(episode.duration_seconds);
        let cost = format_cents(episode.cost_cents);
        let retrievers = retriever_summary(&episode).filter(|s| !s.is_empty());
        let run_id = episode.run_id.clone().filter(|r| !r.is_empty());
        let retry_id = episode.episode_id.clone();
        let delete_id = episode.episode_id.clone();
        let delete_title = episode.title.clone();
        view! {
            <article class="pods-card">
                <div class="pods-card-body">
                    <ImageWithFallback
                        src=episode.lead_image_url.clone()
                        alt=""
                        class="pods-card-art"
                        placeholder_class="pods-card-art-placeholder"
                        placeholder=mic_placeholder
                        lazy=true
                    />
                    <div class="pods-card-info">
                        <h2 class="pods-card-title">
                            <Link to=detail_path class="pods-card-title-link">{title}</Link>
                        </h2>
                        {byline}
                        {episode
                            .excerpt
                            .clone()
                            .filter(|e| !e.is_empty())
                            .map(|e| view! { <p class="pods-card-excerpt">{e}</p> })}
                        <audio
                            class="pods-card-audio"
                            aria-label=format!("Listen to {}", episode.title)
                            controls=true
                            preload="none"
                            src=episode.audio_url.clone()
                        ></audio>
                        <div class="pods-card-links pods-card-article-link">
                            <a href=episode.article_url.clone() target="_blank" rel="noreferrer" class="pods-card-source">
                                {format!("{} ↗", episode.domain.clone().unwrap_or_else(|| "Article".to_owned()))}
                            </a>
                        </div>
                        <div class="meta-row pods-card-meta">
                            <span>{format_absolute(episode.created_at as f64)}</span>
                            {duration.map(|d| view! { <span>{d}</span> })}
                        </div>
                        <details class="content-disclosure pods-card-options">
                            <summary>"Episode Options"</summary>
                            <div class="meta-row pods-card-meta">
                                {episode
                                    .voice_name
                                    .clone()
                                    .filter(|v| !v.is_empty())
                                    .map(|v| view! { <span>{v}</span> })}
                                {cost.map(|c| view! { <span>{c}</span> })}
                                {retrievers.map(|r| view! { <span>{r}</span> })}
                            </div>
                            <div class="pods-card-links pods-card-admin-actions">
                                {run_id
                                    .map(|run_id| {
                                        view! {
                                            <button
                                                type="button"
                                                class="pods-card-logs"
                                                on:click=move |_| open_logs(run_id.clone(), log_run, toast)
                                            >
                                                "Logs"
                                            </button>
                                        }
                                    })}
                                <button
                                    type="button"
                                    class="pods-card-logs"
                                    on:click=move |_| on_retry_episode(retry_id.clone())
                                >
                                    "Retry"
                                </button>
                                <button
                                    type="button"
                                    class="pods-card-logs pods-card-delete"
                                    on:click=move |_| on_delete_episode(delete_id.clone(), delete_title.clone())
                                >
                                    "Delete"
                                </button>
                            </div>
                        </details>
                    </div>
                </div>
            </article>
        }
    };

    view! {
        <div class="page-header">
            <div class="page-header-stack">
                <h1>"PressPods"</h1>
                <p class="page-subtitle">"Your reading list, ready to listen to."</p>
            </div>
        </div>

        <label class="pods-submit-label" for="article-url">
            "Turn an Article Into an Episode"
        </label>
        <form class="pods-submit" on:submit=on_submit>
            <input
                type="url"
                id="article-url"
                aria-describedby=move || submit_error.with(Option::is_some).then_some("article-url-error")
                aria-invalid=move || submit_error.with(Option::is_some).then_some("true")
                class="pods-submit-input"
                placeholder="https://example.com/article"
                prop:value=move || url.get()
                on:input=move |event| url.set(event_target_value(&event))
                disabled=move || submitting.get()
            />
            <button
                type="submit"
                class="pods-submit-btn"
                disabled=move || submitting.get() || url.with(|u| u.trim().is_empty())
            >
                {move || if submitting.get() { "Submitting…" } else { "Create Episode" }}
            </button>
        </form>
        {move || {
            submit_error
                .get()
                .map(|e| {
                    view! {
                        <div id="article-url-error" role="alert" class="pods-submit-error">
                            {e}
                        </div>
                    }
                })
        }}

        {move || {
            (!jobs.with(Vec::is_empty))
                .then(|| {
                    view! {
                        <section class="pods-jobs">
                            <h2 class="section-title">"Episode Queue"</h2>
                            {job_rows}
                        </section>
                    }
                })
        }}

        {move || {
            (episodes.with(Option::is_none) && error.with(Option::is_none))
                .then(|| view! { <div class="loading">"Loading…"</div> })
        }}
        {move || error.get().map(|e| view! { <div class="error">{e}</div> })}
        {move || {
            (episodes.with(|e| e.as_ref().is_some_and(Vec::is_empty)) && jobs.with(Vec::is_empty))
                .then(|| {
                    view! {
                        <div class="muted">"No episodes yet. Submit an article URL above to create one."</div>
                    }
                })
        }}

        {move || {
            episodes
                .with(|e| e.as_ref().is_some_and(|e| !e.is_empty()))
                .then(|| {
                    view! {
                        <div class="pods-feed">
                            <For
                                each=move || show.visible.get()
                                key=|episode| format!("{}|{episode:?}", episode.episode_id)
                                children=episode_card
                            />
                            {move || {
                                show.has_more
                                    .get()
                                    .then(|| {
                                        view! {
                                            <ShowMoreButton
                                                remaining=show.remaining
                                                on_click=Callback::new(move |()| show.show_more())
                                            />
                                        }
                                    })
                            }}
                        </div>
                    }
                })
        }}

        {move || {
            log_run
                .get()
                .map(|run| view! { <LogViewer run=run on_close=Callback::new(move |()| log_run.set(None)) /> })
        }}
        <Toast toast=toast.toast />
    }
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
    fn delete_confirmation_names_the_episode() {
        assert_eq!(
            delete_message("A \"B\""),
            "Delete \"A \"B\"\"? This removes the episode and its audio permanently."
        );
    }
}
