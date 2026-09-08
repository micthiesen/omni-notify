#!/usr/bin/env node

// Creates a new Pushover application and opens the app page in the browser.
// Usage: node pushover-create-app.mjs <cookie> <name> [icon-path] [description]
//
// Prints the app URL to stdout and opens it in the default browser.

import { readFileSync } from "fs";
import { basename } from "path";

const [, , cookie, name, iconPath, description] = process.argv;

if (!cookie || !name) {
  console.error("Usage: pushover-create-app.mjs <cookie> <name> [icon-path] [description]");
  process.exit(1);
}

function getCsrf(html) {
  const m = html.match(/name="authenticity_token" value="([^"]+)"/);
  if (!m) throw new Error("CSRF token not found");
  return m[1];
}

// Get create form CSRF
const buildPage = await fetch("https://pushover.net/apps/build", {
  headers: { Cookie: cookie },
});
if (buildPage.status !== 200) {
  throw new Error(`Session expired or invalid: /apps/build returned ${buildPage.status}`);
}
const csrf = getCsrf(await buildPage.text());

// Build multipart form
const form = new FormData();
form.append("utf8", "✓");
form.append("authenticity_token", csrf);
form.append("application[short_name]", name);
form.append("application[description]", description || "");
form.append("application[url]", "");
form.append("application[terms_of_service]", "1");

if (iconPath) {
  const iconData = readFileSync(iconPath);
  const blob = new Blob([iconData], { type: "image/png" });
  form.append("application[icon]", blob, basename(iconPath));
}

// Create the app
const res = await fetch("https://pushover.net/apps/create", {
  method: "POST",
  headers: { Cookie: cookie },
  body: form,
  redirect: "manual",
});

if (res.status !== 302) {
  const body = await res.text();
  // Try to extract error messages
  const errors = body.match(/<div class="alert alert-error">([\s\S]*?)<\/div>/);
  const msg = errors ? errors[1].replace(/<[^>]+>/g, "").trim() : "Unknown error";
  throw new Error(`App creation failed (${res.status}): ${msg}`);
}

const appUrl = res.headers.get("location");
const fullUrl = appUrl.startsWith("http") ? appUrl : "https://pushover.net" + appUrl;

// Open the app page in the browser so the user can grab the API token
const { execSync } = await import("child_process");
execSync(`open ${JSON.stringify(fullUrl)}`);
console.log(fullUrl);
