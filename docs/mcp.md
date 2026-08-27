# Executor MCP

Omni serves a streamable-HTTP MCP endpoint at `/mcp` on its existing HTTP port,
`FRONTEND_PORT` (3000 by default). The endpoint uses the official MCP server
transport and supports the normal MCP initialization, discovery, and tool-call
flow.

Every request to `/mcp` requires this header:

```http
Authorization: Bearer <token>
```

The server reads the token only from `OMNI_MCP_TOKEN`. In production it refuses
to start when the value is absent, shorter than 32 characters, or has fewer than
12 distinct characters. Authentication compares fixed-length token digests in
constant time, returns `401` for missing or invalid credentials, disables
caching, and never logs the token. Tests use explicit fake tokens. Production
tokens belong in the Boris Compose secret configuration and must not be added to
this repository.

## Tool surface

The registered tools are bounded adapters over existing Omni services. The
families cover:

- email search, retrieval, activity, rules, feedback, retries, reprocessing, and SMTP sending
- CalDAV event inspection, preview, creation, update, and deletion
- task status, task runs, livestreams, briefings, workspaces, actions, and papercuts
- media library, watchlist, recommendations, podcast accounts, and podcast recommendations
- PressPods jobs and episodes, pet weights, aggregate costs, web search, and iOS live-control diagnostics

The server does not expose arbitrary shell or filesystem access, environment
values, general database access, secret-bearing HTTP, raw attachment or audio
bytes, or caller-selected SMTP identities. Inputs are typed and validated.
Searches and listings use pagination or fixed bounds, and large text fields are
truncated with explicit metadata.

Production email uses the active iCloud IMAP transport, SMTP client, and iCloud
CalDAV discovery. The legacy Fastmail JMAP transport remains selectable but is
not the basis of the MCP design.

## Executor policy

Executor is expected to apply policy before every tool call. Reads, searches,
drafts, previews, and ordinary reversible local changes are normally allowed.
External communications and consequential actions require explicit owner
approval. This includes sending email, changing calendars or podcast accounts,
starting media acquisition, publishing PressPods content, invoking paid models
or search, and running configured workflows whose downstream effects or costs
are material.

MCP annotations describe behavior only. They do not grant approval and
`destructiveHint` is not used merely to signal approval risk. The complete
machine-readable contract is [`docs/mcp-policy.json`](mcp-policy.json). Each
entry contains the actual annotations, side effects, cost characteristics, and
one recommended Executor policy: `allow`, `require_approval`, or `block`.

Regenerate the inventory after changing registered tools:

```bash
pnpm mcp:policy
```

Tests compare the committed inventory with the definitions registered by the
server, so policy drift fails the suite.

## Deployment boundary

Omni owns the code, endpoint, authentication middleware, schemas, tool
implementations, and policy inventory. The Boris Compose deployment owns the
production token, Executor configuration, reverse-proxy exposure, networks, and
live approval policies. Only `/mcp` needs to be routed to the existing container
port; Omni's current web routes and health behavior remain unchanged.
