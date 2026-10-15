//! Speaker-attributed transcript formatting for rolling summaries.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use omni_live_intel::speech::SpeakerProfile;
use omni_live_intel::transcript::{TranscribedTurn, describe_speakers, format_attributed};

fn profile(speaker: u32, label: &str, seconds: f64) -> SpeakerProfile {
    SpeakerProfile {
        speaker,
        label: label.into(),
        seconds,
        destiny_confidence: None,
    }
}

fn said(speaker: u32, text: &str) -> TranscribedTurn {
    TranscribedTurn {
        speaker,
        text: text.into(),
    }
}

#[test]
fn labels_each_change_of_speaker_and_joins_consecutive_turns() {
    let speakers = [profile(0, "Destiny", 10.0), profile(1, "Speaker 1", 5.0)];
    let text = format_attributed(
        &[
            said(0, "I think that's wrong."),
            said(0, "Here's why."),
            said(1, ""),
            said(1, "Go on."),
            said(0, "Okay."),
        ],
        &speakers,
    );
    assert_eq!(
        text,
        "Destiny: I think that's wrong. Here's why.\nSpeaker 1: Go on.\nDestiny: Okay."
    );
}

#[test]
fn a_single_unnamed_speaker_is_plain_text() {
    let speakers = [profile(4, "Speaker 1", 10.0), profile(5, "Speaker 2", 1.0)];
    // Speaker 2's only turn transcribed to nothing.
    let text = format_attributed(
        &[
            said(4, "Hello chat."),
            said(5, ""),
            said(4, "Welcome back."),
        ],
        &speakers,
    );
    assert_eq!(text, "Hello chat. Welcome back.");
}

#[test]
fn a_single_destiny_speaker_keeps_the_label() {
    let speakers = [profile(0, "Destiny", 10.0)];
    assert_eq!(
        format_attributed(&[said(0, "Hello chat.")], &speakers),
        "Destiny: Hello chat."
    );
    assert_eq!(format_attributed(&[], &speakers), "");
}

#[test]
fn describes_speaker_shares_largest_first() {
    let speakers = [
        profile(0, "Speaker 1", 30.0),
        profile(1, "Destiny", 62.0),
        profile(2, "Speaker 2", 8.0),
    ];
    assert_eq!(
        describe_speakers(&speakers),
        "Destiny 62%, Speaker 1 30%, Speaker 2 8%"
    );
    assert_eq!(describe_speakers(&[]), "none detected");
}
