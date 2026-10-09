/**
 * Rollback gate: decodes every `blobs` row of the original database and of the
 * Rust-rewritten copy with mitools `decodeDoc` (node-cbor) and diffs the JS values
 * (via the lossless JS view; `undefined` properties are compared as absent, as TS
 * reads them). Also diffs the entity/version/expires_at columns. Exits 1 on any value or
 * column diff. Property order is compared separately: rows whose values are equal but
 * whose key order differs (Rust struct order vs TS insertion order) are summarized per
 * entity with the first object path whose order changed, and fail only with --strict-order.
 *
 * Usage: tsx crates/xtask/node/node-readback.ts <original.db> <rewritten.db> [--strict-order]
 */
import { createRequire } from "node:module";
import { decodeDoc } from "@micthiesen/mitools/docstore";
import { jsView } from "./js-view.js";

// better-sqlite3 is a mitools dependency, resolved from mitools' own location.
const requireFromMitools = createRequire(import.meta.resolve("@micthiesen/mitools/docstore"));
const Database = requireFromMitools("better-sqlite3") as typeof import("better-sqlite3");

type Row = { pk: string; data: Buffer | null; entity: string | null; version: number; expires_at: number | null };

function load(path: string): Map<string, Row> {
  const db = new Database(path, { readonly: true, fileMustExist: true });
  const rows = db.prepare("SELECT pk, data, entity, version, expires_at FROM blobs").all() as Row[];
  db.close();
  return new Map(rows.map((row) => [row.pk, row]));
}

function stripUndefined(view: unknown): unknown {
  if (Array.isArray(view)) return view.map(stripUndefined);
  if (view && typeof view === "object") {
    return Object.fromEntries(
      Object.entries(view)
        .filter(([, v]) => !(v && typeof v === "object" && "$undefined" in v && Object.keys(v).length === 1))
        .map(([k, v]) => [k, stripUndefined(v)]),
    );
  }
  return view;
}

function sortKeys(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sortKeys);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((k) => [k, sortKeys((value as Record<string, unknown>)[k])]),
    );
  }
  return value;
}

/** First object path whose own key order differs (values already known equal). */
function orderPath(a: unknown, b: unknown, path: string): string | null {
  if (Array.isArray(a) && Array.isArray(b)) {
    for (let i = 0; i < a.length; i++) {
      const found = orderPath(a[i], b[i], `${path}/${i}`);
      if (found) return found;
    }
    return null;
  }
  if (a && b && typeof a === "object" && typeof b === "object") {
    const ka = Object.keys(a);
    const kb = Object.keys(b);
    if (ka.join("\u0000") !== kb.join("\u0000")) return `${path || "/"} [${ka.join(",")}] vs [${kb.join(",")}]`;
    for (const k of ka) {
      const found = orderPath((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k], `${path}/${k}`);
      if (found) return found;
    }
  }
  return null;
}

type View = { ok: true; value: unknown } | { ok: false; text: string };

function view(row: Row): View {
  if (!row.data) return { ok: false, text: "<null data>" };
  try {
    return { ok: true, value: stripUndefined(jsView(decodeDoc(row.data))) };
  } catch (error) {
    return { ok: false, text: `<corrupt: ${(error as Error).message}>` };
  }
}

const args = process.argv.slice(2);
const strictOrder = args.includes("--strict-order");
const [originalPath, rewrittenPath] = args.filter((a) => !a.startsWith("--"));
if (!originalPath || !rewrittenPath) throw new Error("usage: node-readback.ts <original.db> <rewritten.db> [--strict-order]");
const original = load(originalPath);
const rewritten = load(rewrittenPath);
const diffs: string[] = [];
const orderOnly = new Map<string, { rows: number; paths: Map<string, number> }>();
for (const [pk, row] of original) {
  const other = rewritten.get(pk);
  if (!other) {
    diffs.push(`missing in rewritten: ${pk}`);
    continue;
  }
  for (const column of ["entity", "version", "expires_at"] as const) {
    if (row[column] !== other[column]) diffs.push(`${pk}: ${column} ${row[column]} != ${other[column]}`);
  }
  const a = view(row);
  const b = view(other);
  if (!a.ok || !b.ok) {
    const at = a.ok ? JSON.stringify(a.value) : a.text;
    const bt = b.ok ? JSON.stringify(b.value) : b.text;
    if (at !== bt) diffs.push(`${pk}: value differs\n  original:  ${at.slice(0, 400)}\n  rewritten: ${bt.slice(0, 400)}`);
    continue;
  }
  const sa = JSON.stringify(sortKeys(a.value));
  const sb = JSON.stringify(sortKeys(b.value));
  if (sa !== sb) {
    diffs.push(`${pk}: value differs\n  original:  ${sa.slice(0, 400)}\n  rewritten: ${sb.slice(0, 400)}`);
    continue;
  }
  const path = orderPath(a.value, b.value, "");
  if (path) {
    const entity = row.entity ?? "<null>";
    const entry = orderOnly.get(entity) ?? { rows: 0, paths: new Map() };
    entry.rows++;
    const shape = path.replace(/\/\d+(?=\/|\s|$)/g, "/#");
    entry.paths.set(shape, (entry.paths.get(shape) ?? 0) + 1);
    orderOnly.set(entity, entry);
  }
}
for (const pk of rewritten.keys()) if (!original.has(pk)) diffs.push(`extra in rewritten: ${pk}`);
const orderRows = [...orderOnly.values()].reduce((n, e) => n + e.rows, 0);
console.log(`compared ${original.size} rows; ${diffs.length} value/column differences; ${orderRows} rows differ only in property order`);
for (const diff of diffs.slice(0, 200)) console.log(diff);
for (const [entity, entry] of [...orderOnly].sort()) {
  console.log(`order-only ${entity}: ${entry.rows} rows`);
  for (const [path, n] of [...entry.paths].sort((x, y) => y[1] - x[1]).slice(0, 5)) console.log(`  ${n}x ${path}`);
}
process.exit(diffs.length === 0 && (!strictOrder || orderRows === 0) ? 0 : 1);
