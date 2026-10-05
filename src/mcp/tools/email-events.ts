import { Effect } from "effect";
import { z } from "zod";
import {
  EVENT_DELIVERY_FAILURES,
  EVENT_DELIVERY_WITHHOLDS,
  EVENT_REQUEST_METHODS,
} from "../events/persistence.js";
import type { McpRuntime } from "../runtime.js";
import { annotations, defineTool, emptyInputSchema } from "../tool.js";

const folderSchema = z.enum(["inbox", "archive"]);

const statusSchema = z.object({
  enabled: z.boolean(),
  checkedAt: z.string().nullable(),
  requests: z.array(
    z.object({
      at: z.string(),
      method: z.enum(EVENT_REQUEST_METHODS),
      owner: z.string(),
      folder: folderSchema.nullable(),
      callbackHost: z.string().nullable(),
      outcome: z.string(),
    }),
  ),
  subscriptionTotal: z.number(),
  subscriptions: z.array(
    z.object({
      id: z.string(),
      folder: folderSchema,
      owner: z.string(),
      callbackHost: z.string().nullable(),
      state: z.enum(["active", "expired", "stale_key"]),
      expiresAt: z.string(),
      verifiedAt: z.string(),
    }),
  ),
  deliveries: z.object({
    pending: z.number(),
    withheld: z.number(),
    delivered: z.number(),
    failed: z.number(),
    recent: z.array(
      z.object({
        eventId: z.string(),
        subscriptionId: z.string(),
        folder: folderSchema,
        status: z.enum(["pending", "delivered", "failed"]),
        attempts: z.number(),
        lastStatus: z.number().nullable(),
        lastError: z.string().nullable(),
        failure: z.enum(EVENT_DELIVERY_FAILURES).nullable(),
        withheld: z.enum(EVENT_DELIVERY_WITHHOLDS).nullable(),
        createdAt: z.string(),
        updatedAt: z.string(),
      }),
    ),
  }),
});

export function createEmailEventTools(runtime: McpRuntime) {
  return [
    defineTool({
      name: "email_events_status",
      title: "Check Email Event Delivery",
      description:
        "Report the email.received MCP Events lifecycle Omni has observed: recent events/list and subscription requests, subscriptions by callback host, and webhook delivery outcomes. Contains no secrets, tokens, callback paths, or message content.",
      inputSchema: emptyInputSchema,
      outputSchema: statusSchema,
      annotations: annotations(true, false, true, false),
      policy: { sideEffects: [], cost: "none", recommendedPolicy: "allow" },
      execute: () =>
        runtime.events
          ? runtime.events
              .status()
              .pipe(Effect.map((status) => ({ enabled: true, ...status })))
          : Effect.succeed({
              enabled: false,
              checkedAt: null,
              requests: [],
              subscriptionTotal: 0,
              subscriptions: [],
              deliveries: {
                pending: 0,
                withheld: 0,
                delivered: 0,
                failed: 0,
                recent: [],
              },
            }),
    }),
  ];
}
