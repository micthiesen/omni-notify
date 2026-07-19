import { Entity } from "@micthiesen/mitools/entities";

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
export function findSenderRule(
  from: string,
  pipeline: "parcel" | "calendar",
): EmailRuleData | undefined {
  const fromLower = from.toLowerCase();
  const matches = EmailRuleEntity.getAll().filter(
    (rule) =>
      (rule.scope === pipeline || rule.scope === "both") &&
      matchesSenderPattern(fromLower, rule.pattern),
  );
  return matches.find((rule) => rule.verdict === "block") ?? matches[0];
}

export function getSenderRuleVerdict(
  from: string,
  pipeline: "parcel" | "calendar",
): EmailRuleVerdict | undefined {
  return findSenderRule(from, pipeline)?.verdict;
}

export function listEmailRules(): EmailRuleData[] {
  return EmailRuleEntity.getAll().sort((a, b) => b.createdAt - a.createdAt);
}

export function upsertEmailRule(entry: {
  pattern: string;
  scope: EmailRuleScope;
  verdict: EmailRuleVerdict;
}): EmailRuleData {
  const pattern = entry.pattern.trim().toLowerCase();
  if (!pattern) throw new Error("Sender rule pattern must be non-empty");
  const ruleId = `${entry.scope}:${pattern}`;
  const existing = EmailRuleEntity.get({ ruleId });
  const row: EmailRuleData = {
    ruleId,
    pattern,
    scope: entry.scope,
    verdict: entry.verdict,
    createdAt: existing?.createdAt ?? Date.now(),
  };
  EmailRuleEntity.upsert(row);
  return row;
}

export function deleteEmailRule(ruleId: string): boolean {
  if (EmailRuleEntity.get({ ruleId }) === undefined) return false;
  EmailRuleEntity.delete({ ruleId });
  return true;
}

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

export function getSenderRuleCoverage(pattern: string): EmailRuleCoverage {
  const normalized = pattern.trim().toLowerCase();
  const matches = EmailRuleEntity.getAll().filter(
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
}

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

export function planRuleAdd(
  pattern: string,
  scope: EmailRuleScope,
  verdict: EmailRuleVerdict,
): RuleAddPlan {
  const normalized = pattern.trim().toLowerCase();
  if (!normalized) throw new Error("Sender rule pattern must be non-empty");
  const coverage = getSenderRuleCoverage(normalized);

  // A "both" rule is authoritative for a pattern: it supersedes any existing
  // single-scope rows, so fold them in (delete) rather than leaving a
  // contradictory parcel/calendar rule that "block beats allow" would honor.
  if (scope === "both") {
    const singles = coverage.matches.filter(
      (rule) => rule.scope === "parcel" || rule.scope === "calendar",
    );
    const existingBothRow = coverage.matches.find((rule) => rule.scope === "both");
    if (existingBothRow?.verdict === verdict && singles.length === 0) {
      return { action: "noop-exists", existing: existingBothRow };
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
      return { action: "upgrade-to-both", delete: singles, row };
    }
    return { action: "create", row };
  }

  // Already fully covered by an existing user rule of the same verdict?
  // A "both" row covers every target scope (parcel, calendar, or both).
  const existingBoth = coverage.matches.find(
    (rule) => rule.scope === "both" && rule.verdict === verdict,
  );
  if (existingBoth) {
    return { action: "noop-exists", existing: existingBoth };
  }
  const existingExact = coverage.matches.find(
    (rule) => rule.scope === scope && rule.verdict === verdict,
  );
  if (existingExact) {
    return { action: "noop-exists", existing: existingExact };
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
    return { action: "upgrade-to-both", delete: [opposite], row: newRow };
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
  };
}

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
export function upsertEmailRuleChecked(input: {
  pattern: string;
  scope: EmailRuleScope;
  verdict: EmailRuleVerdict;
}): UpsertEmailRuleCheckedResult {
  const plan = planRuleAdd(input.pattern, input.scope, input.verdict);
  switch (plan.action) {
    case "noop-exists":
      return { rule: plan.existing, merged: false, alreadyExists: true };
    case "upgrade-to-both":
      // Write the superseding "both" row first so a crash mid-op can never lose
      // coverage — a leftover single-scope row is harmless and swept at boot.
      EmailRuleEntity.upsert(plan.row);
      for (const row of plan.delete) EmailRuleEntity.delete({ ruleId: row.ruleId });
      return { rule: plan.row, merged: true, alreadyExists: false };
    case "create":
      EmailRuleEntity.upsert(plan.row);
      return { rule: plan.row, merged: false, alreadyExists: false };
  }
}

/**
 * Pure sweep: given a full set of rule rows, find every same-pattern,
 * same-verdict parcel+calendar pair that should collapse into one "both"
 * row. Used for a one-time normalization pass over `EmailRuleEntity` at
 * boot; callers apply the returned deletes+create themselves.
 */
export function findMergeableRuleGroups(
  rows: EmailRuleData[],
): Array<{ delete: EmailRuleData[]; create: EmailRuleData }> {
  // Patterns already governed by a "both" row must not be merged — doing so
  // would upsert a fresh "both" row that clobbers the existing one (and could
  // silently flip its verdict).
  const bothPatterns = new Set(
    rows.filter((r) => r.scope === "both").map((r) => r.pattern),
  );
  const byPatternVerdict = new Map<string, EmailRuleData[]>();
  for (const row of rows) {
    if (row.scope === "both") continue;
    if (bothPatterns.has(row.pattern)) continue;
    const key = `${row.pattern}:::${row.verdict}`;
    const group = byPatternVerdict.get(key);
    if (group) group.push(row);
    else byPatternVerdict.set(key, [row]);
  }

  const groups: Array<{ delete: EmailRuleData[]; create: EmailRuleData }> = [];
  for (const rows of byPatternVerdict.values()) {
    const parcel = rows.find((r) => r.scope === "parcel");
    const calendar = rows.find((r) => r.scope === "calendar");
    if (!parcel || !calendar) continue;
    const createdAt = Math.min(parcel.createdAt, calendar.createdAt);
    groups.push({
      delete: [parcel, calendar],
      create: {
        ruleId: `both:${parcel.pattern}`,
        pattern: parcel.pattern,
        scope: "both",
        verdict: parcel.verdict,
        createdAt,
      },
    });
  }
  return groups;
}

/**
 * Idempotent normalization sweep, run at boot. Two passes:
 *  1. Canonicalize every rule's pattern (`plex.tv` → `@plex.tv`), collapsing any
 *     rules that now share a (scope, pattern) — earliest createdAt wins; on a
 *     verdict conflict the most-recently-created verdict wins (ties → block).
 *  2. Collapse same-pattern + same-verdict parcel/calendar pairs into one `both`
 *     rule.
 * Returns the number of rows changed/removed. A second run is a no-op.
 */
export function normalizeSenderRules(): number {
  let changes = 0;

  // Pass 1: canonical pattern form + collapse resulting (scope, pattern) dupes.
  const canonical = new Map<string, EmailRuleData>();
  const verdictAt = new Map<string, number>();
  const toDelete = new Set<string>();
  for (const row of EmailRuleEntity.getAll()) {
    const pattern = normalizeRulePattern(row.pattern);
    const ruleId = `${row.scope}:${pattern}`;
    const existing = canonical.get(ruleId);
    if (!existing) {
      canonical.set(ruleId, { ...row, pattern, ruleId });
      verdictAt.set(ruleId, row.createdAt);
      if (ruleId !== row.ruleId) {
        toDelete.add(row.ruleId);
        changes++;
      }
    } else {
      existing.createdAt = Math.min(existing.createdAt, row.createdAt);
      // Honor the most-recently-expressed verdict so a later explicit override
      // (e.g. an allow added after a legacy block) isn't silently discarded;
      // ties fall to block, the safe default.
      const prevAt = verdictAt.get(ruleId) ?? 0;
      if (
        row.createdAt > prevAt ||
        (row.createdAt === prevAt && row.verdict === "block")
      ) {
        existing.verdict = row.verdict;
        verdictAt.set(ruleId, row.createdAt);
      }
      toDelete.add(row.ruleId);
      changes++;
    }
  }
  for (const ruleId of toDelete) EmailRuleEntity.delete({ ruleId });
  for (const row of canonical.values()) EmailRuleEntity.upsert(row);

  // Pass 2: fold parcel+calendar pairs into a single `both` rule. Upsert the
  // merged row before deleting the singles so a crash can't drop coverage.
  const groups = findMergeableRuleGroups(EmailRuleEntity.getAll());
  for (const group of groups) {
    EmailRuleEntity.upsert(group.create);
    for (const row of group.delete) {
      EmailRuleEntity.delete({ ruleId: row.ruleId });
    }
    changes++;
  }
  return changes;
}
