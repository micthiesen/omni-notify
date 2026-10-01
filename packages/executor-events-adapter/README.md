# Executor Events adapter

This private package sits at the existing Executor MCP URL. It forwards legacy
MCP traffic to the unchanged Executor container, serves MCP 2026-07-28 discovery,
bridges its tools to Executor's legacy MCP client, and forwards event methods to
Omni. OAuth registration, authorize/token, well-known discovery, and Executor's
web UI continue to go directly to Executor.

The deployed Executor 1.6.10 uses MCP SDK 1.29.0 and does not implement
`server/discover` or `events/*`. The adapter uses Executor's Better Auth
`GET /api/auth/mcp/get-session` endpoint for the opaque OAuth bearer. It reads
only `userId`, `clientId`, and `accessTokenExpiresAt`; it enforces expiry, then
binds event ownership to `executor:` plus SHA-256 of JSON-encoded
`[userId,clientId]`. Omni receives that identity and the original bearer over
the private Docker network, with its existing MCP token for service access.
Omni rechecks the bearer with Executor before delivery.

Configuration:

- `EXECUTOR_BASE_URL`, default `http://executor:4788`
- `OMNI_BASE_URL`, default `http://omni-notify:8080`
- `OMNI_MCP_TOKEN`, required; use the existing Compose secret value
- `EXECUTOR_ALLOWED_USER_ID`, required; the one Executor owner allowed to
  subscribe
- `PUBLIC_MCP_ORIGIN`, required; `https://mcp.syas.ca` on Boris so OAuth
  challenges retain the existing protected-resource URL
- `PORT`, default `4789`

Build with `docker build -t omni-executor-events-adapter .` from this directory.
Run `npm ci && npm run build && npm test` for local checks. The package and lock
file are independent of third-party Executor core. Update dependencies here and
rebuild only this image when Executor changes.

The intended Boris Compose addition is one container on the existing
`agent-integrations` network, with no published host port and the variables
above. Add an exact `/mcp` location for `mcp.syas.ca` in Nginx Proxy Manager
pointing to `executor-events-adapter:4789`; leave its current `/` location and
all other virtual hosts intact. This is a network routing change and must be
reviewed before it is applied. Rollback removes the exact location; legacy
traffic then reaches Executor directly. Pin the Executor image digest during a
deployment so the tested auth and tool contracts do not move under the adapter.

The adapter's `/health` responds without authentication inside the Docker
network. `/mcp` requires Executor OAuth for MCP 2026-07-28 requests. Legacy
requests retain Executor's existing authentication and transport.

Native elicitation keeps a single Executor tool call open across modern
`input_required` replies. Each invocation has a five-minute total deadline and
one-shot continuation state. If the adapter restarts or the deadline expires,
the pending reply fails safely; the user must explicitly start a new call.
