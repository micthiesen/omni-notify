//! Response decoding: the frontend decodes every response
//! into the shared `omni-api` DTOs, so malformed nested data must fail to
//! decode and valid shapes must round-trip.

use omni_api::intelligence::IntelligenceDetailsResponse;
use omni_api::podcasts::PodcastRecommendation;
use omni_api::presspods::PressPodsEpisodeDetail;
use omni_api::workspaces::WorkspaceSubjectResponse;
use omni_web_kit::api::Snapshot;
use serde_json::{Value, json};

fn rejects<T: serde::de::DeserializeOwned>(value: Value) {
    assert!(serde_json::from_value::<T>(value).is_err());
}

#[test]
fn rejects_malformed_nested_task_data_in_a_snapshot() {
    rejects::<Snapshot>(json!({
        "tasks": [{
            "name": "LiveCheck",
            "schedule": "* * * * *",
            "running": false,
            "nextRuns": [123],
            "lastRun": null
        }],
        "streamers": [],
        "runs": [],
        "onDeck": []
    }));
}

#[test]
fn rejects_malformed_nested_podcast_shortlist_scores() {
    rejects::<PodcastRecommendation>(json!({
        "recommendationId": "rec_1",
        "showTitle": "A Show",
        "episodeTitle": "An Episode",
        "feedUrl": "https://example.com/feed.xml",
        "publishedAt": 1,
        "status": "notified",
        "shortlistScores": {"tasteMatch": 8, "novelty": 7, "composite": 7.5, "risks": [false]},
        "recommendedAt": 2
    }));
}

#[test]
fn accepts_null_and_populated_podcast_recommendation_fields() {
    let recommendation = json!({
        "recommendationId": "rec_1",
        "showTitle": "A Show",
        "episodeTitle": "An Episode",
        "feedUrl": "https://example.com/feed.xml",
        "itunesId": null,
        "artworkUrl": null,
        "episodeUrl": null,
        "publishedAt": 1,
        "durationMinutes": null,
        "status": "notified",
        "whyForUser": null,
        "caveats": [],
        "confidence": null,
        "shortlistScores": null,
        "discoveredVia": null,
        "sourceUrl": null,
        "matchedVoices": [],
        "recommendedAt": 2,
        "notifiedAt": null,
        "queueResult": null,
        "feedback": null,
        "feedbackAt": null,
        "feedbackNote": null
    });
    let mut populated = recommendation.clone();
    let fields = json!({
        "itunesId": 123,
        "artworkUrl": "https://example.com/artwork.jpg",
        "episodeUrl": "https://example.com/episode",
        "durationMinutes": 45,
        "whyForUser": "A strong match",
        "caveats": ["Part two"],
        "confidence": 0.9,
        "shortlistScores": {
            "tasteMatch": 8,
            "novelty": 7,
            "composite": 7.5,
            "risks": ["Requires context"]
        },
        "discoveredVia": "guest search",
        "sourceUrl": "https://example.com/source",
        "matchedVoices": ["A Guest"],
        "notifiedAt": 3,
        "queueResult": "queued",
        "feedback": "good_pick",
        "feedbackAt": 4,
        "feedbackNote": "More like this"
    });
    if let (Some(target), Some(source)) = (populated.as_object_mut(), fields.as_object()) {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
    }
    for value in [recommendation, populated] {
        let decoded: PodcastRecommendation = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), value);
    }
}

#[test]
fn rejects_malformed_nested_press_pods_cost_accounting() {
    rejects::<PressPodsEpisodeDetail>(json!({
        "episodeId": "episode_1",
        "title": "Article",
        "author": null,
        "publication": null,
        "domain": null,
        "articleUrl": "https://example.com/article",
        "leadImageUrl": null,
        "excerpt": null,
        "voiceName": null,
        "synthesizedSeconds": null,
        "audioUrl": "/pods/episode_1.mp3",
        "durationSeconds": null,
        "fileBytes": 100,
        "retrieverName": null,
        "retrieverSeconds": null,
        "retrieverAttempts": null,
        "chapters": null,
        "costCents": null,
        "createdAt": 1,
        "publishedAt": null,
        "runId": null,
        "content": "Article body",
        "authorGender": null,
        "voiceProvider": null,
        "chunks": null,
        "costs": {
            "llmCents": 1,
            "ttsCents": 2,
            "detailCents": {"metadata": 1},
            "detailTokens": {"metadata": {"input": 10, "output": "invalid"}},
            "detailChars": {"speech": 100}
        }
    }));
}

#[test]
fn rejects_malformed_nested_workspace_subject_state() {
    rejects::<WorkspaceSubjectResponse>(json!({
        "workspace": {
            "id": "research",
            "title": "Research",
            "description": "Research workspace",
            "subjectLabel": "Subject",
            "subjectLabelPlural": "Subjects",
            "taskName": "WorkspaceResearch",
            "schedule": "0 12 * * *",
            "instructions": "Research it",
            "artifacts": []
        },
        "subject": {
            "workspaceId": "research",
            "subjectId": "subject_1",
            "title": "A subject",
            "status": "deleted",
            "summary": "Summary",
            "createdAt": 1,
            "updatedAt": 2
        },
        "artifacts": [],
        "artifactRevisions": [],
        "messages": [],
        "sources": [],
        "actions": [],
        "emailScope": null,
        "papercuts": []
    }));
}

#[test]
fn rejects_malformed_nested_livestream_runtime_queue_data() {
    rejects::<IntelligenceDetailsResponse>(json!({
        "intelligence": null,
        "diagnostics": null,
        "events": [],
        "runtime": {
            "enabled": true,
            "voiceprintLoaded": true,
            "model": "model",
            "queues": {
                "capture": {"running": "one", "queued": 0},
                "speech": {"running": 0, "queued": 0},
                "llm": {"running": 0, "queued": 0}
            },
            "activeStreamCount": 1,
            "activeVoiceTargetCount": 1,
            "budget": {"spentCents": 1, "limitCents": 10, "remainingCents": 9},
            "intervals": {"voiceSeconds": 60, "summarySeconds": 300}
        },
        "generatedAt": 1
    }));
}
