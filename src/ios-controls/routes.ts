import { createHash, createHmac, timingSafeEqual } from "node:crypto";
import type { Logger } from "@micthiesen/mitools/logging";
import type { Hono } from "hono";
import { z } from "zod";
import { IOS_CONTROL_SLOT_COUNT } from "./liveSlots.js";
import type { IOSControlService } from "./service.js";

const registrationSchema = z.object({
  deviceId: z.string().trim().min(8).max(200),
  controls: z
    .array(
      z.object({
        controlId: z.string().trim().min(1).max(500),
        slot: z.number().int().min(1).max(IOS_CONTROL_SLOT_COUNT),
        pushToken: z.string().regex(/^[0-9a-fA-F]{32,512}$/),
        environment: z.enum(["sandbox", "production"]),
      }),
    )
    .max(32),
});

const AUTH_WINDOW_SECONDS = 300;

function safeEqual(supplied: string, expected: string): boolean {
  const a = Buffer.from(supplied);
  const b = Buffer.from(expected);
  return a.length === b.length && timingSafeEqual(a, b);
}

function pruneNonces(nonces: Map<string, number>, now: number): void {
  for (const [nonce, expiresAt] of nonces) {
    if (expiresAt <= now) nonces.delete(nonce);
  }
}

export function registerIOSControlRoutes(
  app: Hono,
  service: IOSControlService,
  authToken: string | undefined,
  parentLogger: Logger,
): void {
  const logger = parentLogger.extend("IOSControlRoutes");
  const usedNonces = new Map<string, number>();
  app.use("/api/ios-controls/*", async (c, next) => {
    if (!authToken) {
      return c.json({ error: "iOS controls are not configured" }, 503);
    }
    const timestamp = Number(c.req.header("X-Omni-Timestamp"));
    const nonce = c.req.header("X-Omni-Nonce") ?? "";
    const supplied = c.req.header("Authorization")?.replace(/^Omni-HMAC\s+/i, "") ?? "";
    const now = Math.floor(Date.now() / 1_000);
    pruneNonces(usedNonces, now);
    if (
      !Number.isInteger(timestamp) ||
      Math.abs(now - timestamp) > AUTH_WINDOW_SECONDS ||
      !/^[0-9a-f-]{36}$/i.test(nonce) ||
      usedNonces.has(nonce)
    ) {
      return c.json({ error: "Unauthorized" }, 401);
    }
    const body = Buffer.from(await c.req.raw.clone().arrayBuffer());
    const bodyHash = createHash("sha256").update(body).digest("hex");
    const path = new URL(c.req.url).pathname;
    const canonical = `${timestamp}\n${nonce}\n${c.req.method}\n${path}\n${bodyHash}`;
    const expected = createHmac("sha256", authToken).update(canonical).digest("hex");
    if (!safeEqual(supplied, expected)) return c.json({ error: "Unauthorized" }, 401);
    usedNonces.set(nonce, now + AUTH_WINDOW_SECONDS);
    await next();
  });

  app.get("/api/ios-controls/slots/:slot", (c) => {
    const slot = service.getSlot(Number(c.req.param("slot")));
    if (!slot) return c.json({ error: "Slot must be an integer from 1 to 4" }, 400);
    c.header("Cache-Control", "no-store");
    return c.json(slot);
  });

  app.get("/api/ios-controls/diagnostics", (c) => {
    c.header("Cache-Control", "no-store");
    return c.json(service.diagnostics());
  });

  app.put("/api/ios-controls/registrations", async (c) => {
    const parsed = registrationSchema.safeParse(await c.req.json().catch(() => null));
    if (!parsed.success) {
      return c.json(
        { error: "Invalid control registration", issues: parsed.error.issues },
        400,
      );
    }
    await service.registerDevice(parsed.data.deviceId, parsed.data.controls);
    logger.info(`Registered ${parsed.data.controls.length} control(s) for one device`);
    return c.json({ registered: parsed.data.controls.length });
  });
}
