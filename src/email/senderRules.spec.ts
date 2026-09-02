import { Docstore } from "@micthiesen/mitools/docstore";
import { ManagedRuntime } from "effect";
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

const runtime = ManagedRuntime.make(Docstore.layerMemory);
const runEffect = runtime.runPromise.bind(runtime);

afterEach(async () => {
  await runEffect(EmailRuleEntity.deleteAll());
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
  it("returns undefined when no rule matches", async () => {
    await runEffect(
      upsertEmailRule({ pattern: "other.com", scope: "both", verdict: "block" }),
    );
    expect(
      await runEffect(getSenderRuleVerdict("orders@shop.com", "parcel")),
    ).toBeUndefined();
  });

  it("only applies rules whose scope covers the pipeline", async () => {
    await runEffect(
      upsertEmailRule({ pattern: "shop.com", scope: "parcel", verdict: "block" }),
    );
    expect(await runEffect(getSenderRuleVerdict("orders@shop.com", "parcel"))).toBe(
      "block",
    );
    expect(
      await runEffect(getSenderRuleVerdict("orders@shop.com", "calendar")),
    ).toBeUndefined();
  });

  it('applies "both"-scoped rules to either pipeline', async () => {
    await runEffect(
      upsertEmailRule({ pattern: "shop.com", scope: "both", verdict: "allow" }),
    );
    expect(await runEffect(getSenderRuleVerdict("orders@shop.com", "parcel"))).toBe(
      "allow",
    );
    expect(await runEffect(getSenderRuleVerdict("orders@shop.com", "calendar"))).toBe(
      "allow",
    );
  });

  it("block beats allow when multiple rules match", async () => {
    await runEffect(
      upsertEmailRule({ pattern: "shop.com", scope: "parcel", verdict: "allow" }),
    );
    await runEffect(
      upsertEmailRule({
        pattern: "orders@shop.com",
        scope: "parcel",
        verdict: "block",
      }),
    );
    expect(await runEffect(getSenderRuleVerdict("orders@shop.com", "parcel"))).toBe(
      "block",
    );
    expect(
      (await runEffect(findSenderRule("orders@shop.com", "parcel")))?.pattern,
    ).toBe("orders@shop.com");
  });
});

describe("upsertEmailRule / deleteEmailRule", () => {
  it("normalizes the pattern and derives the ruleId", async () => {
    const row = await runEffect(
      upsertEmailRule({
        pattern: "  Orders@Shop.COM ",
        scope: "parcel",
        verdict: "block",
      }),
    );
    expect(row.pattern).toBe("orders@shop.com");
    expect(row.ruleId).toBe("parcel:orders@shop.com");
  });

  it("rejects an empty pattern", async () => {
    await expect(
      runEffect(upsertEmailRule({ pattern: "  ", scope: "both", verdict: "block" })),
    ).rejects.toThrow();
  });

  it("overwrites the verdict on re-upsert and keeps a single row", async () => {
    await runEffect(
      upsertEmailRule({ pattern: "shop.com", scope: "both", verdict: "block" }),
    );
    const updated = await runEffect(
      upsertEmailRule({
        pattern: "shop.com",
        scope: "both",
        verdict: "allow",
      }),
    );
    expect(updated.verdict).toBe("allow");
    expect(await runEffect(listEmailRules())).toHaveLength(1);
  });

  it("deletes by ruleId and reports whether the rule existed", async () => {
    const row = await runEffect(
      upsertEmailRule({
        pattern: "shop.com",
        scope: "both",
        verdict: "block",
      }),
    );
    expect(await runEffect(deleteEmailRule(row.ruleId))).toBe(true);
    expect(await runEffect(deleteEmailRule(row.ruleId))).toBe(false);
    expect(await runEffect(listEmailRules())).toHaveLength(0);
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
  it('a "both" add folds in and removes existing single-scope rules', async () => {
    await runEffect(
      upsertEmailRule({ pattern: "@x.com", scope: "parcel", verdict: "block" }),
    );
    const result = await runEffect(
      upsertEmailRuleChecked({
        pattern: "@x.com",
        scope: "both",
        verdict: "allow",
      }),
    );

    expect(result.merged).toBe(true);
    const rules = await runEffect(listEmailRules());
    expect(rules).toHaveLength(1);
    expect(rules[0].scope).toBe("both");
    expect(rules[0].verdict).toBe("allow");
    // The contradictory parcel block is gone, so the sender is truly allowed.
    expect(await runEffect(getSenderRuleVerdict("a@x.com", "parcel"))).toBe("allow");
  });

  it("reports an exact same-verdict duplicate as already existing", async () => {
    await runEffect(
      upsertEmailRuleChecked({ pattern: "@x.com", scope: "parcel", verdict: "block" }),
    );
    const again = await runEffect(
      upsertEmailRuleChecked({
        pattern: "@x.com",
        scope: "parcel",
        verdict: "block",
      }),
    );
    expect(again.alreadyExists).toBe(true);
    expect(await runEffect(listEmailRules())).toHaveLength(1);
  });
});
