---
name: upkeep
description: Full maintenance pass on this repo - upgrade Node/pnpm, all dependencies (including majors, researched via changelogs), GitHub Actions, Dockerfile, and LLM model IDs, then verify everything and auto commit/push. Use when the user invokes /upkeep or asks to update/upgrade dependencies, tooling, or "outdated stuff".
---

# Upkeep: full maintenance pass

Upgrade everything in this repo that has drifted, verify it all works, then commit and push to main (CI builds and deploys the Docker image). Be thorough: majors are in scope, but every major gets changelog research before its version is bumped.

## 0. Sync with remote FIRST

`git fetch origin && git status`. If local main is behind, pull (rebase) **before** surveying anything. Upgrading against a stale base wastes the entire pass — the remote may already contain feature work or its own dependency bumps.

## 1. Survey (parallelize all of this)

- `pnpm outdated -r` (covers the `frontend` workspace too)
- Node: current pin in `.node-version` / `engines` / Dockerfile `FROM` vs latest **Active LTS** (`curl -s https://raw.githubusercontent.com/nodejs/Release/main/schedule.json`; pick the latest version whose `lts` date has passed, then latest patch from `https://nodejs.org/dist/index.json`). Don't jump to a Current (non-LTS) major.
- pnpm: `npm view pnpm version` vs `packageManager` field
- GitHub Actions: for each `uses:` in `.github/workflows/*.yml`, `gh api repos/<owner>/<repo>/releases/latest --jq .tag_name`
- LLM model IDs: defaults in `src/ai/registry.ts`, plus examples in `README.md` and `.env.example`

## 2. Research majors before bumping

For each **major** bump (and for pnpm/Node majors), spawn parallel research agents to read the official migration guide/changelog and report only the breaking changes that hit *this codebase's actual usage* (have the agent grep usage sites first). Minors/patches need no research — `pnpm update -r` handles in-range ones.

Model IDs: pick the newest **generally-available** model from the *same provider and tier* (flash-class stays flash-class). Ground truth for what's valid: the typed model-ID unions in the installed provider package d.ts files (e.g. `grep -oE "'gemini-[a-z0-9.-]+'" node_modules/@ai-sdk/google/dist/index.d.ts`). Preview-model IDs get replaced by their GA successor.

## 3. Apply

- Manifests: root `package.json`, `frontend/package.json`. Keep `@types/node` major **matched to the Node runtime major**, not latest.
- Version pins travel together: `.node-version`, `engines`, `packageManager`, Dockerfile `FROM` + pnpm install line, CI (`node-version-file` already covers Node there).
- pnpm settings live in `pnpm-workspace.yaml`, not `package.json#pnpm` (pnpm 11+): `allowBuilds`, `peerDependencyRules`, `patchedDependencies`, `overrides`.
- Patched deps: if a bump changes a patched package's resolved version, check whether upstream fixed the issue; if not, re-key the patch (rename `patches/<name>@<old>.patch` → `@<new>` and update `patchedDependencies`). The `jmap-rfc-types` patch (broken `.ts` extension imports) has survived multiple upstream versions — verify before dropping it.
- Install: `CI=true pnpm install --no-frozen-lockfile`, then `CI=true pnpm update -r` for in-range minors. (`CI=true` avoids the no-TTY modules-purge abort; `--no-frozen-lockfile` because `CI=true` implies frozen.)
- If Node changed: `fnm install <version>` locally and run all commands via `fnm exec --using=<version>` (this machine uses fnm; pnpm comes from corepack, so also set `COREPACK_ENABLE_DOWNLOAD_PROMPT=0`).
- Fix code breakage from majors. When a migration guide claims a rename, confirm against the installed d.ts before editing — guides sometimes describe aliases that still exist.
- Frontend major migrations (vite/recharts/react) can go to a subagent working only in `frontend/` while backend fixes proceed in parallel — but it must NOT run pnpm install (single shared workspace install).

## 4. Verify (all must pass)

```
pnpm run typecheck
CI=true pnpm test
pnpm run build
pnpm --filter frontend run build
pnpm run check          # biome; run `biome migrate --write` if it flags schema version
```

Then a runtime smoke test of the real entrypoint (catches ESM/runtime breaks tests miss):

```
LOG_LEVEL=info DB_NAME=<scratch>/smoke.db TWITCH_CHANNEL_NAMES=testuser FRONTEND_PORT=3199 \
  timeout --signal=SIGTERM 6 node dist/index.js
```

Expect: config logged, tasks registered, graceful shutdown on SIGTERM. The Docker image cannot be verified locally (no container runtime on this box) — CI covers it.

## 5. Ship

1. `git pull --rebase` (again — remote may have moved during the pass)
2. Commit everything with a message summarizing tool/dep/model changes (what and why, not a file list)
3. `git push`
4. Watch CI to completion: `gh run watch <id> --exit-status` (background). The `build-and-publish` job is the only verification of the Dockerfile — do not declare success until it's green. If CI fails, fix and push again.
