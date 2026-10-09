//! Subsystem wiring: tasks register only with complete configuration, with
//! their names, schedules and startup flags.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use omni_config::Config;
use omni_runtime::AppContext;

fn with_env(ctx: &AppContext, extra: &[(&str, &str)]) -> AppContext {
    let mut env = omni_testkit::test_app_env();
    for (key, value) in extra {
        env.insert((*key).to_owned(), (*value).to_owned());
    }
    let mut ctx = ctx.clone();
    ctx.config = Arc::new(Config::from_env(&env).unwrap());
    ctx
}

#[tokio::test]
async fn registers_no_tasks_without_service_configuration() {
    let app = omni_testkit::TestApp::new().await;
    let subsystem = omni_arr::subsystem(&with_env(&app.ctx, &[("OPENAI_API_KEY", "sk-test")]));
    assert!(subsystem.tasks.is_empty());
    assert_eq!(subsystem.entities.len(), 2);
}

#[tokio::test]
async fn registers_both_tasks_with_their_schedules() {
    let app = omni_testkit::TestApp::new().await;
    let ctx = with_env(
        &app.ctx,
        &[
            ("OPENAI_API_KEY", "sk-test"),
            ("SONARR_URL", "http://127.0.0.1:1"),
            ("SONARR_API_KEY", "key"),
            ("OBSERVER_URL", "http://127.0.0.1:2"),
            ("OBSERVER_API_KEY", "key"),
        ],
    );
    let subsystem = omni_arr::subsystem(&ctx);
    let tasks: Vec<(String, String, bool)> = subsystem
        .tasks
        .iter()
        .map(|t| {
            (
                t.name().to_owned(),
                t.schedule().as_str().to_owned(),
                t.options().run_on_startup,
            )
        })
        .collect();
    assert_eq!(
        tasks,
        vec![
            ("ArrRecovery".to_owned(), "0 */5 * * * *".to_owned(), true),
            (
                "ObserverRepair".to_owned(),
                "0 */15 * * * *".to_owned(),
                true
            ),
        ]
    );
}

#[tokio::test]
async fn disables_tasks_without_openai_or_when_switched_off() {
    let app = omni_testkit::TestApp::new().await;
    let no_openai = with_env(
        &app.ctx,
        &[
            ("SONARR_URL", "http://127.0.0.1:1"),
            ("SONARR_API_KEY", "key"),
        ],
    );
    assert!(omni_arr::subsystem(&no_openai).tasks.is_empty());
    let disabled = with_env(
        &app.ctx,
        &[
            ("OPENAI_API_KEY", "sk-test"),
            ("SONARR_URL", "http://127.0.0.1:1"),
            ("SONARR_API_KEY", "key"),
            ("ARR_RECOVERY_ENABLED", "false"),
        ],
    );
    assert!(omni_arr::subsystem(&disabled).tasks.is_empty());
}
