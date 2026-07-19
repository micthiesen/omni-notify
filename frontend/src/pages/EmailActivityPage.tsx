import { useEffect, useMemo, useState } from "react";
import {
  type EmailActivity,
  type EmailActivityOutcome,
  type EmailBuiltinRules,
  type EmailFeedback,
  type EmailPipeline,
  type EmailRule,
  type EmailRuleScope,
  type EmailRuleVerdict,
  createEmailRule,
  deleteEmailRule,
  fetchEmailActivity,
  fetchEmailFeedback,
  fetchEmailRules,
} from "../api";
import { EmailLogModal } from "../components/EmailLogModal";
import { ShowMoreButton, useShowMore } from "../components/ShowMore";
import { StatusFilterChips } from "../components/StatusFilterChips";
import { Toast, useToast } from "../components/Toast";
import { OUTCOME_LABELS, PIPELINE_LABELS } from "../utils/emailLabels";
import { formatAbsolute, formatCents } from "../utils/format";

/** admitTier → short human label; unrecognized tiers fall back to the raw value. */
const ADMIT_TIER_LABELS: Record<string, string> = {
  rule: "Rule",
  builtin: "Built-in",
  triage: "Triage",
  "keyword-fallback": "Keyword Fallback",
  "carrier-name": "Carrier Name",
};

const OUTCOME_FILTER_ORDER: readonly EmailActivityOutcome[] = [
  "processed",
  "partial",
  "failed",
  "no_matches",
  "skipped",
  "filtered",
  "error",
];

const SCOPE_LABELS: Record<EmailRuleScope, string> = {
  parcel: "Parcels",
  calendar: "Calendar",
  both: "Both",
};

/**
 * Collapsible user-editable sender rules: list existing rules with delete,
 * plus an inline add form. Collapsed by default with a count badge.
 */
function SenderRulesSection() {
  const [rules, setRules] = useState<EmailRule[] | null>(null);
  const [builtin, setBuiltin] = useState<EmailBuiltinRules | null>(null);
  const [open, setOpen] = useState(false);
  const [builtinOpen, setBuiltinOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pattern, setPattern] = useState("");
  const [scope, setScope] = useState<EmailRuleScope>("parcel");
  const [verdict, setVerdict] = useState<EmailRuleVerdict>("block");
  const [submitting, setSubmitting] = useState(false);
  const { toast, showToast } = useToast();

  useEffect(() => {
    let cancelled = false;
    fetchEmailRules()
      .then((res) => {
        if (cancelled) return;
        setRules(res.rules);
        setBuiltin(res.builtin);
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : "Failed to load rules");
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const addRule = async () => {
    const trimmed = pattern.trim().toLowerCase();
    if (!trimmed || submitting) return;
    setSubmitting(true);
    setError(null);
    try {
      const res = await createEmailRule({ pattern: trimmed, scope, verdict });
      switch (res.status) {
        case "created":
          showToast("Rule added", "info");
          break;
        case "merged":
          showToast(res.message ?? "Merged into a single Both rule", "info");
          break;
        case "exists":
          showToast(res.message ?? "That rule already exists", "error");
          break;
        case "builtin":
          showToast(res.message ?? "Already covered by a built-in list", "error");
          break;
      }
      // builtin/exists may return no new rule — only prepend when one landed.
      if (res.rule) {
        const rule = res.rule;
        setRules((prev) => {
          const others = (prev ?? []).filter((r) => r.ruleId !== rule.ruleId);
          return [rule, ...others];
        });
      }
      setPattern("");
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to add rule");
    } finally {
      setSubmitting(false);
    }
  };

  const removeRule = async (rule: EmailRule) => {
    const confirmed = window.confirm(
      `Delete the ${rule.verdict} rule for "${rule.pattern}" (${SCOPE_LABELS[rule.scope]})?`,
    );
    if (!confirmed) return;
    setError(null);
    try {
      await deleteEmailRule(rule.ruleId);
      setRules((prev) => prev?.filter((r) => r.ruleId !== rule.ruleId) ?? prev);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to delete rule");
    }
  };

  return (
    <section className="mail-rules">
      <Toast toast={toast} />
      <button
        type="button"
        className="mail-rules-toggle"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        <span className="mail-rules-caret">{open ? "▾" : "▸"}</span>
        Sender Rules
        {rules !== null && <span className="chip-btn-count">{rules.length}</span>}
      </button>
      {open && (
        <div className="mail-rules-body">
          {rules === null && error === null && (
            <div className="muted mail-rules-note">Loading rules…</div>
          )}
          {rules !== null && rules.length === 0 && (
            <div className="muted mail-rules-note">
              No sender rules yet. Rules match a full address (x@y.com) or a
              domain (y.com).
            </div>
          )}
          {rules !== null && rules.length > 0 && (
            <ul className="rule-list">
              {rules.map((rule) => (
                <li key={rule.ruleId} className="rule-row">
                  <span className={`rule-verdict rule-verdict-${rule.verdict}`}>
                    {rule.verdict === "block" ? "Block" : "Allow"}
                  </span>
                  <span className="rule-pattern" title={rule.pattern}>
                    {rule.pattern}
                  </span>
                  <span className="rule-scope">{SCOPE_LABELS[rule.scope]}</span>
                  <button
                    type="button"
                    className="rule-delete"
                    onClick={() => removeRule(rule)}
                  >
                    Delete
                  </button>
                </li>
              ))}
            </ul>
          )}
          <form
            className="rule-form"
            onSubmit={(event) => {
              event.preventDefault();
              void addRule();
            }}
          >
            <input
              type="text"
              className="rule-form-input"
              placeholder="x@y.com or y.com"
              value={pattern}
              onChange={(event) => setPattern(event.target.value)}
              aria-label="Sender pattern"
            />
            <select
              className="rule-form-select"
              value={scope}
              onChange={(event) => setScope(event.target.value as EmailRuleScope)}
              aria-label="Rule scope"
            >
              <option value="parcel">Parcels</option>
              <option value="calendar">Calendar</option>
              <option value="both">Both</option>
            </select>
            <select
              className="rule-form-select"
              value={verdict}
              onChange={(event) => setVerdict(event.target.value as EmailRuleVerdict)}
              aria-label="Rule verdict"
            >
              <option value="block">Block</option>
              <option value="allow">Allow</option>
            </select>
            <button
              type="submit"
              className="run-btn"
              disabled={pattern.trim() === "" || submitting}
            >
              {submitting ? "Adding…" : "Add"}
            </button>
          </form>
          {error && <div className="error-inline mail-rules-error">{error}</div>}
          {builtin && (
            <div className="rule-builtin">
              <button
                type="button"
                className="mail-rules-toggle"
                aria-expanded={builtinOpen}
                onClick={() => setBuiltinOpen((v) => !v)}
              >
                <span className="mail-rules-caret">{builtinOpen ? "▾" : "▸"}</span>
                Built-in Lists
                <span className="chip-btn-count">
                  {builtin.parcel.blocked.length +
                    builtin.parcel.autoPass.length +
                    builtin.calendar.blocked.length +
                    builtin.calendar.autoPass.length}
                </span>
              </button>
              {builtinOpen && (
                <div className="rule-builtin-body">
                  <BuiltinList
                    label="Parcels · Blocked"
                    verdict="block"
                    patterns={builtin.parcel.blocked}
                  />
                  <BuiltinList
                    label="Parcels · Auto-pass"
                    verdict="allow"
                    patterns={builtin.parcel.autoPass}
                  />
                  <BuiltinList
                    label="Calendar · Blocked"
                    verdict="block"
                    patterns={builtin.calendar.blocked}
                  />
                  <BuiltinList
                    label="Calendar · Auto-pass"
                    verdict="allow"
                    patterns={builtin.calendar.autoPass}
                  />
                  <div className="muted mail-rules-note">
                    Built-ins ship with the app and live in code. An Allow rule
                    above overrides a built-in block for that sender.
                  </div>
                </div>
              )}
            </div>
          )}
        </div>
      )}
    </section>
  );
}

function BuiltinList({
  label,
  verdict,
  patterns,
}: {
  label: string;
  verdict: EmailRuleVerdict;
  patterns: string[];
}) {
  return (
    <div className="rule-builtin-group">
      <div className="rule-builtin-label">
        <span className={`rule-verdict rule-verdict-${verdict}`}>
          {verdict === "block" ? "Block" : "Allow"}
        </span>
        <span>{label}</span>
      </div>
      <div className="rule-builtin-patterns">
        {patterns.map((pattern) => (
          <code key={pattern} className="rule-builtin-pattern">
            {pattern}
          </code>
        ))}
      </div>
    </div>
  );
}

export default function EmailActivityPage() {
  const [activities, setActivities] = useState<EmailActivity[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pipeline, setPipeline] = useState<EmailPipeline | null>(null);
  const [outcome, setOutcome] = useState<EmailActivityOutcome | "">("");
  const [feedback, setFeedback] = useState<ReadonlyMap<string, EmailFeedback>>(
    new Map(),
  );
  const [logsFor, setLogsFor] = useState<EmailActivity | null>(null);

  useEffect(() => {
    let cancelled = false;
    setActivities(null);
    setError(null);
    fetchEmailActivity(pipeline ?? undefined, 500)
      .then((res) => {
        if (!cancelled) setActivities(res.activities);
      })
      .catch((err) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : "Failed to load activity");
        }
      });
    return () => {
      cancelled = true;
    };
  }, [pipeline]);

  useEffect(() => {
    let cancelled = false;
    fetchEmailFeedback()
      .then((res) => {
        if (cancelled) return;
        setFeedback(new Map(res.feedback.map((f) => [f.activityId, f])));
      })
      .catch(() => {
        // Feedback indicators are decorative; the page works without them.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const outcomeCounts = useMemo(() => {
    const counts = new Map<EmailActivityOutcome, number>();
    for (const activity of activities ?? []) {
      counts.set(activity.outcome, (counts.get(activity.outcome) ?? 0) + 1);
    }
    return counts;
  }, [activities]);

  const filtered = useMemo(
    () =>
      outcome === ""
        ? (activities ?? [])
        : (activities ?? []).filter((a) => a.outcome === outcome),
    [activities, outcome],
  );

  const { visible, hasMore, remaining, showMore } = useShowMore(
    filtered,
    30,
    `${pipeline ?? "all"}:${outcome}`,
  );

  const handleActivityChange = (updated: EmailActivity) => {
    setActivities(
      (prev) =>
        prev?.map((a) => (a.activityId === updated.activityId ? updated : a)) ??
        prev,
    );
  };

  const handleFeedbackChange = (
    activityId: string,
    updated: EmailFeedback | null,
  ) => {
    setFeedback((prev) => {
      const next = new Map(prev);
      if (updated) next.set(activityId, updated);
      else next.delete(activityId);
      return next;
    });
  };

  return (
    <>
      <div className="page-header">
        <div className="page-header-stack">
          <h1>Email Activity</h1>
          <p className="page-subtitle">
            What the parcel and calendar pipelines did with each email.
          </p>
        </div>
      </div>

      <div className="rec-filters">
        <button
          type="button"
          className={`chip-btn ${pipeline === null ? "active" : ""}`}
          onClick={() => setPipeline(null)}
        >
          All
        </button>
        {(Object.keys(PIPELINE_LABELS) as EmailPipeline[]).map((p) => (
          <button
            key={p}
            type="button"
            className={`chip-btn ${pipeline === p ? "active" : ""}`}
            onClick={() => setPipeline(pipeline === p ? null : p)}
          >
            {PIPELINE_LABELS[p]}
          </button>
        ))}
      </div>

      {activities !== null && activities.length > 0 && (
        <StatusFilterChips
          order={OUTCOME_FILTER_ORDER}
          labels={OUTCOME_LABELS}
          counts={outcomeCounts}
          total={activities.length}
          active={outcome}
          onChange={setOutcome}
        />
      )}

      <SenderRulesSection />

      {activities === null && error === null && (
        <div className="loading">Loading…</div>
      )}
      {error && activities === null && (
        <div className="error">
          <div>Failed to load email activity</div>
          <div className="error-detail">{error}</div>
        </div>
      )}
      {activities !== null && activities.length === 0 && (
        <div className="muted">
          No email activity recorded yet. Activity appears here as new emails are
          processed.
        </div>
      )}
      {activities !== null && activities.length > 0 && filtered.length === 0 && (
        <div className="muted">No emails match the current filters.</div>
      )}

      {filtered.length > 0 && (
        <ul className="mail-list">
          {visible.map((activity) => (
            <li key={activity.activityId} className="mail-row">
              <button
                type="button"
                className="mail-row-btn"
                title="View processing logs"
                onClick={() => setLogsFor(activity)}
              >
                <div className="mail-row-top">
                  <span className="mail-subject" title={activity.subject}>
                    {activity.subject || "(no subject)"}
                  </span>
                  <span className={`mail-outcome mail-outcome-${activity.outcome}`}>
                    {OUTCOME_LABELS[activity.outcome]}
                  </span>
                </div>
                <div className="mail-row-meta">
                  <span className="briefing-badge">
                    {PIPELINE_LABELS[activity.pipeline]}
                  </span>
                  <span className="mail-from" title={activity.from}>
                    {activity.from}
                  </span>
                  {activity.admitReason && (
                    <span className="mail-admit" title={activity.admitReason}>
                      Admitted: {activity.admitReason}
                    </span>
                  )}
                  {activity.admitTier && (
                    <span
                      className={`email-tier email-tier-${activity.admitTier}`}
                      title={`Admitted via ${
                        ADMIT_TIER_LABELS[activity.admitTier] ?? activity.admitTier
                      }`}
                    >
                      {ADMIT_TIER_LABELS[activity.admitTier] ?? activity.admitTier}
                    </span>
                  )}
                  {activity.costCents != null && activity.costCents > 0 && (
                    <span className="email-cost" title="LLM cost for this email">
                      {formatCents(activity.costCents)}
                    </span>
                  )}
                  {feedback.has(activity.activityId) && (
                    <span className="mail-feedback-tag">Feedback</span>
                  )}
                  <span className="mail-time">
                    {formatAbsolute(activity.processedAt)}
                  </span>
                </div>
                {activity.detail && (
                  <div className="mail-detail">{activity.detail}</div>
                )}
                {activity.items.length > 0 && (
                  <ul className="mail-items">
                    {activity.items.map((item, index) => (
                      <li key={`${index}-${item}`}>{item}</li>
                    ))}
                  </ul>
                )}
              </button>
            </li>
          ))}
        </ul>
      )}
      {hasMore && <ShowMoreButton remaining={remaining} onClick={showMore} />}
      {logsFor && (
        <EmailLogModal
          key={logsFor.activityId}
          activity={logsFor}
          feedback={feedback.get(logsFor.activityId) ?? null}
          onActivityChange={handleActivityChange}
          onFeedbackChange={handleFeedbackChange}
          onClose={() => setLogsFor(null)}
        />
      )}
    </>
  );
}
