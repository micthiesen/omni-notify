//! "Taste Brain" section shared by the media and podcast pages.

use leptos::prelude::*;
use omni_api::media::{TasteClaim, TasteProfile};
use omni_api::podcasts::{PodcastTasteClaim, PodcastTasteProfile};
use omni_web_kit::utils::format::{format_absolute, format_relative};
use omni_web_kit::utils::js::{js_round, number_string};

/// One claim with the number of evidence items behind it.
#[derive(Clone, Debug, PartialEq)]
pub struct TasteClaimView {
    pub claim: String,
    pub confidence: f64,
    pub evidence_count: usize,
}

impl From<&TasteClaim> for TasteClaimView {
    fn from(claim: &TasteClaim) -> Self {
        Self {
            claim: claim.claim.clone(),
            confidence: claim.confidence,
            evidence_count: claim.evidence_ids.len(),
        }
    }
}

impl From<&PodcastTasteClaim> for TasteClaimView {
    fn from(claim: &PodcastTasteClaim) -> Self {
        Self {
            claim: claim.claim.clone(),
            confidence: claim.confidence,
            evidence_count: claim.evidence_ids.len(),
        }
    }
}

/// The claim-based fields shared by the media and podcast taste profiles.
#[derive(Clone, Debug, PartialEq)]
pub struct TasteBrainProfile {
    pub version: i64,
    pub generated_at: i64,
    pub summary: String,
    pub stable_preferences: Vec<TasteClaimView>,
    pub conditional_preferences: Vec<TasteClaimView>,
    pub aversions: Vec<TasteClaimView>,
    pub current_saturation: Vec<TasteClaimView>,
    pub exploration_targets: Vec<TasteClaimView>,
    pub uncertainties: Vec<TasteClaimView>,
}

fn claims<T>(list: &[T]) -> Vec<TasteClaimView>
where
    for<'a> &'a T: Into<TasteClaimView>,
{
    list.iter().map(Into::into).collect()
}

impl From<&TasteProfile> for TasteBrainProfile {
    fn from(p: &TasteProfile) -> Self {
        Self {
            version: p.version,
            generated_at: p.generated_at,
            summary: p.summary.clone(),
            stable_preferences: claims(&p.stable_preferences),
            conditional_preferences: claims(&p.conditional_preferences),
            aversions: claims(&p.aversions),
            current_saturation: claims(&p.current_saturation),
            exploration_targets: claims(&p.exploration_targets),
            uncertainties: claims(&p.uncertainties),
        }
    }
}

impl From<&PodcastTasteProfile> for TasteBrainProfile {
    fn from(p: &PodcastTasteProfile) -> Self {
        Self {
            version: p.version,
            generated_at: p.generated_at,
            summary: p.summary.clone(),
            stable_preferences: claims(&p.stable_preferences),
            conditional_preferences: claims(&p.conditional_preferences),
            aversions: claims(&p.aversions),
            current_saturation: claims(&p.current_saturation),
            exploration_targets: claims(&p.exploration_targets),
            uncertainties: claims(&p.uncertainties),
        }
    }
}

fn evidence_title(count: usize) -> String {
    format!(
        "{count} supporting evidence item{}",
        if count == 1 { "" } else { "s" }
    )
}

fn claim_list(claims: Vec<TasteClaimView>) -> impl IntoView {
    view! {
        <ul class="taste-claim-list">
            {claims
                .into_iter()
                .map(|item| {
                    view! {
                        <li>
                            <span>{item.claim}</span>
                            <span class="taste-confidence" title=evidence_title(item.evidence_count)>
                                {format!("{}%", number_string(js_round(item.confidence * 100.0)))}
                            </span>
                        </li>
                    }
                })
                .collect_view()}
        </ul>
    }
}

fn tag_group(label: &'static str, claims: Vec<TasteClaimView>, explore: bool) -> impl IntoView {
    let class = format!(
        "taste-tag {}",
        if explore { "taste-tag-explore" } else { "" }
    );
    view! {
        <div>
            <span class="taste-tags-label">{label}</span>
            <div class="taste-tags">
                {claims
                    .into_iter()
                    .map(|target| {
                        view! {
                            <span
                                class=class.clone()
                                title=format!(
                                    "{} supporting evidence item(s)",
                                    target.evidence_count,
                                )
                            >
                                {target.claim}
                            </span>
                        }
                    })
                    .collect_view()}
            </div>
        </div>
    }
}

fn version_badge(profile: &TasteBrainProfile) -> impl IntoView + use<> {
    let at = profile.generated_at as f64;
    view! {
        <span class="taste-version meta-row" title=format_absolute(at)>
            <span>{format!("v{}", profile.version)}</span>
            <span>{format_relative(at)}</span>
        </span>
    }
}

fn profile_card(
    profile: TasteBrainProfile,
    stats: Vec<(String, String)>,
    footer: Option<AnyView>,
) -> impl IntoView {
    let columns: Vec<(&'static str, Vec<TasteClaimView>)> = vec![
        ("Reliable Preferences", profile.stable_preferences),
        ("Depends on Context", profile.conditional_preferences),
        ("Avoid", profile.aversions),
        ("Still Learning", profile.uncertainties),
    ];
    let explore = profile.exploration_targets;
    let saturated = profile.current_saturation;
    let tags = (!explore.is_empty() || !saturated.is_empty()).then(|| {
        view! {
            <div class="taste-tags-row">
                {(!explore.is_empty()).then(|| tag_group("Explore", explore, true))}
                {(!saturated.is_empty()).then(|| tag_group("Currently Saturated", saturated, false))}
            </div>
        }
    });
    view! {
        <div class="taste-card">
            <p class="taste-summary">{profile.summary}</p>
            {(!stats.is_empty())
                .then(|| {
                    view! {
                        <div class="taste-stats">
                            {stats
                                .into_iter()
                                .map(|(name, value)| {
                                    view! {
                                        <div class="taste-stat">
                                            <span>{name}</span>
                                            <strong>{value}</strong>
                                        </div>
                                    }
                                })
                                .collect_view()}
                        </div>
                    }
                })}
            <div class="taste-columns">
                {columns
                    .into_iter()
                    .filter(|(_, claims)| !claims.is_empty())
                    .map(|(title, claims)| {
                        view! {
                            <div>
                                <h3>{title}</h3>
                                {claim_list(claims)}
                            </div>
                        }
                    })
                    .collect_view()}
            </div>
            {tags}
            {footer}
        </div>
    }
}

/// `stats` and `footer` are derived from the profile by the caller; `footer`
/// renders only while a profile is present.
#[component]
pub fn TasteBrain(
    #[prop(into)] profile: Signal<Option<TasteBrainProfile>>,
    #[prop(into)] loading: Signal<bool>,
    #[prop(into)] error: Signal<Option<String>>,
    subtitle: &'static str,
    empty_text: &'static str,
    #[prop(into)] stats: Signal<Vec<(String, String)>>,
    #[prop(optional, into)] footer: Option<ViewFn>,
    #[prop(optional)] collapsible: bool,
) -> impl IntoView {
    let content = move || {
        let loading = loading.get();
        let error = error.get();
        let profile = profile.get();
        let footer = footer.clone();
        view! {
            {loading.then(|| view! { <div class="loading-inline">"Loading taste profile…"</div> })}
            {match (&error, loading) {
                (Some(error), false) => {
                    Some(view! { <div class="error-inline">"Taste profile unavailable: " {error.clone()}</div> })
                }
                _ => None,
            }}
            {(!loading && error.is_none() && profile.is_none())
                .then(|| view! { <div class="taste-empty">{empty_text}</div> })}
            {profile.map(|profile| profile_card(profile, stats.get(), footer.map(|f| f.run())))}
        }
    };

    if collapsible {
        view! {
            <details class="page-section taste-brain taste-disclosure">
                <summary>
                    <span class="taste-heading">
                        <span>
                            <span class="section-title" role="heading" aria-level="2">
                                "Taste Brain"
                            </span>
                            <span class="muted taste-subtitle">{subtitle}</span>
                        </span>
                        {move || profile.with(|p| p.as_ref().map(version_badge))}
                    </span>
                </summary>
                <div class="taste-disclosure-body">{content}</div>
            </details>
        }
        .into_any()
    } else {
        view! {
            <section class="page-section taste-brain">
                <div class="taste-heading">
                    <div>
                        <h2 class="section-title">"Taste Brain"</h2>
                        <div class="muted taste-subtitle">{subtitle}</div>
                    </div>
                    {move || profile.with(|p| p.as_ref().map(version_badge))}
                </div>
                {content}
            </section>
        }
        .into_any()
    }
}
