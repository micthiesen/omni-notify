import type { McpServer } from "@modelcontextprotocol/server";
import { z } from "zod";
import type { McpRuntime } from "../runtime.js";
import { EventSubscriptionError, type EventPrincipal } from "./service.js";

const folderSchema = z.enum(["inbox", "archive"]);
const argumentsSchema = z.object({ folder: folderSchema }).strict();
const deliverySchema = z
  .object({
    mode: z.literal("webhook"),
    url: z.url(),
    secret: z.string(),
  })
  .strict();
const subscribeSchema = z
  .object({
    name: z.string(),
    arguments: argumentsSchema,
    delivery: deliverySchema,
    ttlMs: z.number().int().positive().finite().nullable().optional(),
    cursor: z.string().nullable().optional(),
  })
  .strict();
const unsubscribeSchema = z
  .object({
    name: z.string(),
    arguments: argumentsSchema,
    delivery: deliverySchema.omit({ secret: true }),
  })
  .strict();
const listSchema = z.object({ cursor: z.string().optional() }).strict();
const OWNER_PATTERN = /^executor:[0-9a-f]{64}$/;

const EVENT_CATALOG = [
  {
    name: "email.received",
    description:
      "A new iCloud message arrived in the selected Inbox or Archive mailbox. Read full mail with email tools using messageId.",
    delivery: ["webhook"],
    inputSchema: {
      type: "object",
      properties: {
        folder: { type: "string", enum: ["inbox", "archive"] },
      },
      required: ["folder"],
      additionalProperties: false,
    },
    payloadSchema: {
      type: "object",
      properties: {
        messageId: { type: "string" },
        folder: { type: "string", enum: ["inbox", "archive"] },
        uidValidity: { type: "string" },
        uid: { type: "integer" },
      },
      required: ["messageId", "folder", "uidValidity", "uid"],
      additionalProperties: false,
    },
  },
];

function rpcError(error: EventSubscriptionError): Error {
  const callback = ["invalid_callback", "challenge_failed", "timeout"].includes(
    error.reason,
  );
  return Object.assign(new Error(callback ? "CallbackEndpointError" : error.message), {
    code: error.reason === "invalid_principal" ? -32001 : callback ? -32015 : -32602,
    data: { reason: error.reason },
  });
}

function principalFromHeaders(headers?: Headers): EventPrincipal | undefined {
  const owner = headers?.get("x-omni-events-owner");
  const authorization = headers?.get("x-omni-events-authorization");
  if (!owner && !authorization) return undefined;
  if (
    !owner ||
    !OWNER_PATTERN.test(owner) ||
    !authorization ||
    !/^Bearer [^\s]{1,8192}$/.test(authorization)
  ) {
    throw Object.assign(new Error("Invalid event principal"), { code: -32602 });
  }
  return { owner, authorization };
}

/** The installed SDK has a supported custom-method seam on its low-level server. */
export function registerEventMethods(server: McpServer, runtime: McpRuntime): void {
  const events = runtime.events;
  if (!events) return;
  server.server.registerCapabilities({ events: {} } as Parameters<
    typeof server.server.registerCapabilities
  >[0]);
  server.server.setRequestHandler(
    "events/list",
    { params: listSchema },
    async (_, ctx) => {
      const owner = ctx.http?.req?.headers.get("x-omni-events-owner") ?? undefined;
      await runtime.effectRunner.runPromise(
        events.recordDiscovery(owner && OWNER_PATTERN.test(owner) ? owner : undefined),
      );
      return { events: EVENT_CATALOG };
    },
  );
  server.server.setRequestHandler(
    "events/subscribe",
    { params: subscribeSchema },
    async (params, ctx) => {
      const principal = principalFromHeaders(ctx.http?.req?.headers);
      try {
        return await runtime.effectRunner.runPromise(
          events.subscribe(params, principal),
        );
      } catch (error) {
        if (error instanceof EventSubscriptionError) throw rpcError(error);
        throw error;
      }
    },
  );
  server.server.setRequestHandler(
    "events/unsubscribe",
    { params: unsubscribeSchema },
    async (params, ctx) => {
      const principal = principalFromHeaders(ctx.http?.req?.headers);
      try {
        return await runtime.effectRunner.runPromise(
          events.unsubscribe(params, principal),
        );
      } catch (error) {
        if (error instanceof EventSubscriptionError) throw rpcError(error);
        throw error;
      }
    },
  );
}
