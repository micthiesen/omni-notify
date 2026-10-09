//! `hygiene`: keep Cargo's build directories from silently growing until builds slow
//! down (ported from deadpan's `cargo xtask gate`).
//!
//! Cargo never deletes superseded artifacts: every feature set, profile, target and
//! source change leaves dependency objects and incremental sessions behind, and a
//! directory with millions of entries makes every rustc invocation slow. This repo
//! also keeps per-agent build directories under `target/` (`target/agents/<name>`)
//! next to the native and `wasm32-unknown-unknown` profiles, so the check measures
//! every Cargo profile directory (one holding `.fingerprint`) below `target/` and
//! below `CARGO_TARGET_DIR` when that lies elsewhere. Above the limit
//! (`OMNI_TARGET_LIMIT_GIB`, default 40) it removes the oldest artifacts with
//! `cargo sweep --maxsize` (which walks nested build directories too) until half
//! the limit remains, then
//! prunes the least recently changed incremental directories, which `cargo sweep`
//! leaves alone; rustc rebuilds missing incremental state. Other files under
//! `target/` are never measured or touched. Disk usage alone never fails the gate.

use std::{
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

use anyhow::{Context, Result};

/// Above this many GiB of Cargo artifacts, the check sweeps stale ones.
const DEFAULT_LIMIT_GIB: u64 = 40;
const LIMIT_VARIABLE: &str = "OMNI_TARGET_LIMIT_GIB";
const GIB: u64 = 1 << 30;
/// `target/agents/<name>/<triple>/<profile>` is the deepest layout in use.
const MAX_DEPTH: usize = 5;

/// Measures Cargo artifacts and sweeps stale ones when they exceed the limit.
pub fn maintain(workspace: &Path) -> Result<()> {
    let workspace = &workspace
        .canonicalize()
        .with_context(|| format!("resolving {}", workspace.display()))?;
    let limit = limit_gib(std::env::var(LIMIT_VARIABLE).ok().as_deref());
    let roots = search_roots(workspace);
    let before = artifact_bytes(&roots)?;
    if before <= limit * GIB {
        println!(
            "hygiene: {} of Cargo artifacts (limit {limit} GiB)",
            gib(before)
        );
        return Ok(());
    }
    let goal = limit / 2 * GIB;
    println!(
        "hygiene: {} of Cargo artifacts exceeds {limit} GiB; removing the oldest down to {} GiB",
        gib(before),
        limit / 2
    );
    // `cargo sweep` walks a whole build directory, nested agent directories
    // included, so each search root is swept once with its share of the goal.
    for directory in &roots {
        let bytes = artifact_bytes(std::slice::from_ref(directory))?;
        if bytes == 0 {
            continue;
        }
        let share = proportional_share(bytes, before, goal);
        let swept = Command::new(cargo())
            .args(["sweep", "--maxsize", &format!("{}MB", share / (1 << 20))])
            .env("CARGO_TARGET_DIR", directory)
            .current_dir(workspace)
            .status();
        if !matches!(swept, Ok(status) if status.success()) {
            println!(
                "hygiene: `cargo sweep` is unavailable (brew install cargo-sweep) or failed for {}; pruning incremental state only",
                directory.display()
            );
        }
    }
    prune_incremental(&roots, goal)?;
    let after = artifact_bytes(&roots)?;
    println!("hygiene: {} -> {}", gib(before), gib(after));
    if after > limit * GIB {
        println!("hygiene: still above {limit} GiB; run `cargo clean` when convenient");
    }
    Ok(())
}

fn cargo() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

/// The configured limit; an unset or unparsable value keeps the default.
fn limit_gib(value: Option<&str>) -> u64 {
    value
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|limit| *limit > 0)
        .unwrap_or(DEFAULT_LIMIT_GIB)
}

/// `target/` plus `CARGO_TARGET_DIR` when it is not already inside it.
fn search_roots(workspace: &Path) -> Vec<PathBuf> {
    let target = workspace.join("target");
    let mut roots = vec![target.clone()];
    if let Some(custom) = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from) {
        let custom = if custom.is_absolute() {
            custom
        } else {
            workspace.join(custom)
        };
        let inside = match (custom.canonicalize(), target.canonicalize()) {
            (Ok(custom), Ok(target)) => custom.starts_with(target),
            _ => custom.starts_with(&target),
        };
        if !inside {
            roots.push(custom);
        }
    }
    roots
}

/// The part of `goal` a build directory holding `bytes` of `total` may keep.
fn proportional_share(bytes: u64, total: u64, goal: u64) -> u64 {
    if total == 0 {
        return goal;
    }
    u64::try_from(u128::from(bytes) * u128::from(goal) / u128::from(total)).unwrap_or(goal)
}

/// Removes the least recently changed per-crate incremental directories until the
/// artifacts fit `goal` bytes.
fn prune_incremental(roots: &[PathBuf], goal: u64) -> Result<()> {
    let mut total = artifact_bytes(roots)?;
    let mut sessions = Vec::new();
    for profile in profile_directories(roots)? {
        let Ok(entries) = std::fs::read_dir(profile.join("incremental")) else {
            continue;
        };
        for entry in entries.flatten() {
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            sessions.push((modified, entry.path()));
        }
    }
    sessions.sort();
    for (_, path) in sessions {
        if total <= goal {
            break;
        }
        let bytes = tree_bytes(&path)?;
        std::fs::remove_dir_all(&path).with_context(|| format!("removing {}", path.display()))?;
        total = total.saturating_sub(bytes);
    }
    Ok(())
}

/// Bytes in every Cargo profile directory below `roots`.
fn artifact_bytes(roots: &[PathBuf]) -> Result<u64> {
    let mut total = 0;
    for profile in profile_directories(roots)? {
        total += tree_bytes(&profile)?;
    }
    Ok(total)
}

/// Directories holding `.fingerprint` below `roots`, without descending into them.
fn profile_directories(roots: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut profiles = Vec::new();
    let mut pending: Vec<(PathBuf, usize)> = roots.iter().map(|root| (root.clone(), 0)).collect();
    while let Some((directory, depth)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries {
            let entry = entry.with_context(|| format!("reading {}", directory.display()))?;
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let path = entry.path();
            if path.join(".fingerprint").is_dir() {
                profiles.push(path);
            } else if depth + 1 < MAX_DEPTH {
                pending.push((path, depth + 1));
            }
        }
    }
    profiles.sort();
    profiles.dedup();
    Ok(profiles)
}

fn tree_bytes(root: &Path) -> Result<u64> {
    let mut total = 0;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let read = || format!("reading {}", directory.display());
        for entry in std::fs::read_dir(&directory).with_context(read)? {
            let entry = entry.with_context(read)?;
            let kind = entry.file_type().with_context(read)?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                total += entry.metadata().with_context(read)?.len();
            }
        }
    }
    Ok(total)
}

fn gib(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let value = bytes as f64 / GIB as f64;
    format!("{value:.1} GiB")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn write(path: &Path, bytes: usize) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, vec![0; bytes])
    }

    #[test]
    fn only_cargo_profile_directories_are_measured() -> Result<()> {
        let scratch = tempfile::tempdir()?;
        let target = scratch.path().join("target");
        std::fs::create_dir_all(target.join("debug/.fingerprint"))?;
        write(&target.join("debug/deps/libcore.rlib"), 1000)?;
        std::fs::create_dir_all(target.join("wasm32-unknown-unknown/debug/.fingerprint"))?;
        write(&target.join("wasm32-unknown-unknown/debug/app.wasm"), 500)?;
        // Per-agent build directories nest a whole target directory.
        let agent = target.join("agents/build");
        std::fs::create_dir_all(agent.join("debug/.fingerprint"))?;
        write(&agent.join("debug/deps/libx.rlib"), 200)?;
        std::fs::create_dir_all(agent.join("wasm32-unknown-unknown/debug/.fingerprint"))?;
        write(&agent.join("wasm32-unknown-unknown/debug/y.wasm"), 100)?;
        // Evidence from other tools lives beside the profiles.
        write(&target.join("evidence/run/capture.png"), 4000)?;

        assert_eq!(artifact_bytes(std::slice::from_ref(&target))?, 1800);
        assert_eq!(artifact_bytes(&[scratch.path().join("missing")])?, 0);
        Ok(())
    }

    #[test]
    fn the_oldest_incremental_state_is_pruned_first() -> Result<()> {
        let scratch = tempfile::tempdir()?;
        let target = scratch.path().join("target");
        let incremental = target.join("debug/incremental");
        std::fs::create_dir_all(target.join("debug/.fingerprint"))?;
        let now = SystemTime::now();
        for (name, age) in [("old-1", 60), ("new-2", 0)] {
            let session = incremental.join(name);
            write(&session.join("query-cache.bin"), 1000)?;
            std::fs::File::open(&session)?.set_modified(now - Duration::from_secs(age))?;
        }

        prune_incremental(std::slice::from_ref(&target), 1500)?;
        assert!(!incremental.join("old-1").exists());
        assert!(incremental.join("new-2").exists());
        assert_eq!(artifact_bytes(&[target])?, 1000);
        Ok(())
    }

    #[test]
    fn the_limit_defaults_to_forty_gib() {
        assert_eq!(limit_gib(None), 40);
        assert_eq!(limit_gib(Some("12")), 12);
        assert_eq!(limit_gib(Some(" 60 ")), 60);
        assert_eq!(limit_gib(Some("0")), 40);
        assert_eq!(limit_gib(Some("lots")), 40);
    }

    #[test]
    fn each_build_directory_keeps_its_share_of_the_goal() {
        assert_eq!(proportional_share(30, 40, 20), 15);
        assert_eq!(proportional_share(10, 40, 20), 5);
        assert_eq!(proportional_share(0, 0, 20), 20);
        assert_eq!(proportional_share(u64::MAX, u64::MAX, 20 * GIB), 20 * GIB);
    }
}
