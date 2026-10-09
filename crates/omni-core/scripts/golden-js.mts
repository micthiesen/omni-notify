// Generates crates/omni-core/tests/golden/js.json: V8 reference outputs for
// omni_core::js and omni_core::digest. Run from the repository root:
//   node_modules/.bin/tsx crates/omni-core/scripts/golden-js.mts
import { writeFileSync } from "node:fs";
import { fingerprintEvidence, digest } from "../../../src/utils/fingerprint.ts";

const out: Record<string, unknown> = {};
const bitsOf = (n: number): string => {
  const b = Buffer.alloc(8);
  b.writeDoubleBE(n);
  return b.toString("hex");
};
const persisted = (s: string): string => Buffer.from(s, "utf8").toString("utf8");

// Deterministic PRNG (mulberry32).
let seed = 0x5eed1234;
const rand = (): number => {
  seed |= 0;
  seed = (seed + 0x6d2b79f5) | 0;
  let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
  t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
};

out.locale = Intl.DateTimeFormat().resolvedOptions().locale;

// utf16 length + slice (persisted form: lone surrogates become U+FFFD).
const utf16Strings = ["", "hello", "é", "😀", "a😀b", "😀😀", "x👩‍👩‍👧y", "日本語テキスト", "\u0000\u001f", "a\u{10FFFF}b"];
out.utf16 = utf16Strings.map((s) => {
  const slices: [number, number, string][] = [];
  for (let a = 0; a <= s.length + 1; a++) {
    for (let b = 0; b <= s.length + 1; b++) slices.push([a, b, persisted(s.slice(a, b))]);
  }
  return { s, len: s.length, slices };
});

// Number#toString.
const numbers: number[] = [0, -0, 1, -1, 0.1, 0.2, 0.3, 1 / 3, 2 / 3, 100, 1e21, 1e-7, 1.5e-7, 123e-20,
  1e20, 123456789012345680000, 5e-324, -5e-324, Number.MAX_VALUE, Number.MIN_VALUE,
  Number.MAX_SAFE_INTEGER, Number.MAX_SAFE_INTEGER + 2, NaN, Infinity, -Infinity, 0.000001, 0.0000001,
  1.7976931348623157e308, 4.35, 0.1 + 0.2, 1e300 * 10, 2 ** 53, 2 ** 64, 2 ** -1074, 1.005, 299792458,
  1760000000123, 0.62, 3.14159, 1e6, 1.25e-6, 9.999999999999999e20, 1e+21 - 65537];
for (let i = 0; i < 1500; i++) {
  const b = Buffer.alloc(8);
  b.writeUInt32BE(Math.floor(rand() * 2 ** 32), 0);
  b.writeUInt32BE(Math.floor(rand() * 2 ** 32), 4);
  numbers.push(b.readDoubleBE(0));
}
for (let i = 0; i < 1500; i++) {
  const scale = 10 ** Math.floor(rand() * 30 - 15);
  numbers.push(Math.round(rand() * 1e6) / 1e3 * scale);
  numbers.push(Math.floor(rand() * 2 ** 53) * (rand() < 0.5 ? -1 : 1));
}
out.numbers = numbers.map((n) => [bitsOf(n), String(n)]);

// JSON.stringify (input is JSON text so serde_json can parse it in insertion order).
const jsonTexts = [
  `{"b":1,"a":[true,null,"x\\n\\u0001"],"2":2.5,"10":{},"01":[]}`,
  `{"z":{"y":{"x":[1,2,{"w":"v"}]}},"4294967294":1,"4294967295":2,"-1":3,"1.5":4,"0":5}`,
  `[1.0, 1e21, 1e-7, 12345678901234567890, -0, 0.1, 2.5e-324]`,
  `"\\u2028\\u2029\\u007f\\b\\f\\t\\r\\"\\\\/"`,
  `{"emoji":"😀","escaped":"\\ud83d\\ude00","ctrl":"\\u001b[0m"}`,
  `[]`, `{}`, `[[],{},[{}]]`, `null`, `true`, `"plain"`, `-12.5e3`,
  `{"nested":{"arr":[{"k":"v","n":null},[1,[2,[3]]]],"s":"a b c"}}`,
];
out.stringify = jsonTexts.map((text) => {
  const v = JSON.parse(text);
  return { text, compact: JSON.stringify(v), pretty2: JSON.stringify(v, null, 2) };
});

// localeCompare (default locale, as the production process uses).
const words = ["", "a", "A", "b", "B", "á", "Á", "ä", "à", "ab", "aB", "Ab", "a b", "a-b", "a_b", "ab1", "ab10", "ab2",
  "1", "10", "2", "z", "Z", "ß", "ss", "æ", "ae", "ø", "o", "œ", "oe", "ñ", "n", "ç", "c", "é", "e", "ê", "日本",
  "中文", "한국어", "Привет", "привет", "Ελληνικά", "😀", "😁", "#tag", "@user", "evidence-1", "evidence-10",
  "evidence-2", "evidence_1", "Evidence-1", "listen", "Listen", "show A", "Show A", "Show B", "zebra", "Zebra",
  " x", " x", "x", "ﬁ", "fi", "Å", "Å", "co-op", "coop", "co op", "naïve", "naive"];
out.localeWords = words;
out.localeCompare = words.map((a) => words.map((b) => Math.sign(a.localeCompare(b))));

// encodeURIComponent.
const uriStrings = ["", "abc", "a b/c?d=é!*'()", "~-_.", "😀", "100%", "#&+=;,:@$", "日本", "\n\t", "[]{}|^`\"<>\\"];
out.encodeURIComponent = uriStrings.map((s) => [s, encodeURIComponent(s)]);

// Date#toISOString.
const isoMs = [0, 1, -1, 999, 1760000000123, -62198755200000, -62135596800000, 253402300799999, 253402300800000,
  8640000000000000, -8640000000000000, 951782400000, 4107542400000, -2208988800001];
out.toISOString = isoMs.map((ms) => [ms, new Date(ms).toISOString()]);

// Number(string).
const numberStrings = ["", " ", " 12 ", "0x10", "0X1f", "0b11", "0o17", "0B2", "1e3", "1E-3", "12abc", "Infinity",
  "-Infinity", "+Infinity", "infinity", "NaN", "-0", "+5", "-5", ".5", "5.", ".", "-.5", "1_000", "\t7\n",
  " 7 ", "﻿8", "1n", "0x", "-0x10", "+0x10", "00012", "1.5e+2", "1e", "e5", "1e400", "-1e400",
  "0.1", "123456789012345678901234567890", "4.9e-324", "2e-324", "0x1fffffffffffff", "0xffffffffffffffffff",
  "١٢", "12 34", "+-1", "--1", "1.2.3", "  -12.50e1  "];
out.stringToNumber = numberStrings.map((s) => [s, bitsOf(Number(s))]);

// digest + fingerprintEvidence (src/utils/fingerprint.ts).
out.digest = ["", "abc", "😀", "line1\nline2"].map((s) => [s, digest(s)]);
const evidenceSets: Record<string, unknown>[][] = [
  [
    { evidenceId: "b", kind: "listen", showTitle: "Show B", observedAt: 200, completion: undefined },
    { evidenceId: "a", kind: "listen", showTitle: "Show A", observedAt: 100, starred: true },
  ],
  [
    { evidenceId: "evidence-10", z: 1, a: null, B: "x", b: [1, { y: 2, x: 1 }], "10": 1, "2": 2 },
    { evidenceId: "Evidence-1", text: "é😀\n\"q\"", n: 0.1, big: 1e21 },
    { evidenceId: "evidence-2", nested: { b: 1, a: 2 } },
    { evidenceId: "evidence_1", skipped: undefined },
  ],
  [],
];
out.fingerprint = evidenceSets.map((items) => ({
  // undefined fields are dropped from the Rust input; TS drops them too.
  items: JSON.parse(JSON.stringify(items)),
  expected: fingerprintEvidence(items as { evidenceId: string }[]),
}));

writeFileSync(new URL("../tests/golden/js.json", import.meta.url), `${JSON.stringify(out)}\n`);
console.log("wrote js.json");
