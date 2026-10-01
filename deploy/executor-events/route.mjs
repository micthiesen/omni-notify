// Run inside the existing NPM container with `node --input-type=module` on stdin.
// Uses NPM's own authorized update/configuration path. Never prints a token.
import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import { execFileSync } from "node:child_process";
import ProxyHost from "/app/models/proxy_host.js";
import Token from "/app/models/token.js";
import Access from "/app/lib/access.js";
import proxyHosts from "/app/internal/proxy-host.js";

const mode = process.env.OMNI_ROUTE_MODE || "inspect";
if (!["inspect", "prepare", "apply", "rollback"].includes(mode))
  throw new Error("Invalid route mode");
const id = 44;
const backupPath = "/data/omni-executor-events-route.json";
const hash = (value) => createHash("sha256").update(value).digest("hex");
const invariants = (row) => {
  const data = row.toJSON();
  for (const key of ["advanced_config", "modified_on", "meta"]) delete data[key];
  return hash(JSON.stringify(data));
};
const host = await ProxyHost.query().findById(id);
if (
  !host ||
  host.domain_names.length !== 1 ||
  host.domain_names[0] !== "mcp.syas.ca" ||
  host.forward_host !== "executor" ||
  host.forward_port !== 4788 ||
  host.access_list_id !== 0
)
  throw new Error("Existing MCP host configuration differs; inspect before proceeding");
const start = "# omni-executor-events:begin";
const finish = "# omni-executor-events:end";
const snippet = `${start}
location = /mcp {
  resolver 127.0.0.11 valid=30s;
  set $omni_executor_events executor-events-adapter;
  proxy_pass http://$omni_executor_events:4789;
  proxy_set_header Host $host;
  proxy_set_header X-Real-IP $remote_addr;
  proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
  proxy_set_header X-Forwarded-Proto $scheme;
  proxy_http_version 1.1;
  proxy_buffering off;
  proxy_read_timeout 3600s;
  proxy_send_timeout 3600s;
}
${finish}`;
const before = host.advanced_config || "";
const unchangedFields = invariants(host);
const addition = before.trimEnd() + "\n\n" + snippet + "\n";
if (mode === "inspect") {
  console.log(
    JSON.stringify({
      id,
      hostname: host.domain_names[0],
      upstream: `${host.forward_host}:${host.forward_port}`,
      adapterRoute: before.includes(start),
      advancedHash: hash(before),
    }),
  );
  process.exit(0);
}
if (mode === "prepare") {
  if (before.includes(start) || /location\s+(?:=\s+)?\/mcp\b/.test(before))
    throw new Error("An MCP-specific route already exists");
  // Verify a complete independent nginx config without changing any live files.
  const candidate = `/tmp/omni-executor-events-${process.pid}.conf`;
  const standalone = `user root; pid /tmp/omni-executor-events-test.pid; events {} http { server { listen 127.0.0.1:47890;\n${snippet}\n} }`;
  await writeFile(candidate, standalone, { mode: 0o600 });
  execFileSync("nginx", ["-t", "-c", candidate], { stdio: "pipe" });
  const prepared = {
    id,
    before,
    after: addition,
    beforeHash: hash(before),
    afterHash: hash(addition),
  };
  try {
    await writeFile(backupPath, JSON.stringify(prepared), { mode: 0o600, flag: "wx" });
  } catch (error) {
    if (error.code !== "EEXIST") throw error;
    const saved = JSON.parse(await readFile(backupPath, "utf8"));
    if (
      saved.beforeHash !== prepared.beforeHash ||
      saved.afterHash !== prepared.afterHash
    )
      throw new Error("Existing route backup differs");
  }
  // Rollback is the exact original string, including whitespace and unrelated directives.
  if (hash(prepared.before) !== hash(before))
    throw new Error("Rollback roundtrip failed");
  console.log(
    JSON.stringify({
      prepared: true,
      syntax: "passed",
      rollback: "exact original configuration",
      beforeHash: prepared.beforeHash,
      afterHash: prepared.afterHash,
    }),
  );
  process.exit(0);
}
const saved = JSON.parse(await readFile(backupPath, "utf8"));
if (
  saved.id !== id ||
  hash(saved.before) !== saved.beforeHash ||
  hash(saved.after) !== saved.afterHash ||
  !saved.after.includes(snippet)
)
  throw new Error("Invalid route backup");
const wanted = mode === "apply" ? saved.after : saved.before;
const expected = mode === "apply" ? saved.before : saved.after;
if (before === wanted) {
  console.log(JSON.stringify({ mode, unchanged: true }));
  process.exit(0);
}
if (before !== expected)
  throw new Error("Concurrent route edit detected; refusing to overwrite it");
const signed = await Token().create({
  iss: "local-maintenance",
  attrs: { id: host.owner_user_id },
  scope: ["user"],
  expiresIn: "5m",
});
const access = new Access(signed.token);
await access.can("proxy_hosts:update", id);
const updated = await proxyHosts.update(access, { id, advanced_config: wanted });
if (updated.meta?.nginx_online === false)
  throw new Error("NPM rejected nginx configuration; inspect and rollback");
execFileSync("nginx", ["-t"], { stdio: "pipe" });
const verified = await ProxyHost.query().findById(id);
if (verified.advanced_config !== wanted) throw new Error("Route read-back mismatch");
if (invariants(verified) !== unchangedFields)
  throw new Error("Unexpected change outside the MCP route");
console.log(
  JSON.stringify({
    mode,
    verified: true,
    advancedHash: hash(wanted),
    unchangedUpstream: `${verified.forward_host}:${verified.forward_port}`,
  }),
);
process.exit(0);
