# Boris deployment

This is a separate Compose project under `/home/michael/compose/executor-events`.
It joins the existing `compose_agent-integrations` network, publishes no host
ports, and uses an exact adapter image SHA. Executor core, its data/authentication,
login/UI, and every other service keep their current configuration.

## Install or update after verification

Run on Boris from a checkout or copied directory containing these files:

```sh
python3 install.py VERIFIED_40_CHARACTER_SHA --owner EXISTING_EXECUTOR_USER_ID
```

The installer reads the resolved existing Omni MCP token into process memory
from the running Omni container, copies only that credential into mode-0600
`private.env`, and writes nonsecret version/owner settings to `deployment.env`.
It does not print credentials or edit the main Compose files, Reminders config,
or the existing `.env`. `deployment.env.previous` retains the previous pin.
The separate package image is built and published by the `Executor Events Adapter`
workflow in this repository. Check both that workflow and Omni's main workflow
for the exact SHA before installing. Do not use `latest` for this sidecar.

Omni defaults its delegated authorization check to
`http://executor:4788/api/auth/mcp/get-session` when Dockerized. The optional
`OMNI_EVENTS_EXECUTOR_AUTH_URL` overrides that fixed internal endpoint. The
adapter's owner ID is an existing Executor user, not a new account or API key.

## Exact MCP route

`route.mjs` is deliberately specific to the inspected NPM proxy host 44:
`mcp.syas.ca`, upstream `executor:4788`, no NPM access list. It refuses drift.
It adds one `location = /mcp` in that host's advanced configuration. OAuth,
well-known endpoints, UI paths, TLS and access lists remain unchanged.

From the directory containing this file on Boris:

```sh
docker exec -i -w /app -e OMNI_ROUTE_MODE=inspect npm node --input-type=module < route.mjs
docker exec -i -w /app -e OMNI_ROUTE_MODE=prepare npm node --input-type=module < route.mjs
# After compatibility, rollback and adapter health checks pass:
docker exec -i -w /app -e OMNI_ROUTE_MODE=apply npm node --input-type=module < route.mjs
```

Prepare validates an independent nginx configuration and saves a private copy
of the exact prior configuration. Apply uses NPM's own permission checks,
audit log and update/configuration API, with a short-lived locally signed token
for the host's existing owner. That token is kept in process memory and is not
persisted or printed. No user, permission, OAuth client, or signing key changes.
The script rejects concurrent edits and verifies unchanged host fields.

## Rollback

```sh
docker exec -i -w /app -e OMNI_ROUTE_MODE=rollback npm node --input-type=module < route.mjs
```

This restores the exact original advanced configuration, including unrelated
settings. MCP immediately reaches the unchanged Executor host again. Existing
legacy clients retain their endpoint, credentials and OAuth metadata. Event
subscriptions and outbox remain in Omni; rolling back the route does not erase
or unsubscribe them. Stop subscriptions first if delivery should stop.

To roll back the adapter image, copy `deployment.env.previous` over
`deployment.env`, then run from `/home/michael/compose/executor-events`:

```sh
docker compose --env-file deployment.env -f compose.yml up -d --wait
```

After routing rollback, the unused adapter can be stopped with the same Compose
command ending in `stop`. Do not stop or recreate Executor or NPM to update the
adapter. For an Omni code rollback use its prior image SHA; archive receipts
and event data are additive and do not require a destructive migration.

## Verify

Check adapter health and image revision, then exercise existing legacy
initialize/tools/list, native elicitation, modern server/discover/events/list,
OAuth 401 challenge and login/UI paths. Confirm other hosts retain their prior
status and routing. Use only read-only tools and synthetic mail fixtures.
Connected ChatGPT rescanning, subscription/challenge, matching callback delivery,
and idle-dot wake still need the parent-run lifecycle in `docs/mcp-events.md`.
