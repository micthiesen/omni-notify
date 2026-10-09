// Server-rendered Reminders administration page (no framework, CSP script-src 'self').
// Behavior mirrors the former React RemindersPage: bounded status display, explicit
// sign-in, six-digit code entry and access checks. Codes are never stored.
"use strict";

(function () {
  const phases = [
    "disabled",
    "authenticated",
    "authentication-needed",
    "transient-outage",
    "rate-limited",
    "unsupported-protocol",
    "awaiting-device-approval",
    "terms-required",
  ];
  const paths = {
    status: "/api/reminders/status",
    start: "/api/reminders/auth/start",
    code: "/api/reminders/auth/code",
    verify: "/api/reminders/auth/verify",
  };
  const $ = (id) => document.getElementById(id);
  let status = null;
  let busy = false;

  function decode(body) {
    const value = body && typeof body === "object" ? body.status : null;
    if (!value || typeof value !== "object") return null;
    if (typeof value.enabled !== "boolean" || !phases.includes(value.phase)) return null;
    return value;
  }

  async function request(operation, input) {
    let response;
    try {
      response = await fetch(paths[operation], {
        method: operation === "status" ? "GET" : "POST",
        credentials: "omit",
        cache: "no-store",
        redirect: "error",
        headers: operation === "status" ? {} : { "Content-Type": "application/json" },
        body: operation === "status" ? undefined : JSON.stringify(input || {}),
      });
    } catch {
      throw new Error("Could not reach the Reminders service");
    }
    let body = null;
    try {
      body = await response.json();
    } catch {
      body = null;
    }
    if (!response.ok && (operation === "start" || operation === "verify")) {
      const decoded = decode(body);
      if (decoded && decoded.phase !== "authenticated") return decoded;
    }
    if (!response.ok) {
      throw new Error(
        response.status === 429
          ? "Too many requests. Try again later."
          : operation === "code"
            ? "Code submission was not confirmed. Select Check access before trying again."
            : `Reminders request failed (${response.status})`,
      );
    }
    const decoded = decode(body);
    if (!decoded) throw new Error("Invalid Reminders response");
    return decoded;
  }

  function statusText(value) {
    switch (value.phase) {
      case "disabled":
        return "Reminders monitoring is disabled on the server.";
      case "authenticated":
        return "Connected to iCloud Reminders.";
      case "authentication-needed":
        return value.challengeId
          ? "Enter the six-digit code shown on your trusted Apple device."
          : "Sign in to connect iCloud Reminders.";
      case "awaiting-device-approval":
        return value.reason === "pcs"
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
      default:
        return "";
    }
  }

  // Connection badge: Status kind and word (mirrors `phase_badge` in omni-web-pages).
  function phaseBadge(value) {
    if (!value) return ["running", "Checking"];
    switch (value.phase) {
      case "disabled":
        return ["idle", "Disabled"];
      case "authenticated":
        return ["ok", "Connected"];
      case "authentication-needed":
        return ["warn", value.challengeId ? "Code needed" : "Sign-in needed"];
      case "awaiting-device-approval":
        return ["warn", "Awaiting approval"];
      case "terms-required":
        return ["warn", "Terms required"];
      case "rate-limited":
        return ["warn", "Rate limited"];
      case "transient-outage":
        return ["warn", "Apple unavailable"];
      case "unsupported-protocol":
        return ["fault", "Unsupported response"];
      default:
        return ["idle", "Unknown"];
    }
  }

  function showError(message) {
    const node = $("error");
    node.textContent = message;
    node.hidden = !message;
  }

  function render() {
    $("status").textContent = status ? statusText(status) : "Loading connection status…";
    const diagnostic = status && status.diagnostic;
    const diagnosticNode = $("diagnostic");
    if (diagnostic && typeof diagnostic.stage === "string") {
      const http = diagnostic.httpStatus ? `, Apple HTTP ${diagnostic.httpStatus}` : "";
      diagnosticNode.textContent = `Failed step: ${diagnostic.stage} (${diagnostic.category}${http}).`;
      diagnosticNode.hidden = false;
    } else {
      diagnosticNode.hidden = true;
    }
    const challenge = Boolean(status && status.challengeId && status.phase === "authentication-needed");
    $("code-form").hidden = !challenge;
    $("submit-code").disabled = busy;
    $("submit-code").textContent = busy ? "Verifying…" : "Submit code";
    const canStart = Boolean(
      status &&
        status.enabled &&
        ["authentication-needed", "unsupported-protocol", "transient-outage", "rate-limited"].includes(status.phase) &&
        !status.challengeId,
    );
    $("start").hidden = !canStart;
    $("start").disabled = busy;
    $("verify").hidden = !(status && status.enabled && status.phase !== "disabled");
    $("verify").disabled = busy;
    $("verify").classList.toggle("primary", !canStart && !challenge);
    const [kind, word] = phaseBadge(status);
    $("phase").className = `status ${kind}`;
    $("phase").textContent = word;
  }

  function run(operation, input) {
    busy = true;
    showError("");
    render();
    request(operation, input).then(
      (next) => {
        status = next;
        busy = false;
        render();
      },
      (cause) => {
        busy = false;
        showError(cause.message);
        render();
      },
    );
  }

  function init() {
    if (window.location.protocol !== "https:") {
      $("insecure").hidden = false;
      return;
    }
    $("controls").hidden = false;
    render();
    $("start").addEventListener("click", () => run("start"));
    $("verify").addEventListener("click", () => run("verify"));
    $("code-form").addEventListener("submit", (event) => {
      event.preventDefault();
      const input = $("reminders-code");
      const code = input.value;
      input.value = "";
      const challengeId = status && status.challengeId;
      if (!challengeId || !/^[0-9]{6}$/.test(code)) {
        showError("Enter a six-digit code from your trusted device.");
        return;
      }
      run("code", { challengeId, code });
    });
    request("status").then(
      (next) => {
        status = next;
        render();
      },
      (cause) => showError(cause.message),
    );
  }

  init();
})();
