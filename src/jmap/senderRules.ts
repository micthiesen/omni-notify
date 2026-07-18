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
  if (pattern.includes("@")) {
    const addr = senderAddress(fromLower);
    return addr === pattern || addr.endsWith(pattern);
  }
  const domain = senderDomain(fromLower);
  return domain === pattern || domain.endsWith(`.${pattern}`);
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
