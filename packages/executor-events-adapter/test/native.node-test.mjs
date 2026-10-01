import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { createServer } from "node:http";
import { test } from "node:test";
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/streamableHttp.js";
import { isInitializeRequest } from "@modelcontextprotocol/sdk/types.js";
import { Client, StreamableHTTPClientTransport } from "@modelcontextprotocol/client";
import { start } from "../dist/server.js";

function listen(server) {
  return new Promise((resolve) =>
    server.listen(0, "127.0.0.1", () =>
      resolve(`http://127.0.0.1:${server.address().port}`),
    ),
  );
}

test("MCP 2 native elicitation completes through a real legacy MCP session", async () => {
  let calls = 0;
  const sessions = new Map();
  const upstream = createServer(async (req, res) => {
    if (req.url === "/api/auth/mcp/get-session") {
      res.setHeader("content-type", "application/json");
      res.end(
        JSON.stringify({
          userId: "u",
          clientId: "c",
          accessTokenExpiresAt: new Date(Date.now() + 60000).toISOString(),
        }),
      );
      return;
    }
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    const parsed = chunks.length
      ? JSON.parse(Buffer.concat(chunks).toString())
      : undefined;
    let transport = sessions.get(req.headers["mcp-session-id"]);
    if (!transport && isInitializeRequest(parsed)) {
      transport = new StreamableHTTPServerTransport({
        sessionIdGenerator: randomUUID,
        onsessioninitialized: (id) => sessions.set(id, transport),
      });
      const mcp = new McpServer(
        { name: "fake-executor", version: "1.0.0" },
        { capabilities: { tools: {} } },
      );
      mcp.registerTool("confirm", { inputSchema: {} }, async () => {
        calls++;
        const reply = await mcp.server.elicitInput({
          mode: "form",
          message: "Confirm?",
          requestedSchema: {
            type: "object",
            properties: { yes: { type: "boolean" } },
            required: ["yes"],
          },
        });
        return {
          content: [
            {
              type: "text",
              text:
                reply.action === "accept" && reply.content?.yes === true
                  ? "accepted"
                  : "declined",
            },
          ],
        };
      });
      await mcp.connect(transport);
    }
    if (!transport) {
      res.writeHead(404).end();
      return;
    }
    await transport.handleRequest(req, res, parsed);
  });
  const executorBaseUrl = await listen(upstream);
  const adapter = start(
    {
      executorBaseUrl,
      omniBaseUrl: executorBaseUrl,
      omniMcpToken: "unused",
      allowedUserId: "u",
      publicMcpOrigin: "https://mcp.syas.ca",
    },
    0,
  );
  await new Promise((resolve) => adapter.once("listening", resolve));
  const client = new Client(
    { name: "native-test", version: "1.0.0" },
    {
      versionNegotiation: { mode: { pin: "2026-07-28" } },
      capabilities: { elicitation: { form: {} } },
    },
  );
  client.setRequestHandler("elicitation/create", async () => ({
    action: "accept",
    content: { yes: true },
  }));
  try {
    const transport = new StreamableHTTPClientTransport(
      new URL(`http://127.0.0.1:${adapter.address().port}/mcp?elicitation_mode=native`),
      {
        requestInit: { headers: { authorization: "Bearer test" } },
      },
    );
    await client.connect(transport);
    const result = await client.callTool({ name: "confirm", arguments: {} });
    assert.equal(result.content[0].text, "accepted");
    assert.equal(calls, 1);
  } finally {
    await client.close();
    await new Promise((resolve) => adapter.close(resolve));
    for (const transport of sessions.values()) await transport.close();
    await new Promise((resolve) => upstream.close(resolve));
  }
});
