//! Voice target scheduling.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use omni_live_intel::observation::{DggPresence, Streamer, StreamerTier};
use omni_live_intel::voice_targets::select_voice_targets;

fn target(id: &str, viewers: f64, hosted: bool) -> Streamer {
    Streamer {
        id: id.into(),
        display_name: id.into(),
        bindings: vec![],
        tier: StreamerTier::Background,
        dgg: Some(DggPresence {
            hosted,
            viewers: Some(viewers),
        }),
    }
}

fn ids(items: Vec<Streamer>) -> Vec<String> {
    items.into_iter().map(|s| s.id).collect()
}

#[test]
fn selects_the_highest_viewed_targets_rather_than_network_completion_order() {
    let xqc = target("dgg:kick:xqc", 7.0, false);
    let prsek = target("prsek", 194.0, false);
    let prying_mind = target("dgg:kick:pryingmind", 381.0, true);
    let bingsamaa = target("dgg:twitch:bingsamaa", 69.0, false);
    assert_eq!(
        ids(select_voice_targets(
            vec![xqc, bingsamaa, prying_mind, prsek],
            3
        )),
        vec!["dgg:kick:pryingmind", "prsek", "dgg:twitch:bingsamaa"]
    );
}

#[test]
fn uses_hosted_state_and_id_as_deterministic_tie_breakers() {
    assert_eq!(
        ids(select_voice_targets(
            vec![
                target("z", 10.0, false),
                target("b", 10.0, true),
                target("a", 10.0, false)
            ],
            3
        )),
        vec!["b", "a", "z"]
    );
}
