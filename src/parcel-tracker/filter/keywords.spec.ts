import { Injector } from "@micthiesen/mitools/config";
import type { Logger } from "@micthiesen/mitools/logging";
import { LogLevel } from "@micthiesen/mitools/logging";
import { afterEach, describe, expect, it, vi } from "vitest";
import { EmailRuleEntity, upsertEmailRule } from "../../jmap/senderRules.js";
import { EmailTriageService, type TriageVerdict } from "../../jmap/triage.js";
import config from "../../utils/config.js";
import { filterTrackingCandidate, isAliexpressOrderStatus } from "./keywords.js";

Injector.configure({
  config: {
    LOG_LEVEL: LogLevel.INFO,
    PUSHOVER_TOKEN: "fake-token",
    PUSHOVER_USER: "fake-user",
    DOCKERIZED: false,
    DB_NAME: "parcel-filter.spec.db",
  },
});

const mockLogger = {
  debug: vi.fn(),
  info: vi.fn(),
  warn: vi.fn(),
  error: vi.fn(),
  extend: vi.fn(),
} as unknown as Logger;

let nextId = 0;
const make = (from: string, subject: string, textBody = "") => ({
  id: `email-${nextId++}`,
  from,
  subject,
  textBody,
});

/** Triage stub with a fixed verdict; asserts on whether it was consulted. */
function stubTriage(verdict: TriageVerdict) {
  const classifyFn = vi.fn(async () => verdict);
  return { triage: new EmailTriageService(mockLogger, classifyFn), classifyFn };
}

/** Triage stub whose model is down. */
function downTriage() {
  const classifyFn = vi.fn(async (): Promise<TriageVerdict> => {
    throw new Error("model down");
  });
  return { triage: new EmailTriageService(mockLogger, classifyFn), classifyFn };
}

const parcelYes: TriageVerdict = { parcel: true, calendar: false, reason: "tracking" };
const parcelNo: TriageVerdict = {
  parcel: false,
  calendar: false,
  reason: "no shipment",
};

afterEach(() => {
  EmailRuleEntity.deleteAll();
});

describe("filterTrackingCandidate — sender rules", () => {
  it("a block rule beats even a carrier sender", async () => {
    upsertEmailRule({ pattern: "ups.com", scope: "parcel", verdict: "block" });
    const result = await filterTrackingCandidate(
      make("noreply@ups.com", "Delivery update"),
      mockLogger,
      stubTriage(parcelYes).triage,
    );
    expect(result).toEqual({ pass: false, reason: "blocked by rule ups.com" });
  });

  it("an allow rule passes without consulting triage", async () => {
    upsertEmailRule({ pattern: "somestore.com", scope: "both", verdict: "allow" });
    const { triage, classifyFn } = downTriage();
    const result = await filterTrackingCandidate(
      make("orders@somestore.com", "Anything at all"),
      mockLogger,
      triage,
    );
    expect(result).toEqual({
      pass: true,
      reason: "allowed by rule somestore.com",
      admitTier: "rule",
    });
    expect(classifyFn).not.toHaveBeenCalled();
  });

  it("calendar-scoped rules do not affect the parcel filter", async () => {
    upsertEmailRule({ pattern: "ups.com", scope: "calendar", verdict: "block" });
    const result = await filterTrackingCandidate(
      make("noreply@ups.com", "Delivery update"),
      mockLogger,
      downTriage().triage,
    );
    expect(result).toEqual({
      pass: true,
      reason: "carrier sender",
      admitTier: "builtin",
    });
  });

  it("an allow rule overrides the built-in blacklist", async () => {
    upsertEmailRule({ pattern: "npmjs.com", scope: "parcel", verdict: "allow" });
    const result = await filterTrackingCandidate(
      make("support@npmjs.com", "Successfully published a package"),
      mockLogger,
      downTriage().triage,
    );
    expect(result).toEqual({
      pass: true,
      reason: "allowed by rule npmjs.com",
      admitTier: "rule",
    });
  });
});

describe("filterTrackingCandidate — static blacklist", () => {
  it("rejects blacklisted amazon senders and subdomains", async () => {
    for (const from of ["shipment-tracking@amazon.com", "ship-confirm@amazon.co.uk"]) {
      const result = await filterTrackingCandidate(
        make(from, "Your order has shipped"),
        mockLogger,
        stubTriage(parcelYes).triage,
      );
      expect(result.pass, `Expected ${from} to be rejected`).toBe(false);
      expect(result.reason).toBe("blacklisted sender");
    }
  });

  it("rejects npm registry mail", async () => {
    const result = await filterTrackingCandidate(
      make("support@npmjs.com", "Successfully published your-package@1.0.0"),
      mockLogger,
      stubTriage(parcelYes).triage,
    );
    expect(result).toEqual({ pass: false, reason: "blacklisted sender" });
  });

  it("rejects the user's own outgoing address when configured", async () => {
    const original = config.FASTMAIL_USERNAME;
    config.FASTMAIL_USERNAME = "michael@example.com";
    try {
      const result = await filterTrackingCandidate(
        make("michael@example.com", "Fwd: your package shipped"),
        mockLogger,
        stubTriage(parcelYes).triage,
      );
      expect(result).toEqual({ pass: false, reason: "blacklisted sender" });
    } finally {
      config.FASTMAIL_USERNAME = original;
    }
  });

  it("rejects food-delivery senders even with tracking keywords", async () => {
    const result = await filterTrackingCandidate(
      make("noreply@uber.com", "Your delivery is in transit"),
      mockLogger,
      stubTriage(parcelYes).triage,
    );
    expect(result).toEqual({ pass: false, reason: "blacklisted sender" });
  });
});

describe("isAliexpressOrderStatus", () => {
  it.each([
    ["Order 8196234512: view details", true],
    ["Your order is awaiting confirmation", true],
    ["Order shipped! See what's on the way", true],
    ["Order confirmed — thanks for shopping", true],
    ["Delivery update for your order", true],
    ["Awaiting Payment: complete your purchase", true],
    ["Package from your order is ready to ship", false],
  ])("aliexpress subject %j → %s", (subject, expected) => {
    expect(isAliexpressOrderStatus("transaction@notice.aliexpress.com", subject)).toBe(
      expected,
    );
  });

  it("never matches non-aliexpress senders", () => {
    expect(isAliexpressOrderStatus("orders@shop.com", "Order shipped")).toBe(false);
  });
});

describe("filterTrackingCandidate — aliexpress", () => {
  it("skips order-status emails without consulting triage", async () => {
    const { triage, classifyFn } = stubTriage(parcelYes);
    const result = await filterTrackingCandidate(
      make("transaction@notice.aliexpress.com", "Order 8196234512: Awaiting delivery"),
      mockLogger,
      triage,
    );
    expect(result).toEqual({ pass: false, reason: "aliexpress order-status" });
    expect(classifyFn).not.toHaveBeenCalled();
  });

  it("still lets ready-to-ship emails through to triage", async () => {
    const result = await filterTrackingCandidate(
      make(
        "transaction@notice.aliexpress.com",
        "Package from your order is ready to ship",
      ),
      mockLogger,
      stubTriage(parcelYes).triage,
    );
    expect(result).toEqual({
      pass: true,
      reason: "triage: tracking",
      admitTier: "triage",
    });
  });
});

describe("filterTrackingCandidate — carrier senders", () => {
  it("passes carrier domains without consulting triage", async () => {
    const { triage, classifyFn } = downTriage();
    const result = await filterTrackingCandidate(
      make("noreply@FedEx.com", "Update"),
      mockLogger,
      triage,
    );
    expect(result).toEqual({
      pass: true,
      reason: "carrier sender",
      admitTier: "builtin",
    });
    expect(classifyFn).not.toHaveBeenCalled();
  });
});

describe("filterTrackingCandidate — triage", () => {
  it("passes when triage says parcel", async () => {
    const result = await filterTrackingCandidate(
      make("orders@somestore.com", "Order update"),
      mockLogger,
      stubTriage(parcelYes).triage,
    );
    expect(result).toEqual({
      pass: true,
      reason: "triage: tracking",
      admitTier: "triage",
    });
  });

  it("fails when triage says no, even with tracking keywords present", async () => {
    const result = await filterTrackingCandidate(
      make("orders@somestore.com", "Your order has shipped!"),
      mockLogger,
      stubTriage(parcelNo).triage,
    );
    expect(result).toEqual({ pass: false, reason: "triage: no shipment" });
  });
});

describe("filterTrackingCandidate — keyword fallback when triage is down", () => {
  it("matches tracking keywords in the subject", async () => {
    const result = await filterTrackingCandidate(
      make("orders@somestore.com", "Your order has shipped!"),
      mockLogger,
      downTriage().triage,
    );
    expect(result).toEqual({
      pass: true,
      reason: 'keyword "shipped" (triage unavailable)',
      admitTier: "keyword-fallback",
    });
  });

  it("matches tracking keywords in the body, case-insensitively", async () => {
    const result = await filterTrackingCandidate(
      make("orders@somestore.com", "Order confirmation", "Your TRACKING number is X"),
      mockLogger,
      downTriage().triage,
    );
    expect(result.pass).toBe(true);
  });

  it("rejects unrelated emails", async () => {
    const result = await filterTrackingCandidate(
      make("hello@example.com", "Weekly digest", "Here are this week's top stories."),
      mockLogger,
      downTriage().triage,
    );
    expect(result).toEqual({
      pass: false,
      reason: "no keyword match (triage unavailable)",
    });
  });
});
