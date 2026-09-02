import { Entity } from "@micthiesen/mitools/entities";
import { Effect, Option } from "effect";

export type EmailRuleScope = "parcel" | "calendar" | "both";
export type EmailRuleVerdict = "block" | "allow";

export type EmailRuleData = {
  /** `${scope}:${pattern}` */
  ruleId: string;
  /** Lowercase full address ("x@y.com") or bare domain ("y.com"). */
  pattern: string;
  scope: EmailRuleScope;
  verdict: EmailRuleVerdict;
  createdAt: number;
};

export const EmailRuleEntity = new Entity<EmailRuleData, ["ruleId"]>(
  "email-sender-rule",
  ["ruleId"],
);

/**
 * Address portion of an already-lowercased `from`. Production senders are bare
 * addresses, but this also tolerates the display-name form `Name <user@host>`.
 */
function senderAddress(fromLower: string): string {
  const bracketed = fromLower.match(/<([^>]*)>/);
  return (bracketed ? bracketed[1] : fromLower).trim();
}

/** Domain portion of an already-lowercased sender. */
function senderDomain(fromLower: string): string {
  const addr = senderAddress(fromLower);
  const at = addr.lastIndexOf("@");
  return at >= 0 ? addr.slice(at + 1).trim() : addr;
}

/**
 * Pure: does a sender match a user rule pattern?
 * - Pattern containing "@": matches when the address equals it or ends with it
 *   (so "@y.com"-style prefixes cover every mailbox at that host).
 * - Bare domain: matches when the sender's domain equals it or is a subdomain.
 */
export function matchesSenderPattern(fromLower: string, pattern: string): boolean {
  // Domain rule ("@host"): match the host and any subdomain of it.
  if (pattern.startsWith("@")) {
    const domain = pattern.slice(1);
    const senderDom = senderDomain(fromLower);
    return senderDom === domain || senderDom.endsWith(`.${domain}`);
  }
  // Full-address rule ("local@host"): exact address match.
  if (pattern.includes("@")) {
    return senderAddress(fromLower) === pattern;
  }
  // Bare domain (legacy; new rules normalize to "@host"): host + subdomains.
  const domain = senderDomain(fromLower);
  return domain === pattern || domain.endsWith(`.${pattern}`);
}

/**
 * Canonical form for a user rule pattern. The block UI and rule form send raw
 * input; we store one consistent shape:
 * - display-name wrappers are stripped ("Name <x@y.com>" → "x@y.com"),
 * - a bare domain becomes a domain rule ("plex.tv" → "@plex.tv"),
 * - an already-"@host" domain rule is kept (collapsing "@@" and casing),
 * - a full "local@host" address is kept as an exact-address rule.
 * Domain rules ("@host") match the host AND its subdomains (see
 * `matchesSenderPattern`), which is what we want ~always.
 */
export function normalizeRulePattern(input: string): string {
  let p = input.trim().toLowerCase();
  if (!p) return p;
  const bracket = p.match(/<([^>]*)>/);
  if (bracket?.[1]) p = bracket[1].trim();
  if (p.startsWith("@")) return `@${p.replace(/^@+/, "")}`;
  if (p.includes("@")) return p;
  return `@${p}`;
}

/**
 * The rule that decides this sender for a pipeline (scope matches the pipeline
 * or is "both"). When multiple rules match, block beats allow.
 */
export const findSenderRule = Effect.fn("EmailSenderRule.find")(function* (
  from: string,
  pipeline: "parcel" | "calendar",
) {
  const fromLower = from.toLowerCase();
  const matches = (yield* EmailRuleEntity.getAll()).filter(
    (rule) =>
      (rule.scope === pipeline || rule.scope === "both") &&
      matchesSenderPattern(fromLower, rule.pattern),
  );
  return matches.find((rule) => rule.verdict === "block") ?? matches[0];
});

export const getSenderRuleVerdict = Effect.fn("EmailSenderRule.verdict")(function* (
  from: string,
  pipeline: "parcel" | "calendar",
) {
  return (yield* findSenderRule(from, pipeline))?.verdict;
});

export const listEmailRules = Effect.fn("EmailSenderRule.list")(function* () {
  return (yield* EmailRuleEntity.getAll()).sort((a, b) => b.createdAt - a.createdAt);
});

export const upsertEmailRule = Effect.fn("EmailSenderRule.upsert")(function* (entry: {
  pattern: string;
  scope: EmailRuleScope;
  verdict: EmailRuleVerdict;
}) {
  const pattern = entry.pattern.trim().toLowerCase();
  if (!pattern) throw new Error("Sender rule pattern must be non-empty");
  const ruleId = `${entry.scope}:${pattern}`;
  const existing = Option.getOrUndefined(yield* EmailRuleEntity.get({ ruleId }));
  const row: EmailRuleData = {
    ruleId,
    pattern,
    scope: entry.scope,
    verdict: entry.verdict,
    createdAt: existing?.createdAt ?? Date.now(),
  };
  yield* EmailRuleEntity.upsert(row);
  return row;
});

export const deleteEmailRule = Effect.fn("EmailSenderRule.delete")(function* (
  ruleId: string,
) {
  return yield* EmailRuleEntity.delete({ ruleId });
});

/**
 * Existing USER-rule coverage for a pattern (exact match only — this reports
 * what's already on the books, not what would match a sender via
 * `matchesSenderPattern`'s prefix/subdomain rules). A "both"-scoped rule
 * covers parcel and calendar simultaneously.
 */
export type EmailRuleCoverage = {
  pattern: string;
  /** Scopes with an existing block rule (direct match or via a "both" row). */
  blockedScopes: Set<"parcel" | "calendar">;
  /** Scopes with an existing allow rule (direct match or via a "both" row). */
  allowedScopes: Set<"parcel" | "calendar">;
  hasBothRule: boolean;
  /** The exact-pattern rows backing the coverage above. */
  matches: EmailRuleData[];
};

export const getSenderRuleCoverage = Effect.fn("EmailSenderRule.coverage")(function* (
  pattern: string,
) {
  const normalized = pattern.trim().toLowerCase();
  const matches = (yield* EmailRuleEntity.getAll()).filter(
    (rule) => rule.pattern === normalized,
  );
  const blockedScopes = new Set<"parcel" | "calendar">();
  const allowedScopes = new Set<"parcel" | "calendar">();
  let hasBothRule = false;
  for (const rule of matches) {
    const scopes: Array<"parcel" | "calendar"> =
      rule.scope === "both" ? ["parcel", "calendar"] : [rule.scope];
    if (rule.scope === "both") hasBothRule = true;
    const target = rule.verdict === "block" ? blockedScopes : allowedScopes;
    for (const scope of scopes) target.add(scope);
  }
  return { pattern: normalized, blockedScopes, allowedScopes, hasBothRule, matches };
});

/**
 * Pure decision for adding a rule, without writing anything. Determines
 * whether the add is redundant (`noop-exists`), should merge two
 * single-scope rules of the same verdict into one "both" row
 * (`upgrade-to-both`), or is a plain new rule (`create`).
 */
export type RuleAddPlan =
  | { action: "create"; row: EmailRuleData }
  | { action: "upgrade-to-both"; delete: EmailRuleData[]; row: EmailRuleData }
  | { action: "noop-exists"; existing: EmailRuleData };

export const planRuleAdd = Effect.fn("EmailSenderRule.planAdd")(function* (
  pattern: string,
  scope: EmailRuleScope,
  verdict: EmailRuleVerdict,
) {
  const normalized = pattern.trim().toLowerCase();
  if (!normalized) throw new Error("Sender rule pattern must be non-empty");
  const coverage = yield* getSenderRuleCoverage(normalized);

  // A "both" rule is authoritative for a pattern: it supersedes any existing
  // single-scope rows, so fold them in (delete) rather than leaving a
  // contradictory parcel/calendar rule that "block beats allow" would honor.
  if (scope === "both") {
    const singles = coverage.matches.filter(
      (rule) => rule.scope === "parcel" || rule.scope === "calendar",
    );
    const existingBothRow = coverage.matches.find((rule) => rule.scope === "both");
    if (existingBothRow?.verdict === verdict && singles.length === 0) {
      return { action: "noop-exists", existing: existingBothRow } as const;
    }
    const createdAt =
      existingBothRow?.verdict === verdict
        ? existingBothRow.createdAt
        : Math.min(Date.now(), ...coverage.matches.map((r) => r.createdAt));
    const row: EmailRuleData = {
      ruleId: `both:${normalized}`,
      pattern: normalized,
      scope: "both",
      verdict,
      createdAt,
    };
    // The existing `both:` row (if any) is replaced by the upsert at the same
    // ruleId; only the single-scope rows need explicit deletion.
    if (singles.length > 0) {
      return { action: "upgrade-to-both", delete: singles, row } as const;
    }
    return { action: "create", row } as const;
  }

  // Already fully covered by an existing user rule of the same verdict?
  // A "both" row covers every target scope (parcel, calendar, or both).
  const existingBoth = coverage.matches.find(
    (rule) => rule.scope === "both" && rule.verdict === verdict,
  );
  if (existingBoth) {
    return { action: "noop-exists", existing: existingBoth } as const;
  }
  const existingExact = coverage.matches.find(
    (rule) => rule.scope === scope && rule.verdict === verdict,
  );
  if (existingExact) {
    return { action: "noop-exists", existing: existingExact } as const;
  }

  // Adding a single-scope rule while the opposite single scope already has
  // the same verdict → merge into one "both" row.
  const oppositeScope = scope === "parcel" ? "calendar" : "parcel";
  const opposite = coverage.matches.find(
    (rule) => rule.scope === oppositeScope && rule.verdict === verdict,
  );
  if (opposite) {
    const newRow: EmailRuleData = {
      ruleId: `both:${normalized}`,
      pattern: normalized,
      scope: "both",
      verdict,
      createdAt: opposite.createdAt,
    };
    return { action: "upgrade-to-both", delete: [opposite], row: newRow } as const;
  }

  return {
    action: "create",
    row: {
      ruleId: `${scope}:${normalized}`,
      pattern: normalized,
      scope,
      verdict,
      createdAt: Date.now(),
    },
  } as const;
});

export type UpsertEmailRuleCheckedResult = {
  rule: EmailRuleData;
  /** True when this add merged two single-scope rules into one "both" row. */
  merged: boolean;
  /** True when an identical (or broader) rule already existed; nothing changed. */
  alreadyExists: boolean;
};

/**
 * Add-a-rule entry point that rejects redundant duplicates and normalizes a
 * same-verdict parcel+calendar pair into a single "both" rule. Use this
 * instead of `upsertEmailRule` for user-facing rule creation.
 */
export const upsertEmailRuleChecked = Effect.fn("EmailSenderRule.upsertChecked")(
  function* (input: {
    pattern: string;
    scope: EmailRuleScope;
    verdict: EmailRuleVerdict;
  }) {
    const plan = yield* planRuleAdd(input.pattern, input.scope, input.verdict);
    switch (plan.action) {
      case "noop-exists":
        return { rule: plan.existing, merged: false, alreadyExists: true };
      case "upgrade-to-both":
        // Write the superseding "both" row first so a crash mid-op can never lose
        // coverage — a leftover single-scope row is harmless and swept at boot.
        yield* EmailRuleEntity.upsert(plan.row);
        for (const row of plan.delete)
          yield* EmailRuleEntity.delete({ ruleId: row.ruleId });
        return { rule: plan.row, merged: true, alreadyExists: false };
      case "create":
        yield* EmailRuleEntity.upsert(plan.row);
        return { rule: plan.row, merged: false, alreadyExists: false };
    }
  },
);
