#!/bin/sh
# Slate validation gate: host firmware, ESP32-H2 build, PCB tooling.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

cd "$root/firmware"
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked

cd "$root/firmware/app"
cargo fmt --check
cargo clippy --locked --release -- -D warnings
cargo build --locked --release

cd "$root/pcb"
bun install --frozen-lockfile
bun run check
bun run typecheck
bun run check:mainboard
bun run test
