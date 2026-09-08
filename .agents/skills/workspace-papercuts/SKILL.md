---
name: workspace-papercuts
description: Review, triage, and address structured papercuts reported by Omni workspace agents. Use when the user asks to inspect workspace friction, improve the workspace system from past runs, fix recurring agent/tool/data/integration/UI issues, or run workspace maintenance. Reads open reports, finds the reusable root cause, implements and verifies a general fix, and resolves reports only after the fix is shipped and confirmed.
---

# Workspace Papercuts

Use the structured ledger at `http://omni.boris/api/workspace-papercuts?status=open`. Reports are signals, not automatically correct diagnoses.

## Workflow

1. Read all open reports. Group duplicates by underlying capability, not wording. Prioritize repeated, blocking, or correctness-related issues.
2. Inspect linked run logs and the relevant workspace subject. Reproduce or establish the root cause before editing.
3. Fix the shared primitive, integration, prompt, or UI. Do not add a subject-specific branch when the capability belongs in the workspace definition or engine.
4. Add regression coverage, then run the repository's required verification and review workflow in `AGENTS.md`.
5. Ship when the repository instructions or current request authorize it. Reuse existing authorization instead of asking again. Verify the deployed behavior.
6. Resolve each fixed report with `POST /api/workspace-papercuts/{papercutId}/resolve` and JSON `{"status":"addressed","resolution":"..."}`. Use `dismissed` only for a demonstrated non-issue and explain why.

Never resolve a report merely because code was written. Keep it open if deployment or real behavior remains unverified.
