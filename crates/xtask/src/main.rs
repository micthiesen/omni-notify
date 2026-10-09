//! `cargo xtask <command>`: repository tooling (see docs/architecture.md).
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};

mod api_diff;
mod capture;
mod compat;
mod deps;
mod golden_synth;
mod mcp;
mod target_hygiene;

const COMMANDS: &[(&str, &str)] = &[
    (
        "capture-golden",
        "--base URL [--mcp-token-env NAME] [--route PATH]...: capture read-only HTTP GET fixtures and the live MCP tools/list into the gitignored .local/golden-capture/",
    ),
    (
        "golden-synthesize",
        "[--from DIR] [--check]: derive the committed synthetic HTTP fixtures (no personal data) from the raw capture",
    ),
    (
        "mcp-policy",
        "[--check]: render docs/mcp-policy.json and its golden copy from the committed MCP tool metadata",
    ),
    (
        "golden-check",
        "verify mcp-policy and that committed golden fixtures are unchanged",
    ),
    (
        "compat-audit",
        "--db COPY: per-entity counts, CBOR decode failures, legacy rows and Rust re-encode parity",
    ),
    (
        "api-diff",
        "--ts URL --rust URL [--route PATH]... [--shape]: diff JSON values (or shapes) of read routes",
    ),
    ("deps-check", "enforce the crate dependency direction rules"),
    (
        "hygiene",
        "measure Cargo build directories under target/ and sweep the oldest artifacts above OMNI_TARGET_LIMIT_GIB (default 40)",
    ),
    (
        "gate",
        "local pre-push gate: hygiene, fmt --check, workspace and wasm32 clippy -D warnings, deps-check, workspace tests --no-fail-fast",
    ),
];

/// The web crates trunk builds for `wasm32-unknown-unknown`; workspace Clippy only
/// checks them for the host target.
const WASM_CRATES: &[&str] = &["omni-web", "omni-web-kit", "omni-web-pages"];

/// Runs `cargo <args>` from the repository root, failing on a non-zero exit.
fn cargo(args: &[&str]) -> Result<()> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    println!("gate: cargo {}", args.join(" "));
    let status = std::process::Command::new(cargo)
        .args(args)
        .current_dir(repo_root())
        .status()
        .context("starting cargo")?;
    if !status.success() {
        bail!("cargo {} exited with {status}", args.join(" "));
    }
    Ok(())
}

/// Build-directory hygiene, then the checks CI runs, plus wasm32 Clippy for the
/// web crates. Not run in CI: hosted runners start from a restored cache.
fn gate() -> Result<()> {
    target_hygiene::maintain(&repo_root())?;
    cargo(&["fmt", "--all", "--check"])?;
    cargo(&[
        "clippy",
        "--workspace",
        "--all-targets",
        "--locked",
        "--",
        "-D",
        "warnings",
    ])?;
    let mut wasm = vec!["clippy"];
    for name in WASM_CRATES {
        wasm.extend(["-p", name]);
    }
    wasm.extend([
        "--target",
        "wasm32-unknown-unknown",
        "--all-targets",
        "--locked",
        "--",
        "-D",
        "warnings",
    ]);
    cargo(&wasm)?;
    deps::deps_check()?;
    cargo(&["test", "--workspace", "--locked", "--no-fail-fast"])?;
    println!("gate: ok");
    Ok(())
}

/// The repository root (two levels above this crate).
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Values of every `--name VALUE` occurrence.
pub fn flag_values<'a>(args: &'a [String], name: &str) -> Vec<&'a str> {
    args.windows(2)
        .filter(|pair| pair[0] == name)
        .map(|pair| pair[1].as_str())
        .collect()
}

pub fn flag_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    flag_values(args, name).into_iter().next()
}

fn usage() {
    eprintln!("usage: cargo xtask <command> [args]\n\ncommands:");
    for (name, help) in COMMANDS {
        eprintln!("  {name:<16} {help}");
    }
}

fn run(command: &str, args: &[String]) -> Result<()> {
    let check = args.iter().any(|a| a == "--check");
    match command {
        "capture-golden" => capture::capture_golden(args),
        "golden-synthesize" => golden_synth::golden_synthesize(args),
        "mcp-policy" => mcp::mcp_policy(check),
        "golden-check" => {
            mcp::mcp_policy(true)?;
            capture::committed_fixtures_unchanged()?;
            println!("golden-check: ok");
            Ok(())
        }
        "compat-audit" => compat::compat_audit(args),
        "api-diff" => api_diff::api_diff(args),
        "deps-check" => deps::deps_check(),
        "hygiene" => target_hygiene::maintain(&repo_root()),
        "gate" => gate(),
        other => bail!("unknown command {other:?}"),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((command, rest)) = args.split_first() else {
        usage();
        return ExitCode::FAILURE;
    };
    if command == "help" || command == "--help" || command == "-h" {
        usage();
        return ExitCode::SUCCESS;
    }
    match run(command, rest) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
