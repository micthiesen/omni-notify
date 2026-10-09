/**
 * Rollback gate: decodes every `blobs` row of the original database and of the
 * Rust-rewritten copy with mitools `decodeDoc` (node-cbor) and diffs the JS values
 * (via the lossless JS view; `undefined` properties are compared as absent, as TS
 * reads them). Also diffs the entity/version/expires_at columns. Exits 1 on any diff.
 *
 * Usage: tsx crates/xtask/node/node-readback.ts <original.db> <rewritten.db>
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

function view(row: Row): string {
  if (!row.data) return "<null data>";
  try {
    return JSON.stringify(stripUndefined(jsView(decodeDoc(row.data))));
  } catch (error) {
    return `<corrupt: ${(error as Error).message}>`;
  }
}

const [originalPath, rewrittenPath] = process.argv.slice(2);
if (!originalPath || !rewrittenPath) throw new Error("usage: node-readback.ts <original.db> <rewritten.db>");
const original = load(originalPath);
const rewritten = load(rewrittenPath);
const diffs: string[] = [];
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
  if (a !== b) diffs.push(`${pk}: value differs\n  original:  ${a.slice(0, 400)}\n  rewritten: ${b.slice(0, 400)}`);
}
for (const pk of rewritten.keys()) if (!original.has(pk)) diffs.push(`extra in rewritten: ${pk}`);
console.log(`compared ${original.size} rows; ${diffs.length} differences`);
for (const diff of diffs.slice(0, 200)) console.log(diff);
process.exit(diffs.length === 0 ? 0 : 1);
