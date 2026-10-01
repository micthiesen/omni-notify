import assert from "node:assert/strict";
import { createServer } from "node:http";
import { after, before, test } from "node:test";
import { Effect } from "effect";
import {
  Client as ModernClient,
  StreamableHTTPClientTransport as ModernTransport,
} from "@modelcontextprotocol/client";
import { ownerId, authenticateOAuth } from "../dist/auth.js";
import { Continuations } from "../dist/continuations.js";
import { start } from "../dist/server.js";

const token = "test-oauth-token";
const userId = "owner-user";
const clientId = "client-id";
const options = {
  executorBaseUrl: "",
  omniBaseUrl: "",
  omniMcpToken: "omni-test-token",
  allowedUserId: userId,
  publicMcpOrigin: "https://mcp.syas.ca",
};
let executor;
let omni;
let adapter;
let received = null;

function listen(server) {
  return new Promise((resolve) =>
    server.listen(0, "127.0.0.1", () =>
      resolve(`http://127.0.0.1:${server.address().port}`),
    ),
  );
}

before(async () => {
  executor = createServer(async (req, res) => {
    if (req.url === "/api/auth/mcp/get-session") {
      res.setHeader("content-type", "application/json");
      res.end(
        req.headers.authorization === `Bearer ${token}`
          ? JSON.stringify({
              userId,
              clientId,
              accessTokenExpiresAt: new Date(Date.now() + 60000).toISOString(),
              accessToken: "must-not-leak",
              refreshToken: "must-not-leak",
            })
          : "null",
      );
      return;
    }
    if (req.url.startsWith("/mcp")) {
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify({ legacy: true, path: req.url }));
      return;
    }
    res.writeHead(404).end();
  });
  omni = createServer(async (req, res) => {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    received = {
      headers: req.headers,
      path: req.url,
      body: JSON.parse(Buffer.concat(chunks).toString()),
    };
    if (received.headers["mcp-method"] !== received.body.method) {
      res.writeHead(400, { "content-type": "application/json" });
      res.end(JSON.stringify({ error: { message: "Mcp-Method header mismatch" } }));
      return;
    }
    res.setHeader("content-type", "application/json");
    res.end(
      JSON.stringify({ jsonrpc: "2.0", id: received.body.id, result: { events: [] } }),
    );
  });
  options.executorBaseUrl = await listen(executor);
  options.omniBaseUrl = await listen(omni);
  adapter = start(options, 0);
  await new Promise((resolve) => adapter.once("listening", resolve));
});

after(async () => {
  await Promise.all(
    [executor, omni, adapter].map(
      (server) => new Promise((resolve) => server.close(resolve)),
    ),
  );
});

function request(
  method,
  params = {},
  auth = token,
  path = "/mcp?elicitation_mode=native",
) {
  return fetch(`http://127.0.0.1:${adapter.address().port}${path}`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${auth}`,
      "content-type": "application/json",
      "mcp-protocol-version": "2026-07-28",
    },
    body: JSON.stringify({
      jsonrpc: "2.0",
      id: 7,
      method,
      params: {
        ...params,
        _meta: { "io.modelcontextprotocol/protocolVersion": "2026-07-28" },
      },
    }),
  });
}

test("OAuth projection uses stable user/client identity and never exposes session secrets", async () => {
  const identity = await Effect.runPromise(
    authenticateOAuth(`Bearer ${token}`, options),
  );
  assert.deepEqual(identity, {
    owner: ownerId(userId, clientId),
    authorization: `Bearer ${token}`,
    expiresAt: identity.expiresAt,
  });
  assert.equal(Object.hasOwn(identity, "accessToken"), false);
  assert.equal(
    await Effect.runPromise(authenticateOAuth("Bearer invalid", options)),
    null,
  );
});

test("modern discovery and event forwarding use the same OAuth owner", async () => {
  const discovered = await (await request("server/discover")).json();
  assert.deepEqual(discovered.result.supportedVersions, ["2026-07-28"]);
  assert.deepEqual(Object.keys(discovered.result.capabilities).sort(), [
    "events",
    "resources",
    "tools",
  ]);
  const events = await (await request("events/list")).json();
  assert.deepEqual(events.result.events, []);
  assert.equal(received.path, "/mcp");
  assert.equal(received.headers["x-omni-events-owner"], ownerId(userId, clientId));
  assert.equal(received.headers["x-omni-events-authorization"], `Bearer ${token}`);
  assert.equal(received.headers.authorization, "Bearer omni-test-token");
  assert.equal(received.headers["mcp-method"], "events/list");
});

test("the MCP 2 client negotiates the modern protocol against the adapter", async () => {
  const client = new ModernClient(
    { name: "adapter-test", version: "1.0.0" },
    { versionNegotiation: { mode: { pin: "2026-07-28" } } },
  );
  const transport = new ModernTransport(
    new URL(`http://127.0.0.1:${adapter.address().port}/mcp?elicitation_mode=native`),
    {
      requestInit: { headers: { authorization: `Bearer ${token}` } },
    },
  );
  try {
    await client.connect(transport);
    assert.equal(client.getProtocolEra(), "modern");
  } finally {
    await client.close();
  }
});

test("modern event methods reject invalid OAuth before reaching Omni", async () => {
  received = null;
  const response = await request("events/list", {}, "invalid");
  assert.equal(response.status, 401);
  assert.equal(
    response.headers.get("www-authenticate"),
    'Bearer resource_metadata="https://mcp.syas.ca/.well-known/oauth-protected-resource"',
  );
  assert.equal(received, null);
});

test("modern protocol header and request metadata must agree", async () => {
  received = null;
  const response = await fetch(`http://127.0.0.1:${adapter.address().port}/mcp`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
      "mcp-protocol-version": "2026-07-28",
    },
    body: JSON.stringify({
      jsonrpc: "2.0",
      id: 9,
      method: "events/list",
      params: {
        _meta: { "io.modelcontextprotocol/protocolVersion": "2025-06-18" },
      },
    }),
  });
  assert.equal(response.status, 400);
  assert.equal((await response.json()).error.code, -32020);
  assert.equal(received, null);
});

test("unsupported modern version fails without forwarding to legacy Executor", async () => {
  const response = await fetch(`http://127.0.0.1:${adapter.address().port}/mcp`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
      "mcp-protocol-version": "2026-09-01",
    },
    body: JSON.stringify({ jsonrpc: "2.0", id: 10, method: "tools/list" }),
  });
  assert.equal(response.status, 400);
  assert.equal((await response.json()).error.code, -32022);
});

test("legacy MCP retains its native query and upstream route", async () => {
  const response = await fetch(
    `http://127.0.0.1:${adapter.address().port}/mcp?elicitation_mode=native`,
    {
      method: "POST",
      headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize" }),
    },
  );
  assert.deepEqual(await response.json(), {
    legacy: true,
    path: "/mcp?elicitation_mode=native",
  });
});

test("native elicitation resumes one invocation and rejects replay and another owner", async () => {
  let calls = 0;
  let handler;
  let closed = 0;
  const fake = {
    setRequestHandler(_schema, callback) {
      handler = callback;
    },
    callTool() {
      calls++;
      return new Promise((resolve) => {
        queueMicrotask(async () => {
          const answer = await handler({
            method: "elicitation/create",
            params: {
              message: "Confirm?",
              requestedSchema: { type: "object", properties: {} },
            },
          });
          resolve({ content: [{ type: "text", text: answer.action }] });
        });
      });
    },
    async close() {
      closed++;
    },
  };
  const manager = new Continuations(
    options,
    5000,
    (_options, _auth, _query, _interactive, configure) => {
      configure(fake);
      return Effect.succeed(fake);
    },
  );
  const params = { name: "execute", arguments: { code: "check()" } };
  const first = await Effect.runPromise(
    manager.call(
      "owner-1",
      "Bearer x",
      new URLSearchParams("elicitation_mode=native"),
      params,
    ),
  );
  assert.equal(first.resultType, "input_required");
  assert.equal(first.inputRequests.elicitation.method, "elicitation/create");
  const replay = {
    ...params,
    requestState: first.requestState,
    inputResponses: { elicitation: { action: "accept", content: {} } },
  };
  await assert.rejects(
    Effect.runPromise(
      manager.call("owner-2", "Bearer y", new URLSearchParams(), replay),
    ),
  );
  const result = await Effect.runPromise(
    manager.call("owner-1", "Bearer x", new URLSearchParams(), replay),
  );
  assert.equal(result.resultType, "complete");
  assert.equal(result.content[0].text, "accept");
  await assert.rejects(
    Effect.runPromise(
      manager.call("owner-1", "Bearer x", new URLSearchParams(), replay),
    ),
  );
  assert.equal(calls, 1);
  assert.equal(closed, 1);
});

test("concurrent native replies consume a continuation once and decline safely", async () => {
  let calls = 0;
  let handler;
  const fake = {
    setRequestHandler(_schema, callback) {
      handler = callback;
    },
    callTool() {
      calls++;
      return new Promise((resolve) =>
        queueMicrotask(async () => {
          const response = await handler({
            method: "elicitation/create",
            params: {
              mode: "url",
              message: "Open link?",
              url: "https://example.test/",
            },
          });
          resolve({ content: [{ type: "text", text: response.action }] });
        }),
      );
    },
    async close() {},
  };
  const manager = new Continuations(
    options,
    5000,
    (_options, _auth, _query, _support, configure) => {
      configure(fake);
      return Effect.succeed(fake);
    },
    1,
  );
  const params = { name: "execute", arguments: { code: "link()" } };
  const first = await Effect.runPromise(
    manager.call("owner", "Bearer x", new URLSearchParams(), params, {
      form: false,
      url: true,
    }),
  );
  const secondInvocation = Effect.runPromise(
    manager.call("owner", "Bearer x", new URLSearchParams(), params, {
      form: false,
      url: true,
    }),
  );
  await assert.rejects(secondInvocation);
  const retry = {
    ...params,
    requestState: first.requestState,
    inputResponses: { elicitation: { action: "decline" } },
  };
  const [a, b] = await Promise.allSettled([
    Effect.runPromise(manager.call("owner", "Bearer x", new URLSearchParams(), retry)),
    Effect.runPromise(manager.call("owner", "Bearer x", new URLSearchParams(), retry)),
  ]);
  assert.equal(
    [a.status, b.status].filter((status) => status === "fulfilled").length,
    1,
  );
  assert.equal(
    (a.status === "fulfilled" ? a.value : b.value).content[0].text,
    "decline",
  );
  assert.equal(calls, 1);
});

test("a stalled tool before elicitation expires and releases its capacity", async () => {
  let calls = 0;
  let closed = 0;
  const fake = {
    setRequestHandler() {},
    callTool() {
      calls++;
      if (calls === 1) return new Promise(() => {});
      return Promise.resolve({ content: [{ type: "text", text: "recovered" }] });
    },
    async close() {
      closed++;
    },
  };
  const manager = new Continuations(options, 80, () => Effect.succeed(fake), 1);
  const params = { name: "execute", arguments: { code: "once()" } };
  await assert.rejects(
    Effect.runPromise(manager.call("owner", "Bearer x", new URLSearchParams(), params)),
  );
  assert.equal(closed, 1);
  const recovered = await Effect.runPromise(
    manager.call("owner", "Bearer x", new URLSearchParams(), params),
  );
  assert.equal(recovered.content[0].text, "recovered");
  assert.equal(calls, 2);
  assert.equal(closed, 2);
});

test("a stalled tool after accepted elicitation expires without replay", async () => {
  let calls = 0;
  let closed = 0;
  let handler;
  const fake = {
    setRequestHandler(_schema, callback) {
      handler = callback;
    },
    callTool() {
      calls++;
      if (calls > 1)
        return Promise.resolve({ content: [{ type: "text", text: "recovered" }] });
      queueMicrotask(() => {
        void handler({
          method: "elicitation/create",
          params: { message: "Confirm?", requestedSchema: { type: "object" } },
        });
      });
      return new Promise(() => {});
    },
    async close() {
      closed++;
    },
  };
  const manager = new Continuations(
    options,
    100,
    (_options, _auth, _query, _support, configure) => {
      configure(fake);
      return Effect.succeed(fake);
    },
    1,
  );
  const params = { name: "execute", arguments: { code: "once()" } };
  const first = await Effect.runPromise(
    manager.call("owner", "Bearer x", new URLSearchParams(), params),
  );
  const reply = {
    ...params,
    requestState: first.requestState,
    inputResponses: { elicitation: { action: "accept", content: {} } },
  };
  await assert.rejects(
    Effect.runPromise(manager.call("owner", "Bearer x", new URLSearchParams(), reply)),
  );
  assert.equal(closed, 1);
  await assert.rejects(
    Effect.runPromise(manager.call("owner", "Bearer x", new URLSearchParams(), reply)),
  );
  assert.equal(calls, 1);
  const recovered = await Effect.runPromise(
    manager.call("owner", "Bearer x", new URLSearchParams(), params),
  );
  assert.equal(recovered.content[0].text, "recovered");
  assert.equal(closed, 2);
});
