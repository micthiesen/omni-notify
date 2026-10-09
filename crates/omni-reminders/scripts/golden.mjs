// Regenerates crates/omni-reminders/tests/golden/*.json from the TypeScript
// implementation. Run from the repository root:
//   node --import tsx crates/omni-reminders/scripts/golden.mjs
// The output is deterministic except for AES-GCM nonces and cookie creation
// times, which the Rust tests read back rather than recompute.
import { createHash } from "node:crypto";
import { mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { inflateSync } from "node:zlib";
import { Effect } from "effect";
import { CookieJar } from "tough-cookie";
import { createRemindersStore } from "../../../src/reminders/store.ts";
import { encodeCrdtDocument } from "../../../src/reminders/codec.ts";
import { GSASRPAuthenticator } from "../../../src/reminders/apple.ts";
import { encodeRecurrenceValues } from "../../../src/reminders/recurrence.ts";

const out = path.join(import.meta.dirname, "..", "tests", "golden");
const KEY = "0123456789abcdef".repeat(4);
const ACCOUNT = "Golden.Fixture@Example.com";

// Cookie jar: domain, host-only, max-age, expires and path-scoped cookies.
const jar = new CookieJar();
jar.setCookieSync(
  "X-APPLE-WEBAUTH-PCS-Cloudkit=pcs-cookie; Domain=icloud.com; Path=/; Secure; HttpOnly; SameSite=None",
  "https://setup.icloud.com/setup/ws/1/requestPCS",
);
jar.setCookieSync("aasp=auth; Domain=apple.com; Path=/; Secure; HttpOnly", "https://idmsa.apple.com/appleauth/auth/signin/init");
jar.setCookieSync("host-only=1; Max-Age=86400000", "https://setup.icloud.com/setup/ws/1/validate");
jar.setCookieSync("scoped=2; Expires=Wed, 21 Oct 2099 07:28:00 GMT; Path=/database", "https://p01-ckdatabasews.icloud.com/database/1/x");
jar.setCookieSync("expired=3; Expires=Wed, 21 Oct 2015 07:28:00 GMT", "https://setup.icloud.com/");
const cookieUrls = [
  "https://setup.icloud.com/setup/ws/1/validate",
  "https://setup.icloud.com/other",
  "https://p01-ckdatabasews.icloud.com/database/1/com.apple.reminders/production/private/records/query",
  "https://idmsa.apple.com/appleauth/auth/signin/init",
  "https://www.icloud.com/",
  "http://setup.icloud.com/setup/ws/1/validate",
];
const cookieHeaders = Object.fromEntries(
  cookieUrls.map((url) => [url, jar.getCookieStringSync(url)]),
);
const serializedJar = jar.serializeSync();

function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === "object")
    return Object.fromEntries(
      Object.entries(value)
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([key, item]) => [key, canonical(item)]),
    );
  return value;
}
const fingerprint = (value) =>
  createHash("sha256").update(JSON.stringify(canonical(value))).digest("hex");
const fingerprintCases = [
  "stable-request-key-1",
  { operation: "create", listId: "List/test", title: "Milk" },
  { operation: "create", listId: "List/a", title: "Ä b", dueDate: null, priority: 5, allDay: true, description: "x y" },
  { operation: "update-recurrence", id: "Reminder/1", changeTag: "t", ruleId: "RecurrenceRule/1", ruleChangeTag: "r", patch: { endDate: null, daysOfWeek: [{ dayOfTheWeek: 2, weekNumber: -1 }], Interval: 3, interval: 2, _a: 1, a: 2, B: 3, b: 4, "10": 5, "2": 6 } },
  { operation: "complete-recurring", id: "Reminder/1", changeTag: "t", ruleId: "RecurrenceRule/1", ruleChangeTag: "r", timeZone: "America/Vancouver" },
].map((input) => ({ input, fingerprint: fingerprint(input) }));

const state = {
  version: 1,
  session: {
    clientId: "auth-00000000-0000-4000-8000-000000000000",
    scnt: "scnt-value",
    sessionId: "session-id",
    sessionToken: "session-token",
    trustToken: "trust-token",
    accountCountry: "CAN",
    cookies: JSON.stringify(serializedJar),
    ckBaseUrl: "https://p01-ckdatabasews.icloud.com/database/1/com.apple.reminders/production/private",
    dsid: "123456789",
  },
  notified: true,
  operations: {
    [fingerprint("confirmed-key-0000001")]: {
      fingerprint: fingerprint({ operation: "delete", id: "Reminder/A", changeTag: "t" }),
      recordId: "Reminder/A",
      state: "confirmed",
      result: { id: "Reminder/A", deleted: true, verified: true },
    },
    [fingerprint("reserved-key-00000002")]: {
      fingerprint: fingerprint({ operation: "create", listId: "List/test", title: "Milk" }),
      recordId: `Reminder/${fingerprint("reserved-key-00000002").slice(0, 32).toUpperCase()}`,
      state: "reserved",
    },
  },
};
const dir = await mkdtemp(path.join(os.tmpdir(), "omni-reminders-golden-"));
try {
  await Effect.runPromise(createRemindersStore(dir, KEY, ACCOUNT).write(state));
  const [name] = await readdir(dir);
  const bytes = await readFile(path.join(dir, name));
  await writeFile(
    path.join(out, "store.json"),
    `${JSON.stringify(
      {
        key: KEY,
        account: ACCOUNT,
        filename: name,
        ciphertextBase64: bytes.toString("base64"),
        state,
        cookieHeaders,
      },
      null,
      2,
    )}\n`,
  );
} finally {
  await rm(dir, { recursive: true, force: true });
}

// CRDT documents: the inflated protobuf is deterministic.
const crdt = ["", "Buy milk", "ADP reminder 🌿", "Title second line paragraph\u0001\t🙂"].map(
  (text) => ({
    text,
    protobufBase64: inflateSync(Buffer.from(encodeCrdtDocument(text), "base64")).toString("base64"),
    encodedBase64: encodeCrdtDocument(text),
  }),
);

// SRP with fixed client and server secrets.
const N = BigInt(
  "0xac6bdb41324a9a9bf166de5e1389582faf72b6651987ee07fc3192943db56050a37329cbb4a099ed8193e0757767a13dd52312ab4b03310dcd7f48a9da04fd50e8083969edb767b0cf6095179a163ab3661a05fbd5faaae82918a9962f0b93b855f97993ec975eeaa80d740adbf4ff747359d041d5c33ea71d281e446b14773bca97b43a23fb801676bd207a436c6481f1d2b9078717461a5b9d32e688f87748544523b524b0d57d5ea77a2775d2ecfa032cfbdbf52fb3786160279004e57ae6af874e7303ce53299ccc041c7bc308d82a5698f3a8d0c38271ae35f8e9dbfbb694b5c803d89f7ae435de236d525f54759b65e372fcd68ef20fa7111f9e4aff73",
);
const modPow = (base, exp, mod) => {
  let result = 1n;
  base %= mod;
  for (; exp > 0n; exp >>= 1n) {
    if (exp & 1n) result = (result * base) % mod;
    base = (base * base) % mod;
  }
  return result;
};
const toB64 = (n) => {
  let hex = n.toString(16);
  if (hex.length % 2) hex = `0${hex}`;
  return Buffer.from(hex, "hex").toString("base64");
};
const srp = [];
for (const protocol of ["s2k", "s2k_fo"]) {
  const a = BigInt(`0x${createHash("sha256").update(`client-${protocol}`).digest("hex").repeat(8)}`);
  const b = BigInt(`0x${createHash("sha256").update(`server-${protocol}`).digest("hex").repeat(8)}`);
  const B = modPow(2n, b, N);
  const auth = new GSASRPAuthenticator("golden@example.com");
  auth.clienta = a;
  auth.clientA = modPow(2n, a, N);
  const server = {
    protocol,
    iteration: 1000,
    salt: Buffer.from("golden-salt-0123").toString("base64"),
    b: toB64(B),
    c: "opaque-c",
  };
  const proof = await auth.getComplete("correct horse battery", server);
  srp.push({ protocol, a: a.toString(16), A: toB64(auth.clientA), server, password: "correct horse battery", account: "golden@example.com", proof });
}

const recurrence = [
  { frequency: "yearly", interval: 2, occurrenceCount: 0, firstDayOfWeek: 2, endDate: 1800000000000, daysOfWeek: [{ dayOfTheWeek: 2, weekNumber: 0 }, { dayOfTheWeek: 7 }], daysOfMonth: [1, -1], daysOfYear: [1, -366], weeksOfYear: [1, -53], monthsOfYear: [1, 12], setPositions: [1, -1] },
  { frequency: "monthly", interval: 1, daysOfWeek: [{ dayOfTheWeek: 2, weekNumber: -1 }] },
].map((rule) => ({ rule, encoded: encodeRecurrenceValues(rule) }));

await writeFile(
  path.join(out, "codec.json"),
  `${JSON.stringify({ crdt, srp, recurrence, fingerprints: fingerprintCases }, null, 2)}\n`,
);
