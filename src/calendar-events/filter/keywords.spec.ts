import { Injector } from "@micthiesen/mitools/config";
import type { Logger } from "@micthiesen/mitools/logging";
import { LogLevel } from "@micthiesen/mitools/logging";
import { afterEach, describe, expect, it, vi } from "vitest";
import { EmailRuleEntity, upsertEmailRule } from "../../jmap/senderRules.js";
import { EmailTriageService, type TriageVerdict } from "../../jmap/triage.js";
import config from "../../utils/config.js";
import { filterCalendarCandidate } from "./keywords.js";

Injector.configure({
  config: {
    LOG_LEVEL: LogLevel.INFO,
    PUSHOVER_TOKEN: "fake-token",
    PUSHOVER_USER: "fake-user",
    DOCKERIZED: false,
    DB_NAME: "calendar-filter.spec.db",
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

function stubTriage(verdict: TriageVerdict) {
  const classifyFn = vi.fn(async () => verdict);
  return { triage: new EmailTriageService(mockLogger, classifyFn), classifyFn };
}

function downTriage() {
  const classifyFn = vi.fn(async (): Promise<TriageVerdict> => {
    throw new Error("model down");
  });
  return { triage: new EmailTriageService(mockLogger, classifyFn), classifyFn };
}

const calendarYes: TriageVerdict = {
  parcel: false,
  calendar: true,
  reason: "upcoming appointment",
};
const calendarNo: TriageVerdict = {
  parcel: false,
  calendar: false,
  reason: "not an event",
};

afterEach(() => {
  EmailRuleEntity.deleteAll();
});

describe("filterCalendarCandidate — sender rules", () => {
  it("a block rule beats even a known auto-pass sender", async () => {
    upsertEmailRule({ pattern: "eventbrite.com", scope: "calendar", verdict: "block" });
    const result = await filterCalendarCandidate(
      make("noreply@eventbrite.com", "Your event is coming up"),
      stubTriage(calendarYes).triage,
    );
    expect(result).toEqual({ pass: false, reason: "blocked by rule eventbrite.com" });
  });

  it("an allow rule passes without consulting triage", async () => {
    upsertEmailRule({ pattern: "clinic.example", scope: "both", verdict: "allow" });
    const { triage, classifyFn } = downTriage();
    const result = await filterCalendarCandidate(
      make("frontdesk@clinic.example", "Anything at all"),
      triage,
    );
    expect(result).toEqual({ pass: true, reason: "allowed by rule clinic.example" });
    expect(classifyFn).not.toHaveBeenCalled();
  });

  it("parcel-scoped rules do not affect the calendar filter", async () => {
    upsertEmailRule({ pattern: "eventbrite.com", scope: "parcel", verdict: "block" });
    const result = await filterCalendarCandidate(
      make("noreply@eventbrite.com", "anything"),
      downTriage().triage,
    );
    expect(result).toEqual({ pass: true, reason: "known sender" });
  });
});

describe("filterCalendarCandidate — static blacklist", () => {
  it("rejects newsletters and social senders", async () => {
    const result = await filterCalendarCandidate(
      make("blockedandreported@substack.com", "Weekly Open Thread"),
      stubTriage(calendarYes).triage,
    );
    expect(result).toEqual({ pass: false, reason: "blacklisted sender" });
  });

  it("rejects food delivery senders even with calendar keywords present", async () => {
    const result = await filterCalendarCandidate(
      make("no-reply@doordash.com", "Order Confirmation for Michael from DashMart"),
      stubTriage(calendarYes).triage,
    );
    expect(result.pass).toBe(false);
  });

  it("rejects Steam, npm, and UPS shipment senders", async () => {
    for (const from of [
      "noreply@steampowered.com",
      "support@npmjs.com",
      "pkginfo@ups.com",
    ]) {
      const result = await filterCalendarCandidate(
        make(from, "Confirmation of your recent activity"),
        stubTriage(calendarYes).triage,
      );
      expect(result.pass, `Expected ${from} to be rejected`).toBe(false);
      expect(result.reason).toBe("blacklisted sender");
    }
  });

  it("rejects the user's own outgoing address when configured", async () => {
    const original = config.FASTMAIL_USERNAME;
    config.FASTMAIL_USERNAME = "michael@example.com";
    try {
      const result = await filterCalendarCandidate(
        make("michael@example.com", "Dinner reservation details"),
        stubTriage(calendarYes).triage,
      );
      expect(result).toEqual({ pass: false, reason: "blacklisted sender" });
    } finally {
      config.FASTMAIL_USERNAME = original;
    }
  });
});

describe("filterCalendarCandidate — auto-pass senders", () => {
  it("passes a known domain without consulting triage", async () => {
    const { triage, classifyFn } = downTriage();
    const result = await filterCalendarCandidate(
      make("noreply@eventbrite.com", "anything"),
      triage,
    );
    expect(result).toEqual({ pass: true, reason: "known sender" });
    expect(classifyFn).not.toHaveBeenCalled();
  });

  it("passes a transactional subdomain of a known domain", async () => {
    const result = await filterCalendarCandidate(
      make("noreply@reminder.eventbrite.com", "Just added! BCIMS New Year's Retreat"),
      downTriage().triage,
    );
    expect(result).toEqual({ pass: true, reason: "known sender" });
  });

  it("passes a known domain wrapped in a display-name angle-bracket form", async () => {
    const result = await filterCalendarCandidate(
      make('"Eventbrite Reminders" <noreply@reminder.eventbrite.com>', "anything"),
      downTriage().triage,
    );
    expect(result).toEqual({ pass: true, reason: "known sender" });
  });

  it("sends a lookalike domain to triage instead of auto-passing", async () => {
    const result = await filterCalendarCandidate(
      make("noreply@noteventbrite.com", "Updates to Our Privacy Policy"),
      stubTriage(calendarNo).triage,
    );
    expect(result).toEqual({ pass: false, reason: "triage: not an event" });
  });

  it("no longer auto-passes @google.com corporate mail", async () => {
    const result = await filterCalendarCandidate(
      make("googleaistudio-noreply@google.com", "[Reminder] Secure your API access"),
      stubTriage(calendarNo).triage,
    );
    expect(result).toEqual({ pass: false, reason: "triage: not an event" });
  });
});

describe("filterCalendarCandidate — triage", () => {
  it("passes when triage says calendar", async () => {
    const result = await filterCalendarCandidate(
      make("no-reply@cortico.health", "Your Dr. Hassan Salame Appointment"),
      stubTriage(calendarYes).triage,
    );
    expect(result).toEqual({ pass: true, reason: "triage: upcoming appointment" });
  });

  it("fails when triage says no, even with calendar keywords present", async () => {
    const result = await filterCalendarCandidate(
      make("service@intl.paypal.com", "Reminder: your subscription renews"),
      stubTriage(calendarNo).triage,
    );
    expect(result).toEqual({ pass: false, reason: "triage: not an event" });
  });
});

describe("filterCalendarCandidate — keyword fallback when triage is down", () => {
  it("passes real calendar subjects on keywords", async () => {
    const result = await filterCalendarCandidate(
      make("notifications@tribehome.com", "Reminder - Hard Surface Cleaning - Monday"),
      downTriage().triage,
    );
    expect(result).toEqual({
      pass: true,
      reason: 'keyword "reminder" (triage unavailable)',
    });
  });

  it('does not pass on an "All rights reserved" footer', async () => {
    const result = await filterCalendarCandidate(
      make(
        "service@intl.paypal.com",
        "Your way to pay with PayPal is set",
        "Thanks for setting up PayPal.\n© 2026 PayPal. All rights reserved.",
      ),
      downTriage().triage,
    );
    expect(result).toEqual({
      pass: false,
      reason: "no keyword match (triage unavailable)",
    });
  });

  it("does not pass a privacy-policy email that merely mentions a concert", async () => {
    const result = await filterCalendarCandidate(
      make(
        "no-reply@legal.spotify.com",
        "Updates to Our Privacy Policy",
        "We may process data when you attend a concert or live event.",
      ),
      downTriage().triage,
    );
    expect(result.pass).toBe(false);
  });
});
