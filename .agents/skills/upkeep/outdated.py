#!/usr/bin/env python3
"""List exact-pinned workspace dependencies with newer eligible crates.io releases.

Usage: python3 .agents/skills/upkeep/outdated.py [--min-age-days N] [CARGO_TOML]

Reads `[workspace.dependencies]` from the root Cargo.toml (default: the one two
directories above this skill), skips path dependencies, and asks crates.io for
each crate's versions. A version is eligible when it is not yanked, not a
pre-release, and was published at least N days ago (default 14). Prints one line
per crate that has a newer eligible version, marking semver-incompatible bumps.
Requests are spaced one second apart, per the crates.io crawler policy.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import pathlib
import sys
import time
import tomllib
import urllib.request

USER_AGENT = "OpenAI File Downloader, XaiImageApiFetch/1.0"


def parse_version(text: str) -> tuple[int, ...] | None:
    core = text.split("+", 1)[0]
    if "-" in core:
        return None
    try:
        return tuple(int(part) for part in core.split("."))
    except ValueError:
        return None


def compatible(current: tuple[int, ...], candidate: tuple[int, ...]) -> bool:
    """Cargo's caret compatibility: same leftmost non-zero component."""
    for index, value in enumerate(current):
        if value != 0 or index == len(current) - 1:
            return candidate[: index + 1] == current[: index + 1]
    return False


def pinned(spec: object) -> str | None:
    version = spec.get("version") if isinstance(spec, dict) else spec
    if isinstance(spec, dict) and "path" in spec:
        return None
    if isinstance(version, str) and version.startswith("="):
        return version[1:]
    return None


def fetch_versions(name: str) -> list[dict]:
    request = urllib.request.Request(
        f"https://crates.io/api/v1/crates/{name}/versions",
        headers={"User-Agent": USER_AGENT},
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)["versions"]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--min-age-days", type=int, default=14)
    parser.add_argument(
        "cargo_toml",
        nargs="?",
        default=pathlib.Path(__file__).resolve().parents[3] / "Cargo.toml",
        type=pathlib.Path,
    )
    args = parser.parse_args()

    manifest = tomllib.loads(args.cargo_toml.read_text())
    dependencies = manifest.get("workspace", {}).get("dependencies", {})
    cutoff = dt.datetime.now(dt.timezone.utc) - dt.timedelta(days=args.min_age_days)

    unpinned = []
    for name, spec in sorted(dependencies.items()):
        if isinstance(spec, dict) and "path" in spec:
            continue
        current_text = pinned(spec)
        if current_text is None:
            unpinned.append(name)
            continue
        current = parse_version(current_text)
        crate = spec.get("package", name) if isinstance(spec, dict) else name
        try:
            versions = fetch_versions(crate)
        except Exception as error:  # report and continue with the next crate
            print(f"{name}: lookup failed: {error}", file=sys.stderr)
            continue
        finally:
            time.sleep(1)
        eligible = []
        for entry in versions:
            parsed = parse_version(entry["num"])
            published = dt.datetime.fromisoformat(entry["created_at"])
            if parsed and not entry["yanked"] and published <= cutoff:
                eligible.append((parsed, entry["num"], published.date()))
        if not eligible or current is None:
            continue
        newest = max(eligible)
        if newest[0] <= current:
            continue
        kind = "compatible" if compatible(current, newest[0]) else "BREAKING"
        print(f"{name}: {current_text} -> {newest[1]} ({kind}, published {newest[2]})")

    for name in unpinned:
        print(f"{name}: not an exact `=` pin", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
