//! `StylistBackup`: an hourly consistent archive of the `real` namespace.
//!
//! Kopia snapshots the data volume every 20 minutes, but it copies the live
//! SQLite file without a consistent snapshot. This task writes a consistent,
//! checksummed archive of the real ledger into the private archive directory
//! whenever the ledger changed since the last backup (checked hourly), so the
//! Kopia snapshots after it hold a restorable copy. Backups carry evidence
//! metadata and digests, not bytes: the evidence files themselves are
//! immutable, content-addressed files in the same private directory. Writing a
//! backup re-hashes every file, and the 04:23 run re-verifies an unchanged
//! backup the same way, so a missing or corrupt file fails the run (which is
//! logged and alerted like any failed task). It touches no fixture namespace.

use futures::future::BoxFuture;
use omni_tasks::{CronSchedule, InvalidScheduleError, RunContext, Task, TaskError, TaskOptions};

use crate::api::ArchiveSource;
use crate::archive::Reason;
use crate::error::StylistError;
use crate::service::Stylist;
use crate::types::{ArchiveId, Namespace};

pub const TASK_NAME: &str = "StylistBackup";
/// Hourly at 23 minutes past, local time.
pub const SCHEDULE: &str = "23 * * * *";
/// The local hour whose run re-verifies an unchanged backup and its evidence.
pub const REVERIFY_HOUR: i8 = 4;

const LOG: &str = "Stylist";

/// What one backup run did.
#[derive(Debug, PartialEq, Eq)]
pub enum BackupOutcome {
    /// The real namespace has no writes yet.
    Empty,
    /// The newest backup already covers this revision. When re-verified, the
    /// count of evidence files checked.
    UpToDate {
        archive_id: String,
        evidence_verified: Option<u64>,
    },
    Written {
        archive_id: String,
        ledger_revision: u64,
    },
}

impl Stylist {
    /// Writes a backup when the real ledger changed since the newest one;
    /// otherwise, with `reverify`, re-checks that backup and every evidence file.
    pub async fn backup(&self, reverify: bool) -> Result<BackupOutcome, StylistError> {
        self.ready().await?;
        let real = Namespace::real();
        let revision = self
            .store
            .read(|docs| Ok(crate::ledger::namespace_revision(docs.connection(), "real")))
            .await??;
        if revision == 0 {
            return Ok(BackupOutcome::Empty);
        }
        let archives = self.list_archives(Some(&real), 100).await?;
        let latest = archives.archives.iter().find(|m| m.reason == "backup");
        if let Some(latest) = latest
            && latest.ledger_revision == revision
        {
            if !reverify {
                return Ok(BackupOutcome::UpToDate {
                    archive_id: latest.archive_id.clone(),
                    evidence_verified: None,
                });
            }
            match self
                .verify_archive(ArchiveSource::Id(ArchiveId(latest.archive_id.clone())))
                .await
            {
                Ok((_, evidence_verified)) => {
                    return Ok(BackupOutcome::UpToDate {
                        archive_id: latest.archive_id.clone(),
                        evidence_verified: Some(evidence_verified),
                    });
                }
                // A damaged backup is replaced; the new export re-checks every
                // evidence file and fails if one of those is the problem.
                Err(error) => tracing::warn!(
                    target: LOG,
                    "Stylist backup {} failed verification ({}); writing a new one",
                    latest.archive_id,
                    error.tool_text()
                ),
            }
        }
        let exported = self.export(&real, false, Reason::Backup).await?;
        Ok(BackupOutcome::Written {
            archive_id: exported.output.manifest.archive_id,
            ledger_revision: revision,
        })
    }
}

pub struct BackupTask {
    stylist: Stylist,
    schedule: CronSchedule,
}

impl BackupTask {
    pub fn new(stylist: Stylist, tz: &jiff::tz::TimeZone) -> Result<Self, InvalidScheduleError> {
        Ok(Self {
            stylist,
            schedule: CronSchedule::parse(SCHEDULE, tz)?,
        })
    }
}

impl Task for BackupTask {
    fn name(&self) -> &str {
        TASK_NAME
    }

    fn display_name(&self) -> Option<&str> {
        Some("Stylist backup")
    }

    fn schedule(&self) -> &CronSchedule {
        &self.schedule
    }

    fn options(&self) -> TaskOptions {
        TaskOptions::default()
    }

    fn run<'a>(&'a self, _cx: &'a RunContext) -> BoxFuture<'a, Result<(), TaskError>> {
        Box::pin(async move {
            let hour = jiff::Timestamp::from_millisecond(self.stylist.clock.now_ms())
                .map(|t| t.to_zoned(self.stylist.tz.clone()).hour())
                .unwrap_or(REVERIFY_HOUR);
            match self.stylist.backup(hour == REVERIFY_HOUR).await {
                Ok(BackupOutcome::Empty) => {
                    tracing::info!(target: LOG, "No real stylist ledger yet; nothing to back up");
                    Ok(())
                }
                Ok(BackupOutcome::UpToDate {
                    archive_id,
                    evidence_verified,
                }) => {
                    match evidence_verified {
                        Some(count) => tracing::info!(
                            target: LOG,
                            "Stylist backup {archive_id} is current; re-verified it and {count} evidence files"
                        ),
                        None => {
                            tracing::info!(target: LOG, "Stylist backup {archive_id} is current")
                        }
                    }
                    Ok(())
                }
                Ok(BackupOutcome::Written {
                    archive_id,
                    ledger_revision,
                }) => {
                    tracing::info!(
                        target: LOG,
                        "Wrote stylist backup {archive_id} at ledger revision {ledger_revision}"
                    );
                    Ok(())
                }
                Err(error) => Err(TaskError::new(error.tool_text())),
            }
        })
    }
}
