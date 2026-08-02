import { Injector } from "@micthiesen/mitools/config";
import { LogLevel } from "@micthiesen/mitools/logging";
import { afterEach, describe, expect, it } from "vitest";
import {
  deleteEmailRule,
  EmailRuleEntity,
  findSenderRule,
  getSenderRuleVerdict,
  listEmailRules,
  matchesSenderPattern,
  normalizeRulePattern,
  upsertEmailRule,
  upsertEmailRuleChecked,
} from "./senderRules.js";

Injector.configure({
  config: {
    LOG_LEVEL: LogLevel.INFO,
    PUSHOVER_TOKEN: "fake-token",
    PUSHOVER_USER: "fake-user",
    DOCKERIZED: false,
    DB_NAME: "senderrules.spec.db",
  },
});

afterEach(() => {
  EmailRuleEntity.deleteAll();
});

describe("matchesSenderPattern", () => {
  it("matches a full address exactly", () => {
    expect(matchesSenderPattern("orders@shop.com", "orders@shop.com")).toBe(true);
  });

  it("does not match a different mailbox for a full-address pattern", () => {
    expect(matchesSenderPattern("noreply@shop.com", "orders@shop.com")).toBe(false);
  });

  it('matches every mailbox for an "@domain" style pattern', () => {
    expect(matchesSenderPattern("orders@shop.com", "@shop.com")).toBe(true);
    expect(matchesSenderPattern("noreply@shop.com", "@shop.com")).toBe(true);
  });

  it('matches subdomains for an "@domain" style pattern', () => {
    expect(matchesSenderPattern("noreply@mail.shop.com", "@shop.com")).toBe(true);
    expect(matchesSenderPattern("a@deep.mail.shop.com", "@shop.com")).toBe(true);
    expect(matchesSenderPattern("a@notshop.com", "@shop.com")).toBe(false);
  });

  it("matches a bare domain against the sender's domain", () => {
    expect(matchesSenderPattern("orders@shop.com", "shop.com")).toBe(true);
  });

  it("matches subdomains for a bare-domain pattern", () => {
    expect(matchesSenderPattern("noreply@mail.shop.com", "shop.com")).toBe(true);
  });

  it("does not match a lookalike domain for a bare-domain pattern", () => {
    expect(matchesSenderPattern("orders@notshop.com", "shop.com")).toBe(false);
  });

  it("tolerates the display-name angle-bracket form", () => {
    expect(matchesSenderPattern('"shop" <orders@shop.com>', "shop.com")).toBe(true);
    expect(matchesSenderPattern('"shop" <orders@shop.com>', "orders@shop.com")).toBe(
      true,
    );
  });
});

describe("getSenderRuleVerdict", () => {
  it("returns undefined when no rule matches", () => {
    upsertEmailRule({ pattern: "other.com", scope: "both", verdict: "block" });
    expect(getSenderRuleVerdict("orders@shop.com", "parcel")).toBeUndefined();
  });

  it("only applies rules whose scope covers the pipeline", () => {
    upsertEmailRule({ pattern: "shop.com", scope: "parcel", verdict: "block" });
    expect(getSenderRuleVerdict("orders@shop.com", "parcel")).toBe("block");
    expect(getSenderRuleVerdict("orders@shop.com", "calendar")).toBeUndefined();
  });

  it('applies "both"-scoped rules to either pipeline', () => {
    upsertEmailRule({ pattern: "shop.com", scope: "both", verdict: "allow" });
    expect(getSenderRuleVerdict("orders@shop.com", "parcel")).toBe("allow");
    expect(getSenderRuleVerdict("orders@shop.com", "calendar")).toBe("allow");
  });

  it("block beats allow when multiple rules match", () => {
    upsertEmailRule({ pattern: "shop.com", scope: "parcel", verdict: "allow" });
    upsertEmailRule({
      pattern: "orders@shop.com",
      scope: "parcel",
      verdict: "block",
    });
    expect(getSenderRuleVerdict("orders@shop.com", "parcel")).toBe("block");
    expect(findSenderRule("orders@shop.com", "parcel")?.pattern).toBe(
      "orders@shop.com",
    );
  });
});

describe("upsertEmailRule / deleteEmailRule", () => {
  it("normalizes the pattern and derives the ruleId", () => {
    const row = upsertEmailRule({
      pattern: "  Orders@Shop.COM ",
      scope: "parcel",
      verdict: "block",
    });
    expect(row.pattern).toBe("orders@shop.com");
    expect(row.ruleId).toBe("parcel:orders@shop.com");
  });

  it("rejects an empty pattern", () => {
    expect(() =>
      upsertEmailRule({ pattern: "  ", scope: "both", verdict: "block" }),
    ).toThrow();
  });

  it("overwrites the verdict on re-upsert and keeps a single row", () => {
    upsertEmailRule({ pattern: "shop.com", scope: "both", verdict: "block" });
    const updated = upsertEmailRule({
      pattern: "shop.com",
      scope: "both",
      verdict: "allow",
    });
    expect(updated.verdict).toBe("allow");
    expect(listEmailRules()).toHaveLength(1);
  });

  it("deletes by ruleId and reports whether the rule existed", () => {
    const row = upsertEmailRule({
      pattern: "shop.com",
      scope: "both",
      verdict: "block",
    });
    expect(deleteEmailRule(row.ruleId)).toBe(true);
    expect(deleteEmailRule(row.ruleId)).toBe(false);
    expect(listEmailRules()).toHaveLength(0);
  });
});

describe("normalizeRulePattern", () => {
  it("prefixes a bare domain with @", () => {
    expect(normalizeRulePattern("plex.tv")).toBe("@plex.tv");
  });

  it("keeps an @domain pattern (collapsing casing and extra @)", () => {
    expect(normalizeRulePattern("@Plex.TV")).toBe("@plex.tv");
    expect(normalizeRulePattern("@@plex.tv")).toBe("@plex.tv");
  });

  it("keeps a full address as an exact-address rule", () => {
    expect(normalizeRulePattern("Orders@DoorDash.com")).toBe("orders@doordash.com");
  });

  it("strips a display-name wrapper", () => {
    expect(normalizeRulePattern("Plex <no-reply@plex.tv>")).toBe("no-reply@plex.tv");
  });
});

describe("upsertEmailRuleChecked", () => {
  it('a "both" add folds in and removes existing single-scope rules', () => {
    upsertEmailRule({ pattern: "@x.com", scope: "parcel", verdict: "block" });
    const result = upsertEmailRuleChecked({
      pattern: "@x.com",
      scope: "both",
      verdict: "allow",
    });

    expect(result.merged).toBe(true);
    const rules = listEmailRules();
    expect(rules).toHaveLength(1);
    expect(rules[0].scope).toBe("both");
    expect(rules[0].verdict).toBe("allow");
    // The contradictory parcel block is gone, so the sender is truly allowed.
    expect(getSenderRuleVerdict("a@x.com", "parcel")).toBe("allow");
  });

  it("reports an exact same-verdict duplicate as already existing", () => {
    upsertEmailRuleChecked({ pattern: "@x.com", scope: "parcel", verdict: "block" });
    const again = upsertEmailRuleChecked({
      pattern: "@x.com",
      scope: "parcel",
      verdict: "block",
    });
    expect(again.alreadyExists).toBe(true);
    expect(listEmailRules()).toHaveLength(1);
  });
});
