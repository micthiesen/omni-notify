# Claude Code sessions on the Mac

Omni's `claude_*` MCP tools control Claude Code sessions on Michael's MacBook
through `claude-for-dot`, the JSON session client in the dotfiles repository.
Any MCP client with `OMNI_MCP_TOKEN`, including Executor, can use them.

## Link

The Mac connects to Omni; Omni never connects to the laptop. The dotfiles
`omni-link` LaunchAgent long-polls `POST /device-link/poll` on
`https://omni.syas.ca` and returns each job's output to `POST /device-link/result`.
This works on the home LAN and over the WireGuard VPN, and needs no inbound port,
SSH, or stable Mac address.

Both endpoints accept only `Authorization: Bearer <OMNI_DEVICE_LINK_TOKEN>`. The
device token must be strong and different from `OMNI_MCP_TOKEN`; it cannot call
`/mcp`, and the MCP token cannot poll for jobs. On the Mac the token is stored in
the login Keychain (`omni-link set-token`).

Omni holds each poll for up to 25 seconds. The Mac counts as online while a poll
is open or one ended in the last 45 seconds. Tool calls fail immediately with:

- `offline`: the Mac is asleep, away without VPN, or the agent is stopped.
- `disabled`: the Mac's kill switch is on (`omni-link disable`).
- `not_picked_up`: no poll took the job within 30 seconds. The job is withdrawn
  and can no longer run.
- `outcome_unknown`: the Mac took the job but no result arrived in time. Omni
  never retries; read the session before repeating a start or send.

Omni waits 15 seconds past each command's own timeout so the Mac can report
its timeout first. A newer poll releases an older held one, so a connection
left behind by an agent restart cannot take jobs. If an MCP caller disconnects
after the Mac takes a job, the job still runs and its result is discarded.
Job state is in memory. An Omni restart fails in-flight calls with
`outcome_unknown` or `offline`; it does not replay them.

## Projects and permissions

New sessions start only in the projects the Mac's `claude-rc` Remote Control
supervisor uses: `claude-rc/config.toml` in dotfiles plus the Mac's
`~/.config/claude-rc/local.toml`, enabled entries only. `claude_projects_list`
returns that list and `claude_session_start` takes a project name. All other
tools work on every session on the Mac, whatever its directory or origin.

Sessions run with the same full access as `claude-for-dot`
(`--dangerously-skip-permissions`). `start`, `send`, and `stop` therefore have
the Executor policy `require_approval`; listing, reading, and waiting are
`allow`. Starts require an `idempotencyKey`, so a retried start returns the
existing session. Sends are not idempotent.

## Tools

| Tool | Purpose |
| --- | --- |
| `claude_link_status` | Omni's view of the link; works while the Mac is offline |
| `claude_projects_list` | Projects where sessions may start |
| `claude_sessions_list` | Sessions, newest first, optionally by project |
| `claude_session_get` | Status, revision, last assistant text |
| `claude_session_read` | Transcript pages; item text is capped at 8,000 characters |
| `claude_session_result` | Assistant text since the last input, capped at 20,000 |
| `claude_session_wait` | Wait up to 45 seconds; pass `afterRevision` |
| `claude_session_start` | Start a background session in a project |
| `claude_session_send` | Continue an idle background session |
| `claude_session_stop` | Stop a background session and keep its conversation |

A typical flow is `start`, then `wait` with the returned revision until
`timedOut` is false, then `result`. Interactive terminal sessions can be read but
not sent input; reach those through Remote Control.

Omni logs each forwarded command with its session or project, duration, and
outcome code, never prompt text. The `/claude` page shows the link state, the
Mac's live sessions with transcripts, and every `claude_*` call grouped by
session, including prompts and results (bounded, see [MCP activity](mcp.md#activity)).
Its live panels query the Mac read-only through the same link.

Sessions started through `claude-for-dot` are background `claude --bg` processes.
They do not count against the `capacity` of the project's `claude-rc` Remote
Control server, which counts only sessions it spawned itself.
