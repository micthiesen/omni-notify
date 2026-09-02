import { type ServerType, serve } from "@hono/node-server";
import { Logger } from "@micthiesen/mitools/logging";
import { Client, StreamableHTTPClientTransport } from "@modelcontextprotocol/client";
import { Effect } from "effect";
import { Hono } from "hono";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { EmailTransport, FetchedEmail } from "../email/types.js";
import type { PodcastAccountClient } from "../podcast-recs/account.js";
import type { PrinterService } from "../printer/service.js";
import type { TaskRegistry } from "../task-runs/registry.js";
import { registerOmniMcpRoute } from "./route.js";
import type { McpRuntime } from "./runtime.js";
import { createOmniMcpHandler, MCP_SERVER_INSTRUCTIONS } from "./server.js";

const TEST_TOKEN = "test-token-0123456789-ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const handlers: Array<ReturnType<typeof createOmniMcpHandler>> = [];
const clients: Client[] = [];
const httpServers: ServerType[] = [];

function runtime(overrides: Partial<McpRuntime> = {}): McpRuntime {
  return {
    logger: new Logger("McpServerSpec"),
    registry: {
      list: () => [],
      runNow: () => ({ runId: "test-run" }),
    } as unknown as TaskRegistry,
    streamers: [],
    emailControls: {},
    ...overrides,
  };
}

async function connectClient(
  handlerOrUrl: ReturnType<typeof createOmniMcpHandler> | URL,
): Promise<Client> {
  const isUrl = handlerOrUrl instanceof URL;
  const transport = new StreamableHTTPClientTransport(
    isUrl ? handlerOrUrl : new URL("http://mcp.test/mcp"),
    {
      requestInit: { headers: { Authorization: `Bearer ${TEST_TOKEN}` } },
      ...(isUrl
        ? {}
        : {
            fetch: (input: RequestInfo | URL, init?: RequestInit) =>
              handlerOrUrl.fetch(new Request(input, init)),
          }),
    },
  );
  const client = new Client(
    { name: "omni-mcp-spec", version: "1.0.0" },
    { versionNegotiation: { mode: "auto" } },
  );
  await client.connect(transport);
  clients.push(client);
  return client;
}

async function startMcpHttpServer(
  testRuntime: McpRuntime,
  token?: string,
): Promise<{ url: URL; handler: ReturnType<typeof registerOmniMcpRoute> }> {
  const app = new Hono();
  const handler = registerOmniMcpRoute(app, testRuntime, token);
  let listening: (port: number) => void = () => undefined;
  const ready = new Promise<number>((resolve) => {
    listening = resolve;
  });
  const server = serve({ fetch: app.fetch, hostname: "127.0.0.1", port: 0 }, (info) => {
    listening(info.port);
  });
  httpServers.push(server);
  const port = await ready;
  return { url: new URL(`http://127.0.0.1:${port}/mcp`), handler };
}

afterEach(async () => {
  await Promise.all(clients.splice(0).map((client) => client.close()));
  await Promise.all(handlers.splice(0).map((handler) => handler.close()));
  await Promise.all(
    httpServers.splice(0).map(
      (server) =>
        new Promise<void>((resolve, reject) => {
          server.close((error) => (error ? reject(error) : resolve()));
          if ("closeAllConnections" in server) server.closeAllConnections();
        }),
    ),
  );
});

describe("Omni MCP streamable HTTP server", () => {
  it("returns 401 before MCP handling for missing and invalid credentials", async () => {
    const handler = createOmniMcpHandler(runtime(), TEST_TOKEN);
    handlers.push(handler);
    const body = JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize" });

    for (const authorization of [undefined, "Bearer wrong-token", "Basic abc"]) {
      const headers: Record<string, string> = { "Content-Type": "application/json" };
      if (authorization) headers.Authorization = authorization;
      const response = await handler.fetch(
        new Request("http://mcp.test/mcp", { method: "POST", headers, body }),
      );
      expect(response.status).toBe(401);
      expect(response.headers.get("WWW-Authenticate")).toBe("Bearer");
      expect(await response.json()).toEqual({ error: "Unauthorized" });
    }
  });

  it("serves the production Hono route over real HTTP with auth and clean shutdown", async () => {
    const testRuntime = runtime();
    const unconfigured = await startMcpHttpServer(testRuntime, undefined);
    expect((await fetch(unconfigured.url, { method: "POST" })).status).toBe(503);

    const configured = await startMcpHttpServer(testRuntime, TEST_TOKEN);
    handlers.push(configured.handler!);
    const unauthorized = await fetch(configured.url, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize" }),
    });
    expect(unauthorized.status).toBe(401);
    expect(unauthorized.headers.get("WWW-Authenticate")).toBe("Bearer");

    const client = await connectClient(configured.url);
    expect(client.getServerVersion()).toMatchObject({ name: "omni" });
    expect((await client.listTools()).tools.length).toBe(
      configured.handler!.tools.length,
    );
    await client.close();
    clients.splice(clients.indexOf(client), 1);
  });

  it("initializes and lists the complete typed tool surface with the official client", async () => {
    const handler = createOmniMcpHandler(runtime(), TEST_TOKEN);
    handlers.push(handler);
    const client = await connectClient(handler);

    expect(client.getProtocolEra()).toBe("modern");
    expect(client.getServerVersion()).toMatchObject({ name: "omni" });
    expect(client.getInstructions()).toBe(MCP_SERVER_INSTRUCTIONS);
    const { tools } = await client.listTools();
    expect(tools.length).toBe(handler.tools.length);
    expect(tools.map(({ name }) => name)).toContain("email_search");
    for (const tool of tools) {
      expect(tool.inputSchema.type).toBe("object");
      expect(tool.outputSchema?.type).toBe("object");
      expect(tool.annotations).toEqual({
        readOnlyHint: expect.any(Boolean),
        destructiveHint: expect.any(Boolean),
        idempotentHint: expect.any(Boolean),
        openWorldHint: expect.any(Boolean),
      });
    }
  });

  it("executes representative read and mocked mutation calls", async () => {
    const runNow = vi.fn(() => ({ runId: "mock-run-123" }));
    const handler = createOmniMcpHandler(
      runtime({
        registry: { list: () => [], runNow } as unknown as TaskRegistry,
      }),
      TEST_TOKEN,
    );
    handlers.push(handler);
    const client = await connectClient(handler);

    const read = await client.callTool({ name: "tasks_list", arguments: {} });
    expect(read.isError).not.toBe(true);
    expect(read.structuredContent).toEqual({
      tasks: [],
      nextCursor: null,
      total: 0,
    });

    const mutation = await client.callTool({
      name: "task_run",
      arguments: { taskName: "MockTask" },
    });
    expect(mutation.isError).not.toBe(true);
    expect(mutation.structuredContent).toEqual({
      runId: "mock-run-123",
      taskName: "MockTask",
      queued: true,
    });
    expect(runNow).toHaveBeenCalledWith("MockTask", undefined);
  });

  it("paginates reads, rejects malformed bounds, and preserves policy semantics", async () => {
    const tasks = ["Alpha", "Beta", "Gamma"].map((name) => ({
      name,
      displayName: undefined,
      schedule: "0 * * * *",
      running: false,
      nextRuns: [],
      lastRun: null,
    }));
    const handler = createOmniMcpHandler(
      runtime({ registry: { list: () => tasks } as unknown as TaskRegistry }),
      TEST_TOKEN,
    );
    handlers.push(handler);
    const client = await connectClient(handler);

    const first = await client.callTool({
      name: "tasks_list",
      arguments: { cursor: 0, limit: 2 },
    });
    expect(first.structuredContent).toMatchObject({ total: 3, nextCursor: 2 });
    expect((first.structuredContent as { tasks: unknown[] }).tasks).toHaveLength(2);
    const second = await client.callTool({
      name: "tasks_list",
      arguments: { cursor: 2, limit: 2 },
    });
    expect(second.structuredContent).toMatchObject({ total: 3, nextCursor: null });
    expect((second.structuredContent as { tasks: unknown[] }).tasks).toHaveLength(1);

    const malformed = await client.callTool({
      name: "tasks_list",
      arguments: { cursor: -1, limit: 101, unexpected: true },
    });
    expect(malformed.isError).toBe(true);

    const unboundedCosts = await client.callTool({
      name: "costs_read",
      arguments: { days: "all" },
    });
    expect(unboundedCosts.isError).toBe(true);

    const listed = (await client.listTools()).tools;
    expect(listed.find((tool) => tool.name === "task_run")?.annotations).toEqual({
      readOnlyHint: false,
      destructiveHint: false,
      idempotentHint: false,
      openWorldHint: true,
    });
    const taskPolicy = handler.tools.find((tool) => tool.name === "task_run")?.policy;
    expect(taskPolicy?.recommendedPolicy).toBe("require_approval");
    expect(taskPolicy?.sideEffects).toContain("Queues task execution");
    expect(
      handler.tools.find((tool) => tool.name === "email_rules_delete")?.annotations
        .destructiveHint,
    ).toBe(true);
  });

  it("runs a mocked consequential podcast-account mutation without external effects", async () => {
    const dequeueEpisode = vi.fn(() => Effect.succeed("removed" as const));
    const handler = createOmniMcpHandler(
      runtime({
        podcastAccount: {
          name: "MockCastro",
          dequeueEpisode,
        } as unknown as PodcastAccountClient,
      }),
      TEST_TOKEN,
    );
    handlers.push(handler);
    const client = await connectClient(handler);

    const result = await client.callTool({
      name: "podcast_account_update",
      arguments: { action: "dequeue", episodeGuid: "episode-guid-1" },
    });
    expect(result.isError).not.toBe(true);
    expect(result.structuredContent).toEqual({
      account: "MockCastro",
      action: "dequeue",
      result: "removed",
    });
    expect(dequeueEpisode).toHaveBeenCalledWith("episode-guid-1");
    expect(
      handler.tools.find((tool) => tool.name === "podcast_account_update")?.policy,
    ).toMatchObject({ recommendedPolicy: "require_approval" });
  });

  it("exposes guarded printer status and physical printing through a mocked service", async () => {
    const statusEffect = vi.fn(() =>
      Effect.succeed({
        configured: true as const,
        name: "Brother HL-L2370DW series",
        uri: "ipp://printer.test/ipp/print",
        state: "idle" as const,
        stateReasons: [],
        ready: true,
        acceptingJobs: true,
        queuedJobCount: 0,
        tonerPercent: 20,
        monochromeOnly: true as const,
        defaultSides: "two-sided-long-edge" as const,
        supportedFormats: ["application/octet-stream"],
        supportedMedia: ["na_letter_8.5x11in"],
      }),
    );
    const printPdfEffect = vi.fn(() =>
      Effect.succeed({
        accepted: true as const,
        completed: true,
        jobId: 42,
        jobUri: "ipp://printer.test/jobs/42",
        jobState: "completed" as const,
        jobName: "Test document",
        pages: 2,
        copies: 1,
        paper: "letter" as const,
        sides: "two-sided-long-edge" as const,
        impressionsCompleted: 2,
        message: "The printer completed the job successfully",
      }),
    );
    const handler = createOmniMcpHandler(
      runtime({ printer: { statusEffect, printPdfEffect } as PrinterService }),
      TEST_TOKEN,
    );
    handlers.push(handler);
    const client = await connectClient(handler);

    const read = await client.callTool({
      name: "get_printer_status",
      arguments: {},
    });
    expect(read.isError).not.toBe(true);
    expect(read.structuredContent).toMatchObject({
      name: "Brother HL-L2370DW series",
      ready: true,
      tonerPercent: 20,
    });

    const print = await client.callTool({
      name: "print_document",
      arguments: {
        url: "https://example.com/document.pdf",
        jobName: "Test document",
      },
    });
    expect(print.isError).not.toBe(true);
    expect(print.structuredContent).toMatchObject({ accepted: true, jobId: 42 });
    expect(printPdfEffect).toHaveBeenCalledWith({
      url: "https://example.com/document.pdf",
      jobName: "Test document",
      copies: 1,
      paper: "letter",
      sides: "two-sided-long-edge",
      allowDuplicate: false,
    });

    const tool = handler.tools.find(({ name }) => name === "print_document");
    expect(tool?.annotations).toEqual({
      readOnlyHint: false,
      destructiveHint: false,
      idempotentHint: false,
      openWorldHint: true,
    });
    expect(tool?.policy.recommendedPolicy).toBe("require_approval");

    const malformed = await client.callTool({
      name: "print_document",
      arguments: { url: "file:///tmp/document.pdf", copies: 99 },
    });
    expect(malformed.isError).toBe(true);
    expect(printPdfEffect).toHaveBeenCalledTimes(1);
  });

  it("reports printer tools as unavailable when printing is not configured", async () => {
    const handler = createOmniMcpHandler(runtime(), TEST_TOKEN);
    handlers.push(handler);
    const client = await connectClient(handler);
    const result = await client.callTool({
      name: "get_printer_status",
      arguments: {},
    });
    expect(result.isError).toBe(true);
    expect(result.content).toContainEqual({
      type: "text",
      text: "Printer is not configured",
    });
  });

  it("bounds email body output and returns useful tool errors", async () => {
    const email: FetchedEmail = {
      id: "message-1",
      subject: "Bounded message",
      from: "sender@example.test",
      textBody: "x".repeat(50_000),
      links: [],
      receivedAt: "2026-08-26T12:00:00.000Z",
      attachments: [],
    };
    const transport = {
      name: "IMAP",
      fetchEmailByIdEffect: vi.fn((id: string) =>
        Effect.succeed(id === email.id ? email : undefined),
      ),
    } as unknown as EmailTransport;
    const handler = createOmniMcpHandler(
      runtime({ emailControls: { transport } }),
      TEST_TOKEN,
    );
    handlers.push(handler);
    const client = await connectClient(handler);

    const bounded = await client.callTool({
      name: "email_get",
      arguments: { emailId: email.id, bodyChars: 1_000 },
    });
    const output = bounded.structuredContent as {
      email: { excerpt: string; excerptTruncated: boolean };
    };
    expect(output.email.excerpt).toHaveLength(1_000);
    expect(output.email.excerptTruncated).toBe(true);

    const missing = await client.callTool({
      name: "email_get",
      arguments: { emailId: "missing" },
    });
    expect(missing.isError).toBe(true);
    expect(missing.content).toContainEqual({
      type: "text",
      text: "Email no longer exists in the monitored mailbox",
    });
  });

  it("closes cleanly after initialization", async () => {
    const handler = createOmniMcpHandler(runtime(), TEST_TOKEN);
    const client = await connectClient(handler);
    await client.close();
    clients.splice(clients.indexOf(client), 1);
    await expect(handler.close()).resolves.toBeUndefined();
  });
});
