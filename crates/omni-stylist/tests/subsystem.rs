//! The subsystem as the binary wires it: tools, the backup task, and a boot
//! step that prepares the ledger without ever failing boot.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::os::unix::fs::PermissionsExt as _;

use omni_testkit::TestApp;

#[tokio::test(start_paused = true)]
async fn the_subsystem_serves_eleven_tools_and_a_daily_backup() {
    let app = TestApp::new().await;
    let mut subsystem = omni_stylist::subsystem(&app.ctx).unwrap();
    let names: Vec<&str> = subsystem
        .mcp_tools
        .iter()
        .map(|t| t.meta.name.as_str())
        .collect();
    let expected: Vec<&str> = omni_stylist::mcp::defs::TOOLS
        .iter()
        .map(|d| d.name())
        .collect();
    assert_eq!(names, expected);
    assert_eq!(names.len(), 11);
    let tasks: Vec<&str> = subsystem.tasks.iter().map(|t| t.name()).collect();
    assert_eq!(tasks, ["StylistBackup"]);
    assert!(subsystem.entities.is_empty());

    let step = subsystem.boot_steps.remove(0);
    assert_eq!(step.name, "stylist-ledger");
    (step.run)(app.ctx.clone()).await.unwrap();
    let private = &app.ctx.paths.stylist_private;
    let mode = std::fs::metadata(private).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
    for dir in ["evidence/sha256", "archives"] {
        assert!(private.join(dir).is_dir(), "{dir}");
    }
}

#[tokio::test(start_paused = true)]
async fn an_unusable_private_directory_does_not_fail_boot() {
    let app = TestApp::new().await;
    let blocked = app.ctx.paths.stylist_private.clone();
    std::fs::create_dir_all(blocked.parent().unwrap()).unwrap();
    std::fs::write(&blocked, b"a file where the directory should be").unwrap();
    let mut subsystem = omni_stylist::subsystem(&app.ctx).unwrap();
    let step = subsystem.boot_steps.remove(0);
    assert!((step.run)(app.ctx.clone()).await.is_ok());
    let stylist = omni_stylist::service(&app.ctx);
    let error = stylist.ready().await.unwrap_err();
    assert_eq!(error.code(), "io");
}
