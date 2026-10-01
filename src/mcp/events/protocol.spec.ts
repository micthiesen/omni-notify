import { Logger } from "@micthiesen/mitools/logging";
import { Client, StreamableHTTPClientTransport } from "@modelcontextprotocol/client";
import { Effect } from "effect";
import { z } from "zod";
import { afterAll, describe, expect, it } from "vitest";
import { createMitoolsTestRuntime } from "../../test/mitools.js";
import type { TaskRegistry } from "../../task-runs/registry.js";
import type { McpRuntime } from "../runtime.js";
import { createOmniMcpHandler } from "../server.js";
import { EmailEventService } from "./service.js";
import { EventPersistence } from "./persistence.js";
import { WebhookError } from "./webhook.js";

const token = "test-token-0123456789-ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const testRuntime = createMitoolsTestRuntime();
afterAll(() => testRuntime.dispose());

describe("Omni MCP Events protocol", () => {
  it("advertises events and handles list/subscribe/unsubscribe on the authenticated endpoint", async () => {
    const owner = `executor:${"a".repeat(64)}`;
    let verificationFails = false;
    let authorizationAccepted = true;
    const events = new EmailEventService(
      token,
      (candidate, bearer) =>
        Effect.succeed(
          authorizationAccepted &&
            candidate === owner &&
            bearer === "Bearer delegated-fixture",
        ),
      {
        verify: () =>
          verificationFails
            ? Effect.fail(new WebhookError({ reason: "challenge_failed" }))
            : Effect.void,
        deliver: () => Effect.succeed({ status: 204 }),
      },
    );
    const runtime: McpRuntime = {
      logger: Logger.named("McpEventsSpec"),
      effectRunner: testRuntime.runner,
      registry: {
        list: () => Effect.succeed([]),
        runNow: () => Effect.succeed({ runId: "test-run" }),
      } as unknown as TaskRegistry,
      streamers: [],
      emailControls: {},
      events,
    };
    const handler = createOmniMcpHandler(runtime, token);
    let rawDiscovery = "";
    const transport = new StreamableHTTPClientTransport(
      new URL("http://mcp.test/mcp"),
      {
        requestInit: {
          headers: {
            Authorization: `Bearer ${token}`,
            "x-omni-events-owner": owner,
            "x-omni-events-authorization": "Bearer delegated-fixture",
          },
        },
        fetch: async (input: RequestInfo | URL, init?: RequestInit) => {
          const request = new Request(input, init);
          const body: unknown =
            request.method === "POST" ? await request.clone().json() : undefined;
          const response = await handler.fetch(request);
          if (
            typeof body === "object" &&
            body !== null &&
            "method" in body &&
            body.method === "server/discover"
          ) {
            rawDiscovery = await response.clone().text();
          }
          return response;
        },
      },
    );
    const client = new Client(
      { name: "mcp-events-spec", version: "1.0.0" },
      { versionNegotiation: { mode: "auto" } },
    );
    try {
      await client.connect(transport);
      const discovery = await client.discover();
      expect(discovery.supportedVersions).toContain("2026-07-28");
      expect(rawDiscovery).toContain('"events"');

      const catalog = await client.request(
        { method: "events/list", params: {} },
        z.object({ events: z.array(z.object({ name: z.string() })) }),
      );
      expect(catalog.events.map((event) => event.name)).toEqual(["email.received"]);

      const input = {
        name: "email.received",
        arguments: { folder: "inbox" },
        delivery: {
          mode: "webhook",
          url: "https://chatgpt.example.com/events/callback",
          secret: `whsec_${Buffer.alloc(32, 9).toString("base64")}`,
        },
      };
      const subscribed = await client.request(
        { method: "events/subscribe", params: input },
        z.object({ id: z.string(), cursor: z.null(), refreshBefore: z.string() }),
      );
      expect(subscribed.id).toMatch(/^sub_/);
      const persisted = await testRuntime.run(
        EventPersistence.subscription(subscribed.id),
      );
      expect(persisted?.owner).toBe(owner);
      expect(persisted?.encryptedAuthorization).not.toContain("delegated-fixture");
      await expect(
        client.request(
          {
            method: "events/subscribe",
            params: {
              ...input,
              delivery: {
                ...input.delivery,
                url: "http://chatgpt.example.com/events/callback",
              },
            },
          },
          z.object({ id: z.string() }),
        ),
      ).rejects.toMatchObject({
        code: -32015,
        data: { reason: "invalid_callback" },
      });
      verificationFails = true;
      await expect(
        client.request(
          {
            method: "events/subscribe",
            params: {
              ...input,
              delivery: {
                ...input.delivery,
                url: "https://chatgpt.example.com/events/other",
              },
            },
          },
          z.object({ id: z.string() }),
        ),
      ).rejects.toMatchObject({
        code: -32015,
        data: { reason: "challenge_failed" },
      });
      verificationFails = false;
      authorizationAccepted = false;
      await expect(
        client.request(
          { method: "events/subscribe", params: input },
          z.object({ id: z.string() }),
        ),
      ).rejects.toMatchObject({
        code: -32001,
        data: { reason: "invalid_principal" },
      });
      authorizationAccepted = true;
      const removed = await client.request(
        {
          method: "events/unsubscribe",
          params: {
            name: input.name,
            arguments: input.arguments,
            delivery: { mode: "webhook", url: input.delivery.url },
          },
        },
        z.object({}),
      );
      expect(removed).toEqual(expect.objectContaining({}));
    } finally {
      await client.close();
      await handler.close();
    }
  });
});
