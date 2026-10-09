//! Wire shapes of the WP00 DTOs against the TS serializers (`serializeRun`,
//! `TaskRegistry.list`, the run-log routes). Golden fixtures from production
//! (`xtask capture-golden`) replace these hand-written bodies once captured.

use omni_api::runs::{
    LogLevel, Run, RunLogLine, RunLogStreamFrame, RunLogsResponse, RunStatus, RunTrigger,
};
use omni_api::tasks::{TaskInfo, TasksResponse};

fn run() -> Run {
    Run {
        run_id: "LiveCheckTask:0f8fad5b-d9cb-469f-a165-70867728950e".to_owned(),
        task_name: "LiveCheckTask".to_owned(),
        trigger: RunTrigger::Schedule,
        scheduled_for: None,
        started_at: 1_760_000_000_000,
        finished_at: Some(1_760_000_001_000),
        status: RunStatus::Success,
        error: None,
        summary: None,
    }
}

#[test]
fn run_serializes_every_optional_field_as_null_in_ts_key_order() {
    assert_eq!(
        serde_json::to_string(&run()).unwrap(),
        r#"{"runId":"LiveCheckTask:0f8fad5b-d9cb-469f-a165-70867728950e","taskName":"LiveCheckTask","trigger":"schedule","scheduledFor":null,"startedAt":1760000000000,"finishedAt":1760000001000,"status":"success","error":null,"summary":null}"#
    );
}

#[test]
fn task_info_omits_an_absent_display_name() {
    let body = TasksResponse {
        tasks: vec![TaskInfo {
            name: "PressPods".to_owned(),
            display_name: None,
            schedule: "0 */5 * * * *".to_owned(),
            running: false,
            next_runs: vec!["2025-10-09T08:05:00.000Z".to_owned()],
            last_run: None,
        }],
    };
    let json = serde_json::to_string(&body).unwrap();
    assert_eq!(
        json,
        r#"{"tasks":[{"name":"PressPods","schedule":"0 */5 * * * *","running":false,"nextRuns":["2025-10-09T08:05:00.000Z"],"lastRun":null}]}"#
    );
    assert_eq!(serde_json::from_str::<TasksResponse>(&json).unwrap(), body);
}

#[test]
fn run_log_frames_decode_by_event_name() {
    let line = RunLogLine {
        t: 1,
        level: LogLevel::Warn,
        logger: "Main:LiveCheck".to_owned(),
        msg: "m".to_owned(),
    };
    let init = RunLogsResponse {
        run: run(),
        lines: vec![line.clone()],
        dropped: 0,
    };
    let data = serde_json::to_string(&init).unwrap();
    assert!(data.ends_with(
        r#""lines":[{"t":1,"level":"warn","logger":"Main:LiveCheck","msg":"m"}],"dropped":0}"#
    ));
    assert_eq!(
        RunLogStreamFrame::decode("init", &data).unwrap().unwrap(),
        RunLogStreamFrame::Init(init)
    );
    let frame = RunLogStreamFrame::decode("line", &serde_json::to_string(&line).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(frame.event_name(), "line");
    assert!(RunLogStreamFrame::decode("ping", "1").is_none());
}
