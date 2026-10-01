import { useEffect, useState, type FormEvent } from "react";
import { Effect, Schema } from "effect";
import { forkUiRequest, runUiEffect } from "../effect";

const statusSchema = Schema.Struct({
  enabled: Schema.Boolean,
  phase: Schema.Literals([
    "disabled",
    "authenticated",
    "authentication-needed",
    "transient-outage",
    "rate-limited",
    "unsupported-protocol",
    "awaiting-device-approval",
    "terms-required",
  ]),
  reason: Schema.optional(
    Schema.Literals([
      "mfa",
      "pcs",
      "terms",
      "credentials",
      "session-expired",
      "configuration",
      "protocol",
    ]),
  ),
  challengeId: Schema.optional(Schema.String),
  challengeExpiresAt: Schema.optional(Schema.Number),
  diagnostic: Schema.optional(
    Schema.Struct({
      stage: Schema.Literals([
        "sign-in-init",
        "sign-in-proof",
        "sign-in-complete",
        "account-session",
        "second-factor",
        "second-factor-options",
        "device-notification",
        "code-verification",
        "protected-data-access",
        "apple-request",
        "private-storage",
      ]),
      category: Schema.Literals([
        "apple-response",
        "transport",
        "protocol",
        "storage",
        "authentication",
      ]),
      httpStatus: Schema.optional(Schema.Number),
    }),
  ),
});
const responseSchema = Schema.Struct({ status: statusSchema });
type Status = Schema.Schema.Type<typeof statusSchema>;
type Operation = "status" | "start" | "code" | "verify";

const paths: Record<Operation, string> = {
  status: "/api/reminders/status",
  start: "/api/reminders/auth/start",
  code: "/api/reminders/auth/code",
  verify: "/api/reminders/auth/verify",
};

export function remindersRequest(
  operation: Operation,
  input?: { challengeId: string; code: string },
): Effect.Effect<Status, Error> {
  const path = paths[operation];
  return Effect.gen(function* () {
    const response = yield* Effect.tryPromise({
      try: (signal) =>
        fetch(path, {
          method: operation === "status" ? "GET" : "POST",
          credentials: "omit",
          cache: "no-store",
          redirect: "error",
          headers: operation === "status" ? {} : { "Content-Type": "application/json" },
          ...(operation === "status" ? {} : { body: JSON.stringify(input ?? {}) }),
          signal,
        }),
      catch: () => new Error("Could not reach the Reminders service"),
    });
    if (!response.ok && (operation === "start" || operation === "verify")) {
      const body = yield* Effect.tryPromise({
        try: () => response.clone().json() as Promise<unknown>,
        catch: () => new Error("Invalid Reminders response"),
      }).pipe(Effect.catch(() => Effect.succeed(null)));
      const status = yield* Schema.decodeUnknownEffect(responseSchema)(body).pipe(
        Effect.map((value) => value.status),
        Effect.catch(() => Effect.succeed(null)),
      );
      if (status && status.phase !== "authenticated") return status;
    }
    if (!response.ok) {
      return yield* Effect.fail(
        new Error(
          response.status === 429
            ? "Too many requests. Try again later."
            : operation === "code"
              ? "Code submission was not confirmed. Select Check access before trying again."
              : `Reminders request failed (${response.status})`,
        ),
      );
    }
    const body = yield* Effect.tryPromise({
      try: () => response.json() as Promise<unknown>,
      catch: () => new Error("Invalid Reminders response"),
    });
    const decoded = yield* Schema.decodeUnknownEffect(responseSchema)(body).pipe(
      Effect.mapError(() => new Error("Invalid Reminders response")),
    );
    return decoded.status;
  });
}

function statusText(status: Status): string {
  switch (status.phase) {
    case "disabled":
      return "Reminders monitoring is disabled on the server.";
    case "authenticated":
      return "Connected to iCloud Reminders.";
    case "authentication-needed":
      return status.challengeId
        ? "Enter the six-digit code shown on your trusted Apple device."
        : "Sign in to connect iCloud Reminders.";
    case "awaiting-device-approval":
      return status.reason === "pcs"
        ? "Apple sign-in succeeded, but protected Reminders data is not available yet. Select Check access to request access, approve any prompt on your trusted Apple device, then check access again. Keep Advanced Data Protection enabled."
        : "Approve the sign-in on your trusted Apple device, then check access.";
    case "terms-required":
      return "Apple requires you to review account terms in its own interface.";
    case "rate-limited":
      return "Apple has temporarily limited sign-in attempts. Try again later.";
    case "transient-outage":
      return "Apple is temporarily unavailable. Check access later.";
    case "unsupported-protocol":
      return "Apple sign-in or Reminders access returned an unsupported response. The diagnostic below identifies the failed step. Keep Advanced Data Protection enabled.";
  }
}

export default function RemindersPage() {
  const [status, setStatus] = useState<Status | null>(null);
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    if (window.location.protocol !== "https:") return;
    return forkUiRequest(remindersRequest("status"), {
      onSuccess: setStatus,
      onFailure: (cause) => setError(cause.message),
    });
  }, []);

  const request = (
    operation: Operation,
    input?: { challengeId: string; code: string },
  ) => {
    setBusy(true);
    setError("");
    void runUiEffect(remindersRequest(operation, input)).then(
      (next) => {
        setStatus(next);
        setBusy(false);
      },
      (cause: Error) => {
        setError(cause.message);
        setBusy(false);
      },
    );
  };

  const submitCode = (event: FormEvent) => {
    event.preventDefault();
    const challengeId = status?.challengeId;
    const submittedCode = code;
    setCode("");
    if (!challengeId || !/^[0-9]{6}$/.test(submittedCode)) {
      setError("Enter a six-digit code from your trusted device.");
      return;
    }
    request("code", { challengeId, code: submittedCode });
  };

  return (
    <section className="reminders-panel" aria-label="iCloud Reminders">
      <h1>iCloud Reminders</h1>
      <p>
        Connect your account to monitor Reminders. Keep Advanced Data Protection
        enabled.
      </p>
      {window.location.protocol !== "https:" ? (
        <p role="alert">Open this page over HTTPS to administer Reminders.</p>
      ) : (
        <>
          <p role="status">
            {status ? statusText(status) : "Loading connection status…"}
          </p>
          {status?.diagnostic && (
            <p role="alert">
              Failed step: {status.diagnostic.stage} ({status.diagnostic.category}
              {status.diagnostic.httpStatus
                ? `, Apple HTTP ${status.diagnostic.httpStatus}`
                : ""}
              ).
            </p>
          )}
          {status?.challengeId && status.phase === "authentication-needed" && (
            <form onSubmit={submitCode}>
              <label htmlFor="reminders-code">Verification code</label>
              <input
                id="reminders-code"
                type="text"
                inputMode="numeric"
                autoComplete="one-time-code"
                pattern="[0-9]{6}"
                maxLength={6}
                value={code}
                onChange={(event) => setCode(event.target.value)}
              />
              <button type="submit" disabled={busy}>
                {busy ? "Verifying…" : "Submit code"}
              </button>
            </form>
          )}
          {status?.enabled &&
            [
              "authentication-needed",
              "unsupported-protocol",
              "transient-outage",
              "rate-limited",
            ].includes(status.phase) &&
            !status.challengeId && (
              <button type="button" disabled={busy} onClick={() => request("start")}>
                Start sign-in
              </button>
            )}
          {status?.enabled && status.phase !== "disabled" && (
            <button type="button" disabled={busy} onClick={() => request("verify")}>
              Check access
            </button>
          )}
        </>
      )}
      {error && <p role="alert">{error}</p>}
    </section>
  );
}
