//! Arr recovery decision policy.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_arr::arr_recovery::policy::{decide, eligible_queue_item, observation_fingerprint};
use omni_arr::arr_recovery::{
    ArrKind, Decision, DecisionSource, Evidence, Grab, ImportFile, QueueItem, Rejection,
    StatusMessage, Target, TargetEpisode,
};

fn messages(title: &str, lines: &[&str]) -> StatusMessage {
    StatusMessage {
        title: title.to_owned(),
        messages: lines.iter().map(|s| (*s).to_owned()).collect(),
    }
}

fn queue() -> QueueItem {
    QueueItem {
        id: 10,
        download_id: "download-1".into(),
        title: "House.of.Cards.US.S01E01.1080p.WEB-DL".into(),
        status: "completed".into(),
        tracked_download_status: "warning".into(),
        tracked_download_state: "importPending".into(),
        status_messages: vec![messages(
            "Import failed",
            &["Episode was not imported automatically"],
        )],
        size: 1_000.0,
        sizeleft: 0.0,
        output_path: Some("/downloads/House.of.Cards.US.S01E01".into()),
        added: None,
        series_id: Some(42),
        episode_id: None,
        movie_id: None,
        protocol: None,
        download_client: None,
    }
}

fn episode(has_file: bool) -> TargetEpisode {
    TargetEpisode {
        id: 101,
        season_number: 1,
        episode_number: 1,
        title: "Chapter 1".into(),
        has_file,
        monitored: true,
    }
}

fn target() -> Target {
    Target {
        id: 42,
        title: "House of Cards".into(),
        year: 2013,
        monitored: true,
        has_file: false,
        path: "/media/House of Cards".into(),
        episode_ids: vec![101],
        episodes: vec![episode(false)],
        alternate_titles: vec!["House of Cards (US)".into()],
    }
}

fn file() -> ImportFile {
    ImportFile {
        folder_name: None,
        id: 501,
        path: "/downloads/House.of.Cards.US.S01E01/House.of.Cards.US.S01E01.mkv".into(),
        name: "House.of.Cards.US.S01E01.mkv".into(),
        size: 1_000.0,
        series_id: Some(42),
        movie_id: None,
        season_number: Some(1),
        episode_ids: vec![101],
        quality: serde_json::Map::new(),
        languages: None,
        release_group: None,
        indexer_flags: None,
        release_type: None,
        rejections: Vec::new(),
    }
}

fn evidence() -> Evidence {
    Evidence {
        download_health: None,
        kind: ArrKind::Sonarr,
        items: vec![queue()],
        target: target(),
        files: vec![file()],
        grabs: vec![Grab {
            download_id: "download-1".into(),
            source_title: "House.of.Cards.US.S01E01.1080p.WEB-DL".into(),
            series_id: Some(42),
            movie_id: None,
            episode_id: Some(101),
            event_type: "grabbed".into(),
            date: "2026-09-12T00:00:00Z".into(),
        }],
    }
}

fn rejection(kind: &str, reason: &str) -> Rejection {
    Rejection {
        reason: reason.into(),
        kind: kind.into(),
    }
}

fn action(decision: &Decision) -> &'static str {
    decision.action()
}

// describe("eligibleQueueItem")

#[test]
fn accepts_a_completed_diagnosed_import_failure() {
    assert!(eligible_queue_item(&queue()));
}

#[test]
fn rejects_active_state() {
    for state in [
        "downloading",
        "queued",
        "parCheck",
        "unpacking",
        "importing",
    ] {
        let item = QueueItem {
            tracked_download_state: state.into(),
            ..queue()
        };
        assert!(!eligible_queue_item(&item), "{state}");
    }
}

#[test]
fn requires_substantive_diagnostics() {
    let item = QueueItem {
        status_messages: vec![messages("", &[])],
        ..queue()
    };
    assert!(!eligible_queue_item(&item));
}

#[test]
fn requires_the_completed_download_to_have_no_bytes_left() {
    let item = QueueItem {
        sizeleft: 1.0,
        ..queue()
    };
    assert!(!eligible_queue_item(&item));
}

#[test]
fn admits_an_explicit_terminal_no_files_failure_for_further_corroboration() {
    let item = QueueItem {
        status: "failed".into(),
        tracked_download_state: "failed".into(),
        status_messages: vec![messages("No files found are eligible for import", &[])],
        ..queue()
    };
    assert!(eligible_queue_item(&item));
}

// describe("observationFingerprint")

#[test]
fn is_stable_across_queue_and_diagnostic_ordering_and_ignores_ephemeral_queue_ids() {
    let first = QueueItem {
        status_messages: vec![messages("Second", &["B", "A"]), messages("First", &["C"])],
        ..queue()
    };
    let second = QueueItem {
        id: 11,
        download_id: "download-2".into(),
        ..queue()
    };
    let reordered_first = QueueItem {
        id: 999,
        status_messages: vec![messages("First", &["C"]), messages("Second", &["A", "B"])],
        ..first.clone()
    };
    assert_eq!(
        observation_fingerprint(&[first, second.clone()]),
        observation_fingerprint(&[second, reordered_first])
    );
}

#[test]
fn changes_when_the_failure_state_changes() {
    let blocked = QueueItem {
        tracked_download_state: "importBlocked".into(),
        ..queue()
    };
    assert_ne!(
        observation_fingerprint(&[queue()]),
        observation_fingerprint(&[blocked])
    );
}

// describe("decide")

#[test]
fn imports_an_exact_sonarr_mapping_and_supports_the_common_us_title_suffix() {
    let decision = decide(&evidence());
    assert_eq!(action(&decision), "import");
    assert_eq!(decision.source(), DecisionSource::Rules);
}

#[test]
fn never_acts_on_an_active_download() {
    let active = Evidence {
        items: vec![QueueItem {
            status: "downloading".into(),
            tracked_download_state: "downloading".into(),
            sizeleft: 100.0,
            ..queue()
        }],
        target: Target {
            episodes: vec![episode(true)],
            ..target()
        },
        files: vec![ImportFile {
            rejections: vec![rejection(
                "quality",
                "Existing file on disk is of equal or higher quality",
            )],
            ..file()
        }],
        ..evidence()
    };
    assert_eq!(action(&decide(&active)), "defer");
}

#[test]
fn defers_mismatched_episode_numbering() {
    let e = Evidence {
        files: vec![ImportFile {
            name: "House.of.Cards.US.S01E02.mkv".into(),
            ..file()
        }],
        ..evidence()
    };
    assert_eq!(action(&decide(&e)), "defer");
}

#[test]
fn leaves_an_opaque_filename_for_semantic_assessment() {
    let e = Evidence {
        files: vec![ImportFile {
            name: "NqFGW2VSR2C49AkyiFgnB6G.mkv".into(),
            path: "/downloads/House.of.Cards.US.S01E01/NqFGW2VSR2C49AkyiFgnB6G.mkv".into(),
            ..file()
        }],
        ..evidence()
    };
    assert_eq!(action(&decide(&e)), "defer");
}

#[test]
fn does_not_collapse_distinct_regional_aliases() {
    let e = Evidence {
        target: Target {
            title: "The Office (US)".into(),
            alternate_titles: vec![],
            ..target()
        },
        files: vec![ImportFile {
            name: "The.Office.UK.S01E01.mkv".into(),
            path: "/downloads/House.of.Cards.US.S01E01/The.Office.UK.S01E01.mkv".into(),
            ..file()
        }],
        ..evidence()
    };
    assert_eq!(action(&decide(&e)), "defer");
}

#[test]
fn rejects_an_explicit_year_from_a_different_series_incarnation() {
    let e = Evidence {
        target: Target {
            title: "Doctor Who".into(),
            year: 2005,
            alternate_titles: vec![],
            ..target()
        },
        files: vec![ImportFile {
            name: "Doctor.Who.1963.S01E01.mkv".into(),
            path: "/downloads/House.of.Cards.US.S01E01/Doctor.Who.1963.S01E01.mkv".into(),
            ..file()
        }],
        ..evidence()
    };
    assert_eq!(action(&decide(&e)), "defer");
}

#[test]
fn defers_a_file_outside_the_download_output_path() {
    let e = Evidence {
        files: vec![ImportFile {
            path: "/tmp/injected.mkv".into(),
            ..file()
        }],
        ..evidence()
    };
    assert_eq!(action(&decide(&e)), "defer");
}

#[test]
fn defers_permission_sample_and_corruption_evidence() {
    for message in ["Permission denied", "File is a sample", "CRC corrupt"] {
        let e = Evidence {
            items: vec![QueueItem {
                status_messages: vec![messages("Import failed", &[message])],
                ..queue()
            }],
            ..evidence()
        };
        assert_eq!(action(&decide(&e)), "defer", "{message}");
    }
}

#[test]
fn removes_without_searching_when_every_intended_file_exists_and_all_rejections_are_downgrades() {
    let e = Evidence {
        target: Target {
            episodes: vec![episode(true)],
            ..target()
        },
        files: vec![ImportFile {
            rejections: vec![
                rejection(
                    "quality",
                    "Existing file on disk is of equal or higher quality",
                ),
                rejection(
                    "customFormat",
                    "Not a Custom Format upgrade for existing episode file",
                ),
            ],
            ..file()
        }],
        ..evidence()
    };
    assert!(matches!(
        decide(&e),
        Decision::Remove {
            replace: false,
            source: DecisionSource::Rules,
            ..
        }
    ));
}

fn no_files(title: &str) -> Evidence {
    Evidence {
        items: vec![QueueItem {
            status: "failed".into(),
            tracked_download_state: "failed".into(),
            status_messages: vec![messages(title, &[])],
            ..queue()
        }],
        files: vec![],
        ..evidence()
    }
}

#[test]
fn does_not_remove_a_no_files_failure_without_download_client_health_corroboration() {
    let e = no_files("No files found are eligible for import in /tmp/inter");
    assert_eq!(action(&decide(&e)), "defer");
    let corroborated = Evidence {
        download_health: Some("WARNING/HEALTH".into()),
        ..e
    };
    assert!(matches!(
        decide(&corroborated),
        Decision::Remove { replace: true, .. }
    ));
}

#[test]
fn accepts_a_corroborated_terminal_unpack_failure_for_replacement() {
    let e = no_files("No files found are eligible for import");
    let unpack = Evidence {
        download_health: Some("FAILURE/UNPACK".into()),
        ..e.clone()
    };
    assert!(matches!(
        decide(&unpack),
        Decision::Remove { replace: true, .. }
    ));
    let password = Evidence {
        download_health: Some("FAILURE/PASSWORD".into()),
        ..e
    };
    assert_eq!(action(&decide(&password)), "defer");
}

#[test]
fn cleans_a_redundant_downgrade_even_when_arr_could_not_determine_whether_it_is_a_sample() {
    let e = Evidence {
        items: vec![QueueItem {
            status_messages: vec![messages(
                "Import failed",
                &["Unable to determine if file is a sample"],
            )],
            ..queue()
        }],
        target: Target {
            episodes: vec![episode(true)],
            ..target()
        },
        files: vec![ImportFile {
            rejections: vec![
                rejection("sample", "Unable to determine if file is a sample"),
                rejection(
                    "customFormat",
                    "Not a Custom Format upgrade for existing episode file",
                ),
            ],
            ..file()
        }],
        ..evidence()
    };
    assert!(matches!(
        decide(&e),
        Decision::Remove { replace: false, .. }
    ));
}

#[test]
fn does_not_clean_a_downgrade_when_hard_infrastructure_evidence_is_also_present() {
    let e = Evidence {
        items: vec![QueueItem {
            status_messages: vec![messages("Import failed", &["Permission denied"])],
            ..queue()
        }],
        target: Target {
            episodes: vec![episode(true)],
            ..target()
        },
        files: vec![ImportFile {
            rejections: vec![rejection(
                "quality",
                "Existing file on disk is of equal or higher quality",
            )],
            ..file()
        }],
        ..evidence()
    };
    assert_eq!(action(&decide(&e)), "defer");
}

#[test]
fn resets_the_failure_fingerprint_on_size_target_or_path_changes() {
    let first = queue();
    let updates = [
        QueueItem {
            size: 2000.0,
            ..first.clone()
        },
        QueueItem {
            sizeleft: 100.0,
            ..first.clone()
        },
        QueueItem {
            episode_id: Some(999),
            ..first.clone()
        },
        QueueItem {
            output_path: Some("/downloads/new".into()),
            ..first.clone()
        },
    ];
    for update in updates {
        assert_ne!(
            observation_fingerprint(std::slice::from_ref(&first)),
            observation_fingerprint(&[update])
        );
    }
}

#[test]
fn does_not_mistake_a_longer_title_for_the_requested_series() {
    let e = Evidence {
        target: Target {
            title: "Berserk".into(),
            alternate_titles: vec![],
            ..target()
        },
        files: vec![ImportFile {
            name: "Berserk.of.Gluttony.S01E01.mkv".into(),
            ..file()
        }],
        ..evidence()
    };
    assert_eq!(action(&decide(&e)), "defer");
}

#[test]
fn requires_grabbed_episode_ids_not_just_a_matching_series() {
    let mut e = evidence();
    e.grabs[0].episode_id = Some(999);
    assert_eq!(action(&decide(&e)), "defer");
}
