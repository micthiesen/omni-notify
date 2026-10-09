//! "Taste brain" panel shared by the media and podcast pages: the profile
//! summary, behaviour stats and claim groups as key-value bars.

use leptos::prelude::*;
use omni_api::media::{TasteClaim, TasteProfile};
use omni_api::podcasts::{PodcastTasteClaim, PodcastTasteProfile};
use omni_web_kit::components::{Disclosure, EmptyState, ErrorState, Meter, Panel, Skeleton};
use omni_web_kit::hooks::use_is_wide;
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

/// Claims shown per group before the rest fold into a disclosure.
const CLAIMS_SHOWN: usize = 4;

fn evidence_title(count: usize) -> String {
    format!(
        "{count} supporting evidence item{}",
        if count == 1 { "" } else { "s" }
    )
}

fn percent(confidence: f64) -> String {
    format!("{}%", number_string(js_round(confidence * 100.0)))
}

fn claim_row(item: TasteClaimView) -> impl IntoView {
    let value = (item.confidence * 100.0).clamp(0.0, 100.0);
    view! {
        <li title=evidence_title(item.evidence_count)>
            <span class="taste-claim">{item.claim}</span>
            <Meter value=value max=100.0 width=48 label=format!("Confidence {}", percent(item.confidence))/>
            <span class="num taste-pct">{percent(item.confidence)}</span>
        </li>
    }
}

fn claim_group(title: &'static str, claims: Vec<TasteClaimView>) -> impl IntoView {
    let count = claims.len();
    let mut shown = claims;
    let rest = shown.split_off(count.min(CLAIMS_SHOWN));
    let more = (!rest.is_empty()).then(|| {
        let n = rest.len();
        view! {
            <Disclosure summary=format!("{n} more") flush=true>
                <ul class="taste-claims">{rest.into_iter().map(claim_row).collect_view()}</ul>
            </Disclosure>
        }
    });
    view! {
        <div class="taste-group">
            <h4 class="label">{title} <span class="seg-n">{count}</span></h4>
            <ul class="taste-claims">{shown.into_iter().map(claim_row).collect_view()}</ul>
            {more}
        </div>
    }
}

/// Free-text notes (exploration targets, saturation) as quiet prose lines.
fn note_group(title: &'static str, claims: Vec<TasteClaimView>, accent: bool) -> impl IntoView {
    view! {
        <div class="taste-group">
            <h4 class="label">{title}</h4>
            <ul class=if accent { "taste-notes accent" } else { "taste-notes" }>
                {claims
                    .into_iter()
                    .map(|claim| view! {
                        <li title=evidence_title(claim.evidence_count)>{claim.claim}</li>
                    })
                    .collect_view()}
            </ul>
        </div>
    }
}

fn version_meta(profile: &TasteBrainProfile) -> impl IntoView + use<> {
    let at = profile.generated_at as f64;
    view! {
        <span class="num" title=format_absolute(at)>
            {format!("v{} · {}", profile.version, format_relative(at))}
        </span>
    }
}

fn profile_body(
    profile: TasteBrainProfile,
    stats: Vec<(String, String)>,
    footer: Option<AnyView>,
) -> impl IntoView {
    let groups: Vec<(&'static str, Vec<TasteClaimView>)> = vec![
        ("Reliable preferences", profile.stable_preferences),
        ("Depends on context", profile.conditional_preferences),
        ("Avoid", profile.aversions),
        ("Still learning", profile.uncertainties),
    ];
    let explore = profile.exploration_targets;
    let saturated = profile.current_saturation;
    let claim_count = groups.iter().map(|(_, c)| c.len()).sum::<usize>();
    let detail_meta = format!(
        "{claim_count} claim{}",
        if claim_count == 1 { "" } else { "s" }
    );
    view! {
        <div class="taste">
            <p class="taste-summary">{profile.summary}</p>
            {(!stats.is_empty()).then(|| view! {
                <dl class="taste-stats">
                    {stats
                        .into_iter()
                        .map(|(name, value)| view! {
                            <div>
                                <dt>{name}</dt>
                                <dd class="num">{value}</dd>
                            </div>
                        })
                        .collect_view()}
                </dl>
            })}
            <Disclosure
                summary="How it decides"
                meta=detail_meta
                flush=true
                class="taste-more"
            >
                <div class="taste">
                    {groups
                        .into_iter()
                        .filter(|(_, claims)| !claims.is_empty())
                        .map(|(title, claims)| claim_group(title, claims))
                        .collect_view()}
                    {(!explore.is_empty()).then(|| note_group("Explore next", explore, true))}
                    {(!saturated.is_empty()).then(|| note_group("Currently saturated", saturated, false))}
                    {footer}
                </div>
            </Disclosure>
        </div>
    }
}

/// `stats` and `footer` are derived from the profile by the caller; `footer`
/// renders only while a profile is present. With `collapsible` the panel
/// becomes a disclosure below the wide tier (where it no longer sits in the
/// side column).
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
    let wide = use_is_wide();
    let body = move || {
        let footer = footer.clone();
        if let Some(profile) = profile.get() {
            return profile_body(profile, stats.get(), footer.map(|f| f.run())).into_any();
        }
        if let Some(error) = error.get().filter(|_| !loading.get()) {
            return view! { <ErrorState title="Taste profile unavailable" raw=error/> }.into_any();
        }
        if loading.get() {
            return view! {
                <div class="taste stack" role="status" aria-label="Loading taste profile">
                    <Skeleton width="90%"/>
                    <Skeleton width="75%"/>
                    <Skeleton width="60%"/>
                </div>
            }
            .into_any();
        }
        view! { <EmptyState message=empty_text compact=true/> }.into_any()
    };
    let meta = move || profile.with(|p| p.as_ref().map(version_meta));
    move || {
        let body = body.clone();
        if collapsible && !wide.get() {
            let summary_meta = Signal::derive(move || {
                profile.with(|p| p.as_ref().map(|p| format!("v{}", p.version)))
            });
            view! {
                <Disclosure summary="Taste brain" meta=summary_meta class="taste-disclosure">
                    <p class="small muted">{subtitle}</p>
                    {body}
                </Disclosure>
            }
            .into_any()
        } else {
            view! {
                <Panel
                    title="Taste brain"
                    head_end=ViewFn::from(move || meta)
                    pad=true
                    class="taste-panel"
                    aria_label=subtitle
                >
                    {body}
                </Panel>
            }
            .into_any()
        }
    }
}
