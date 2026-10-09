//! Deterministic recovery policy (`src/arr-recovery/policy.ts`).
//!
//! Regular expressions keep JS semantics: ASCII word boundaries and digits.
//! Each pattern is compiled once; an uncompilable pattern (ruled out by the
//! unit tests) fails closed.

use std::cmp::Ordering;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value, json};

use super::types::{
    ArrKind, Decision, DecisionSource, Evidence, Grab, ImportFile, QueueItem, Target,
};
use crate::{js_text, paths};

type Pattern = LazyLock<Option<Regex>>;

const IMPORT_FAILURE_STATES: [&str; 2] = ["importblocked", "importpending"];
const TRACKED_FAILURE_STATUSES: [&str; 2] = ["warning", "error"];
const ACTIVE_STATES: [&str; 8] = [
    "downloading",
    "queued",
    "paused",
    "parcheck",
    "parrepair",
    "repairing",
    "unpacking",
    "importing",
];

const B: &str = r"(?-u:\b)";
/// JS `.`: any character except a line terminator.
const DOT: &str = r"[^\n\r\u{2028}\u{2029}]";

static UNSAFE_FAILURE: Pattern = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i){B}(permission|access denied|unauthori[sz]ed|read[- ]?only|disk (?:full|space)|no space|filesystem|input/output|i/o error|sample|corrupt|damaged|crc|checksum|unpack(?:ing)? failed|invalid (?:video|media)|cannot (?:read|open)|failed to (?:read|open)|path does not exist){B}"
    ))
    .ok()
});
static HARD_UNSAFE_FAILURE: Pattern = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i){B}(permission|access denied|unauthori[sz]ed|read[- ]?only|disk (?:full|space)|no space|filesystem|input/output|i/o error|corrupt|damaged|crc|checksum|unpack(?:ing)? failed|invalid (?:video|media)|cannot (?:read|open)|failed to (?:read|open)|path does not exist){B}"
    ))
    .ok()
});
static NO_ELIGIBLE_FILES: Pattern = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i){B}no files? (?:found (?:are )?)?eligible for import{B}"
    ))
    .ok()
});
static SAMPLE_REJECTION: Pattern = LazyLock::new(|| Regex::new(&format!(r"(?i){B}sample{B}")).ok());
static QUALITY_REJECTION: Pattern = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i){B}(existing|current|on disk){B}{DOT}*{B}(?:equal|higher|better|upgrade|cutoff|quality|custom format|score){B}|{B}not (?:an? )?(?:quality|custom format|custom-format|cf)? ?upgrade{B}|{B}does not improve{B}{DOT}*{B}(?:quality|custom format|score){B}"
    ))
    .ok()
});
static EPISODE_MARKER: Pattern =
    LazyLock::new(|| Regex::new(r"(?i)s[0-9]{1,2}[ ._-]*e[0-9]{1,3}").ok());
static EPISODE_KEYS: Pattern = LazyLock::new(|| {
    Regex::new(r"(?i)s([0-9]{1,2})[ ._-]*e([0-9]{1,3})(?:[ ._-]*e([0-9]{1,3}))?").ok()
});
static YEARS: Pattern =
    LazyLock::new(|| Regex::new(r"(?:^|[^0-9])((?:19|20)[0-9]{2})(?:[^0-9]|$)").ok());

/// `pattern.test(text)`; `if_invalid` when the pattern failed to compile.
fn test(pattern: &Pattern, text: &str, if_invalid: bool) -> bool {
    match pattern.as_ref() {
        Some(re) => re.is_match(text),
        None => if_invalid,
    }
}

fn is_unsafe(text: &str) -> bool {
    test(&UNSAFE_FAILURE, text, true)
}

fn is_hard_unsafe(text: &str) -> bool {
    test(&HARD_UNSAFE_FAILURE, text, true)
}

fn is_no_eligible_files(text: &str) -> bool {
    test(&NO_ELIGIBLE_FILES, text, false)
}

fn is_quality_rejection(text: &str) -> bool {
    test(&QUALITY_REJECTION, text, false)
}

fn is_sample_rejection(text: &str) -> bool {
    test(&SAMPLE_REJECTION, text, false)
}

/// `normalizedEnum`: lower case with everything but `[a-z0-9]` removed.
pub fn normalized_enum(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .collect()
}

fn status_text(item: &QueueItem) -> impl Iterator<Item = &str> {
    item.status_messages.iter().flat_map(|message| {
        std::iter::once(message.title.as_str()).chain(message.messages.iter().map(String::as_str))
    })
}

fn has_substantive_messages(item: &QueueItem) -> bool {
    item.status_messages
        .iter()
        .flat_map(|message| &message.messages)
        .any(|message| omni_core::js::utf16_len(js_text::trim(message)) >= 5)
}

fn is_normal_import_failure(item: &QueueItem) -> bool {
    normalized_enum(&item.status) == "completed"
        && item.sizeleft == 0.0
        && TRACKED_FAILURE_STATUSES
            .contains(&normalized_enum(&item.tracked_download_status).as_str())
        && IMPORT_FAILURE_STATES.contains(&normalized_enum(&item.tracked_download_state).as_str())
        && has_substantive_messages(item)
}

fn download_health(evidence: &Evidence) -> String {
    evidence
        .download_health
        .as_deref()
        .map(normalized_enum)
        .unwrap_or_default()
}

fn is_terminal_no_files_failure(evidence: &Evidence) -> bool {
    let unhealthy_client = [
        "warninghealth",
        "failurehealth",
        "failurepar",
        "failureunpack",
    ]
    .contains(&download_health(evidence).as_str());
    unhealthy_client
        && !evidence.items.is_empty()
        && evidence.items.iter().all(|item| {
            ["completed", "failed", "warning"].contains(&normalized_enum(&item.status).as_str())
                && TRACKED_FAILURE_STATUSES
                    .contains(&normalized_enum(&item.tracked_download_status).as_str())
                && status_text(item).any(is_no_eligible_files)
        })
}

fn is_terminal_no_files_queue_item(item: &QueueItem) -> bool {
    item.sizeleft == 0.0
        && ["completed", "failed", "warning"].contains(&normalized_enum(&item.status).as_str())
        && TRACKED_FAILURE_STATUSES
            .contains(&normalized_enum(&item.tracked_download_status).as_str())
        && ["importblocked", "importpending", "failedpending", "failed"]
            .contains(&normalized_enum(&item.tracked_download_state).as_str())
        && status_text(item).any(is_no_eligible_files)
}

/// Only settled queue failures with explicit Arr diagnostics enter recovery.
pub fn eligible_queue_item(item: &QueueItem) -> bool {
    let state = normalized_enum(&item.tracked_download_state);
    let status = normalized_enum(&item.status);
    if ACTIVE_STATES.contains(&state.as_str()) || ACTIVE_STATES.contains(&status.as_str()) {
        return false;
    }
    is_normal_import_failure(item) || is_terminal_no_files_queue_item(item)
}

/// JS default `Array#sort` order: UTF-16 code unit comparison.
fn js_default_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// Sorts values by `JSON.stringify(a).localeCompare(JSON.stringify(b))` (stable).
fn sort_by_json_locale(values: &mut Vec<Value>) {
    let mut keyed: Vec<(String, Value)> = values
        .drain(..)
        .map(|value| (omni_core::js::json_stringify(&value), value))
        .collect();
    keyed.sort_by(|(a, _), (b, _)| omni_core::js::locale_compare(a, b));
    values.extend(keyed.into_iter().map(|(_, value)| value));
}

fn number(n: f64) -> Value {
    serde_json::Number::from_f64(n).map_or(Value::Null, Value::Number)
}

/// A stable identity for one observed queue failure, independent of queue ordering.
pub fn observation_fingerprint(items: &[QueueItem]) -> String {
    let mut failures: Vec<Value> = items
        .iter()
        .map(|item| {
            let mut messages: Vec<Value> = item
                .status_messages
                .iter()
                .map(|message| {
                    let mut lines: Vec<String> = message
                        .messages
                        .iter()
                        .map(|line| js_text::trim(line).to_owned())
                        .filter(|line| !line.is_empty())
                        .collect();
                    lines.sort_by(|a, b| js_default_cmp(a, b));
                    json!({ "title": js_text::trim(&message.title), "messages": lines })
                })
                .collect();
            sort_by_json_locale(&mut messages);
            let mut failure = Map::new();
            failure.insert("downloadId".into(), json!(item.download_id));
            failure.insert("title".into(), json!(item.title));
            if let Some(output_path) = &item.output_path {
                failure.insert("outputPath".into(), json!(output_path));
            }
            if let Some(series_id) = item.series_id {
                failure.insert("seriesId".into(), json!(series_id));
            }
            if let Some(episode_id) = item.episode_id {
                failure.insert("episodeId".into(), json!(episode_id));
            }
            if let Some(movie_id) = item.movie_id {
                failure.insert("movieId".into(), json!(movie_id));
            }
            failure.insert("size".into(), number(item.size));
            failure.insert("sizeleft".into(), number(item.sizeleft));
            if let Some(added) = &item.added {
                failure.insert("added".into(), json!(added));
            }
            failure.insert("status".into(), json!(normalized_enum(&item.status)));
            failure.insert(
                "trackedDownloadStatus".into(),
                json!(normalized_enum(&item.tracked_download_status)),
            );
            failure.insert(
                "trackedDownloadState".into(),
                json!(normalized_enum(&item.tracked_download_state)),
            );
            failure.insert("messages".into(), Value::Array(messages));
            Value::Object(failure)
        })
        .collect();
    sort_by_json_locale(&mut failures);
    omni_core::digest::sha256_hex(omni_core::js::json_stringify(&Value::Array(failures)))
}

trait TargetIds {
    fn series_id(&self) -> Option<i64>;
    fn movie_id(&self) -> Option<i64>;
}

impl TargetIds for Grab {
    fn series_id(&self) -> Option<i64> {
        self.series_id
    }
    fn movie_id(&self) -> Option<i64> {
        self.movie_id
    }
}

impl TargetIds for ImportFile {
    fn series_id(&self) -> Option<i64> {
        self.series_id
    }
    fn movie_id(&self) -> Option<i64> {
        self.movie_id
    }
}

fn target_id_matches(kind: ArrKind, value: &impl TargetIds, id: i64) -> bool {
    match kind {
        ArrKind::Sonarr => value.series_id() == Some(id),
        ArrKind::Radarr => value.movie_id() == Some(id),
    }
}

pub fn has_matching_grab_history(evidence: &Evidence) -> bool {
    let mut download_ids: Vec<&str> = evidence
        .items
        .iter()
        .map(|item| item.download_id.as_str())
        .collect();
    download_ids.sort_unstable();
    download_ids.dedup();
    if download_ids.is_empty() {
        return false;
    }
    download_ids.iter().all(|download_id| {
        let grabs: Vec<&Grab> = evidence
            .grabs
            .iter()
            .filter(|grab| {
                grab.download_id == *download_id
                    && normalized_enum(&grab.event_type) == "grabbed"
                    && target_id_matches(evidence.kind, *grab, evidence.target.id)
            })
            .collect();
        !grabs.is_empty()
            && (evidence.kind == ArrKind::Radarr
                || evidence
                    .target
                    .episode_ids
                    .iter()
                    .all(|id| grabs.iter().any(|grab| grab.episode_id == Some(*id))))
    })
}

fn intended_episode_ids(target: &Target) -> Vec<i64> {
    let mut ids = target.episode_ids.clone();
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn exact_target_mapping(evidence: &Evidence) -> bool {
    if evidence.files.is_empty() {
        return false;
    }
    if evidence.kind == ArrKind::Radarr {
        return evidence
            .files
            .iter()
            .all(|file| file.movie_id == Some(evidence.target.id) && file.episode_ids.is_empty());
    }
    let intended = intended_episode_ids(&evidence.target);
    if intended.is_empty() {
        return false;
    }
    let mut mapped: Vec<i64> = Vec::new();
    for file in &evidence.files {
        if file.series_id != Some(evidence.target.id) || file.episode_ids.is_empty() {
            return false;
        }
        for episode_id in &file.episode_ids {
            if !intended.contains(episode_id) || mapped.contains(episode_id) {
                return false;
            }
            mapped.push(*episode_id);
        }
    }
    mapped.len() == intended.len()
}

/// `normalizeTitle`: NFKD without combining marks, lower case, `&` as "and",
/// non-alphanumerics collapsed to single spaces.
pub fn normalize_title(value: &str) -> String {
    let decomposed = icu_normalizer::DecomposingNormalizerBorrowed::new_nfkd().normalize(value);
    let stripped: String = decomposed
        .chars()
        .filter(|c| !('\u{0300}'..='\u{036f}').contains(c))
        .collect::<String>()
        .to_lowercase()
        .replace('&', " and ");
    let mut out = String::with_capacity(stripped.len());
    let mut pending_space = false;
    for c in stripped.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push(c);
        } else {
            pending_space = true;
        }
    }
    out
}

fn title_candidates(target: &Target) -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();
    for title in std::iter::once(&target.title).chain(&target.alternate_titles) {
        let normalized = normalize_title(title);
        if omni_core::js::utf16_len(&normalized) > 1 && !candidates.contains(&normalized) {
            candidates.push(normalized);
        }
    }
    candidates
}

fn file_basename(file: &ImportFile) -> &str {
    paths::basename(if file.name.is_empty() {
        &file.path
    } else {
        &file.name
    })
}

/// Byte index where `(?:^|[^0-9])<year>(?=[^0-9]|$)` first matches.
fn year_marker_index(filename: &str, year: &str) -> Option<usize> {
    if year.is_empty() {
        return None;
    }
    let bytes = filename.as_bytes();
    let mut from = 0;
    while let Some(offset) = filename.get(from..).and_then(|rest| rest.find(year)) {
        let start = from + offset;
        let end = start + year.len();
        let before_ok = start == 0 || !bytes[start - 1].is_ascii_digit();
        let after_ok = end == bytes.len() || !bytes[end].is_ascii_digit();
        if before_ok && after_ok {
            return Some(
                filename[..start]
                    .char_indices()
                    .next_back()
                    .map_or(0, |(index, _)| index),
            );
        }
        from = start + 1;
    }
    None
}

fn filename_matches_title(file: &ImportFile, target: &Target) -> bool {
    let filename = file_basename(file);
    let marker = if file.episode_ids.is_empty() {
        year_marker_index(filename, &target.year.to_string())
    } else {
        EPISODE_MARKER
            .as_ref()
            .and_then(|re| re.find(filename))
            .map(|m| m.start())
    };
    let Some(index) = marker else {
        return false;
    };
    let prefix = normalize_title(&filename[..index]);
    title_candidates(target)
        .iter()
        .any(|candidate| prefix == *candidate || prefix == format!("{candidate} {}", target.year))
}

fn episode_keys_from_filename(filename: &str) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    let Some(re) = EPISODE_KEYS.as_ref() else {
        return keys;
    };
    let mut add = |key: String| {
        if !keys.contains(&key) {
            keys.push(key);
        }
    };
    for captures in re.captures_iter(filename) {
        let num = |i: usize| captures.get(i).and_then(|m| m.as_str().parse::<u32>().ok());
        let (Some(season), Some(episode)) = (num(1), num(2)) else {
            continue;
        };
        add(format!("{season}:{episode}"));
        if let Some(second) = num(3) {
            add(format!("{season}:{second}"));
        }
    }
    keys
}

fn filename_matches_target(evidence: &Evidence, file: &ImportFile) -> bool {
    if !filename_matches_title(file, &evidence.target) {
        return false;
    }
    let filename = file_basename(file);
    let years: Vec<i64> = YEARS
        .as_ref()
        .map(|re| {
            re.captures_iter(filename)
                .filter_map(|c| c.get(1).and_then(|m| m.as_str().parse().ok()))
                .collect()
        })
        .unwrap_or_default();
    let year = evidence.target.year;
    if !years.is_empty() && year > 0 && !years.contains(&year) {
        return false;
    }
    if evidence.kind == ArrKind::Radarr {
        if year <= 0 {
            return true;
        }
        return years.contains(&year);
    }
    let mut expected: Vec<String> = Vec::new();
    for id in &file.episode_ids {
        if let Some(episode) = evidence.target.episodes.iter().find(|e| e.id == *id) {
            let key = format!("{}:{}", episode.season_number, episode.episode_number);
            if !expected.contains(&key) {
                expected.push(key);
            }
        }
    }
    let observed = episode_keys_from_filename(filename);
    expected.len() == file.episode_ids.len()
        && observed.len() == expected.len()
        && expected.iter().all(|key| observed.contains(key))
}

fn file_is_confined_to_output(file: &ImportFile, items: &[QueueItem]) -> bool {
    if !paths::is_absolute(&file.path) {
        return false;
    }
    let file_path = paths::resolve(&file.path);
    items.iter().any(|item| {
        let Some(output_path) = item
            .output_path
            .as_deref()
            .filter(|p| paths::is_absolute(p))
        else {
            return false;
        };
        let relative = paths::relative(&paths::resolve(output_path), &file_path);
        relative.is_empty() || (!relative.starts_with("..") && !paths::is_absolute(&relative))
    })
}

fn rejection_reasons(evidence: &Evidence) -> impl Iterator<Item = &str> {
    evidence
        .files
        .iter()
        .flat_map(|file| file.rejections.iter().map(|r| r.reason.as_str()))
}

pub fn has_unsafe_failure_evidence(evidence: &Evidence) -> bool {
    evidence
        .items
        .iter()
        .flat_map(status_text)
        .chain(rejection_reasons(evidence))
        .any(|message| is_unsafe(message) && !is_no_eligible_files(message))
}

pub fn can_safely_import(evidence: &Evidence) -> bool {
    can_structurally_import(evidence)
        && evidence
            .files
            .iter()
            .all(|file| filename_matches_target(evidence, file))
}

/// Non-semantic invariants that an LLM verdict is never allowed to override.
pub fn can_structurally_import(evidence: &Evidence) -> bool {
    !evidence.items.is_empty()
        && evidence.items.iter().all(is_normal_import_failure)
        && has_matching_grab_history(evidence)
        && exact_target_mapping(evidence)
        && evidence.files.iter().all(|file| {
            file.rejections.is_empty() && file_is_confined_to_output(file, &evidence.items)
        })
        && !has_unsafe_failure_evidence(evidence)
}

pub fn has_needed_valid_partial_file(evidence: &Evidence) -> bool {
    let intended = intended_episode_ids(&evidence.target);
    evidence.files.iter().any(|file| {
        file.rejections.is_empty()
            && target_id_matches(evidence.kind, file, evidence.target.id)
            && (evidence.kind == ArrKind::Radarr
                || (!file.episode_ids.is_empty()
                    && file.episode_ids.iter().all(|id| intended.contains(id))))
    })
}

fn all_intended_files_already_exist(evidence: &Evidence) -> bool {
    if evidence.kind == ArrKind::Radarr {
        return evidence.target.has_file;
    }
    let intended = intended_episode_ids(&evidence.target);
    !intended.is_empty()
        && intended.iter().all(|id| {
            evidence
                .target
                .episodes
                .iter()
                .find(|episode| episode.id == *id)
                .is_some_and(|episode| episode.has_file)
        })
}

fn only_quality_downgrade_rejections(evidence: &Evidence) -> bool {
    !evidence.files.is_empty()
        && evidence.files.iter().all(|file| {
            !file.rejections.is_empty()
                && file
                    .rejections
                    .iter()
                    .any(|r| is_quality_rejection(&r.reason))
                && file
                    .rejections
                    .iter()
                    .all(|r| is_quality_rejection(&r.reason) || is_sample_rejection(&r.reason))
        })
        && !evidence
            .items
            .iter()
            .flat_map(status_text)
            .chain(rejection_reasons(evidence))
            .any(is_hard_unsafe)
}

/// Conservative deterministic action. Ambiguity is left for the guarded LLM fallback.
pub fn decide(evidence: &Evidence) -> Decision {
    let rules = DecisionSource::Rules;
    if evidence.items.is_empty() || evidence.items.iter().any(|item| !eligible_queue_item(item)) {
        return Decision::defer("Download is not a settled, diagnosed import failure", rules);
    }
    if has_matching_grab_history(evidence)
        && exact_target_mapping(evidence)
        && all_intended_files_already_exist(evidence)
        && only_quality_downgrade_rejections(evidence)
    {
        return Decision::Remove {
            reason: "Every intended item already has a file and the download is only a downgrade"
                .to_owned(),
            source: rules,
            replace: false,
        };
    }
    if has_unsafe_failure_evidence(evidence) {
        return Decision::defer(
            "Failure evidence may indicate an infrastructure or media-integrity problem",
            rules,
        );
    }
    if can_safely_import(evidence) {
        return Decision::Import {
            reason: "Completed download has an exact, rejection-free target and filename mapping"
                .to_owned(),
            source: rules,
        };
    }
    if is_terminal_no_files_failure(evidence)
        && has_matching_grab_history(evidence)
        && evidence.files.is_empty()
    {
        return Decision::Remove {
            reason: "Download client confirms a terminal failure with no files available to import"
                .to_owned(),
            source: rules,
            replace: true,
        };
    }
    Decision::defer(
        "Evidence does not support a safe deterministic recovery action",
        rules,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pattern_compiles() {
        for pattern in [
            &UNSAFE_FAILURE,
            &HARD_UNSAFE_FAILURE,
            &NO_ELIGIBLE_FILES,
            &SAMPLE_REJECTION,
            &QUALITY_REJECTION,
            &EPISODE_MARKER,
            &EPISODE_KEYS,
            &YEARS,
        ] {
            assert!(pattern.is_some());
        }
    }

    #[test]
    fn normalizes_titles_like_js() {
        assert_eq!(normalize_title("Amélie & Co."), "amelie and co");
        assert_eq!(
            normalize_title("  House.of.Cards.US. "),
            "house of cards us"
        );
        assert_eq!(normalize_title("哭声"), "");
    }

    #[test]
    fn finds_year_markers_like_the_lookahead_regex() {
        assert_eq!(year_marker_index("Movie.2024-GROUP", "2024"), Some(5));
        assert_eq!(year_marker_index("2024.Movie", "2024"), Some(0));
        assert_eq!(year_marker_index("Movie12024", "2024"), None);
        assert_eq!(year_marker_index("Movie.20245.2024", "2024"), Some(11));
    }

    #[test]
    fn word_boundaries_are_ascii() {
        assert!(is_unsafe("Permission denied"));
        assert!(!is_unsafe("permissions"));
        assert!(is_quality_rejection(
            "Not a Custom Format upgrade for existing episode file"
        ));
        assert!(is_quality_rejection(
            "Not an upgrade for existing episode file(s). Existing quality: WEBDL-1080p."
        ));
        assert!(is_no_eligible_files(
            "No files found are eligible for import in /tmp/inter"
        ));
    }
}
