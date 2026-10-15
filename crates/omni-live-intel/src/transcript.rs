//! Speaker-attributed transcripts for rolling summaries.

use crate::speech::SpeakerProfile;

/// One transcribed speaker turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscribedTurn {
    pub speaker: u32,
    pub text: String,
}

fn label(speakers: &[SpeakerProfile], speaker: u32) -> &str {
    speakers
        .iter()
        .find(|p| p.speaker == speaker)
        .map_or("Unknown speaker", |p| p.label.as_str())
}

/// One line per change of speaker, `Label: text`. A transcript whose only
/// speaker is unnamed is plain text: a lone "Speaker 1" label adds nothing.
pub fn format_attributed(turns: &[TranscribedTurn], speakers: &[SpeakerProfile]) -> String {
    let spoken: Vec<&TranscribedTurn> = turns.iter().filter(|t| !t.text.is_empty()).collect();
    let single_unnamed = spoken
        .first()
        .is_some_and(|first| spoken.iter().all(|t| t.speaker == first.speaker))
        && spoken
            .first()
            .is_none_or(|first| label(speakers, first.speaker) != "Destiny");
    if single_unnamed {
        return spoken
            .iter()
            .map(|t| t.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
    }
    let mut lines: Vec<(u32, String)> = Vec::new();
    for turn in spoken {
        match lines.last_mut() {
            Some((speaker, text)) if *speaker == turn.speaker => {
                text.push(' ');
                text.push_str(&turn.text);
            }
            _ => lines.push((turn.speaker, turn.text.clone())),
        }
    }
    lines
        .iter()
        .map(|(speaker, text)| format!("{}: {text}", label(speakers, *speaker)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Each speaker's share of the attributed speech, largest first:
/// `Destiny 62%, Speaker 1 30%, Speaker 2 8%`.
pub fn describe_speakers(speakers: &[SpeakerProfile]) -> String {
    let total: f64 = speakers.iter().map(|p| p.seconds).sum();
    if total <= 0.0 {
        return "none detected".to_owned();
    }
    let mut ordered: Vec<&SpeakerProfile> = speakers.iter().collect();
    ordered.sort_by(|a, b| b.seconds.total_cmp(&a.seconds));
    ordered
        .iter()
        .map(|p| {
            let percent = omni_core::js::math_round(p.seconds / total * 100.0);
            format!("{} {}%", p.label, omni_core::js::number_to_string(percent))
        })
        .collect::<Vec<_>>()
        .join(", ")
}
