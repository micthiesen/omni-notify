#!/usr/bin/env node

// Logs into Pushover via email/password + TOTP from 1Password.
// Prints the session cookie value to stdout for use by other scripts.

import { execSync } from "child_process";

const OP_ITEM = "46brgj2crqbpfjnnhmhwilqbsa";

const jar = {};
function saveCookies(res) {
  for (const c of res.headers.getSetCookie() || []) {
    const [kv] = c.split(";");
    const eq = kv.indexOf("=");
    jar[kv.slice(0, eq).trim()] = kv.slice(eq + 1);
  }
}
function cookies() {
  return Object.entries(jar)
    .map(([k, v]) => `${k}=${v}`)
    .join("; ");
}
function getCsrf(html) {
  const m = html.match(/name="authenticity_token" value="([^"]+)"/);
  if (!m) throw new Error("CSRF token not found");
  return m[1];
}

const email = execSync(`op read 'op://Private/Pushover/username'`).toString().trim();
const password = execSync(`op read 'op://Private/Pushover/password'`).toString().trim();
const otp = execSync(`op item get ${OP_ITEM} --otp`).toString().trim();

// GET login page
const r1 = await fetch("https://pushover.net/login");
saveCookies(r1);
const csrf1 = getCsrf(await r1.text());

// POST email + password
const r2 = await fetch("https://pushover.net/login/login", {
  method: "POST",
  headers: { "Content-Type": "application/x-www-form-urlencoded", Cookie: cookies() },
  body: new URLSearchParams({
    utf8: "✓",
    authenticity_token: csrf1,
    "user[email]": email,
    "user[password]": password,
    commit: "Login",
  }).toString(),
  redirect: "manual",
});
saveCookies(r2);
const html2 = await r2.text();

if (!html2.includes("user[twofa_code]")) {
  throw new Error("Expected 2FA prompt but didn't get one. Login may have failed.");
}

const csrf2 = getCsrf(html2);

// POST TOTP
const r3 = await fetch("https://pushover.net/login/login", {
  method: "POST",
  headers: { "Content-Type": "application/x-www-form-urlencoded", Cookie: cookies() },
  body: new URLSearchParams({
    utf8: "✓",
    authenticity_token: csrf2,
    "user[twofa_code]": otp,
    commit: "Verify",
  }).toString(),
  redirect: "manual",
});
saveCookies(r3);

// Follow redirect
const loc = r3.headers.get("location");
if (loc) {
  const url = loc.startsWith("http") ? loc : "https://pushover.net" + loc;
  const r3b = await fetch(url, { headers: { Cookie: cookies() }, redirect: "manual" });
  saveCookies(r3b);
}

// Verify we're logged in
const verify = await fetch("https://pushover.net/apps/build", {
  headers: { Cookie: cookies() },
  redirect: "manual",
});

if (verify.status !== 200) {
  throw new Error(`Login failed: /apps/build returned ${verify.status}`);
}

// Output cookie for other scripts
console.log(cookies());
