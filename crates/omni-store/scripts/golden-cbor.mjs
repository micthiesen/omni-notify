// Generates crates/omni-store/tests/golden/cbor.json from node-cbor through
// mitools' own encodeDoc/decodeDoc (the production code path). Run from the
// repository root:  cargo xtask golden-cbor  (or node crates/omni-store/scripts/golden-cbor.mjs
// [OUT], OUT defaulting to crates/omni-store/tests/golden/cbor.json)
//
// Deliberately absent (documented divergences in omni_store::cbor::decode):
// tag 32 URLs and tag 35 RegExps (node normalizes the text), odd-length
// big-endian typed arrays (node corrupts the input while failing), BigInts
// wider than 127 bits, and nesting deeper than cbor::MAX_DEPTH.
import { writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { gzipSync } from "node:zlib";
import { decodeDoc, encodeDoc } from "@micthiesen/mitools/docstore";

const require = createRequire(import.meta.resolve("@micthiesen/mitools/docstore"));
const cbor = require("cbor");
const { Tagged, Simple } = cbor;

const bitsOf = (n) => {
  const b = Buffer.alloc(8);
  b.writeDoubleBE(n);
  return b.toString("hex");
};
const persisted = (s) => Buffer.from(s, "utf8").toString("utf8");

// node's _pushTypedArray tag rule (little-endian host).
function typedTag(arr) {
  let typ = 0b01000000;
  let sz = arr.BYTES_PER_ELEMENT;
  const name = arr.constructor.name;
  if (name.startsWith("Float")) {
    typ |= 0b00010000;
    sz /= 2;
  } else if (!name.includes("U")) {
    typ |= 0b00001000;
  }
  if (name.includes("Clamped") || sz !== 1) typ |= 0b00000100;
  typ |= { 1: 0, 2: 1, 4: 2, 8: 3 }[sz];
  return typ;
}

function describe(v) {
  if (v === undefined) return { t: "undefined" };
  if (v === null) return { t: "null" };
  switch (typeof v) {
    case "boolean":
      return { t: "bool", v };
    case "number":
      return { t: "num", bits: bitsOf(v) };
    case "bigint":
      return { t: "bigint", v: v.toString() };
    case "string":
      return { t: "str", v: persisted(v) };
  }
  if (Buffer.isBuffer(v)) return { t: "bytes", hex: v.toString("hex") };
  if (ArrayBuffer.isView(v)) {
    return { t: "typed", tag: typedTag(v), hex: Buffer.from(v.buffer, v.byteOffset, v.byteLength).toString("hex") };
  }
  if (v instanceof Date) return { t: "date", bits: bitsOf(v.getTime()) };
  if (v instanceof Set) return { t: "set", items: [...v].map(describe) };
  if (v instanceof Map) return { t: "map", entries: [...v].map(([k, x]) => [describe(k), describe(x)]) };
  if (Array.isArray(v)) return { t: "arr", items: v.map(describe) };
  if (v instanceof Tagged) return { t: "tagged", tag: v.tag, v: describe(v.value) };
  if (v instanceof Simple) return { t: "simple", v: v.value };
  if (Object.getPrototypeOf(v) === Object.prototype) {
    return { t: "obj", entries: Object.keys(v).map((k) => [k, describe(v[k])]) };
  }
  throw new Error(`cannot describe ${Object.prototype.toString.call(v)}`);
}

const bytes = (n) => Buffer.from(Array.from({ length: n }, (_, i) => (i * 37) & 0xff));
const MAX = Number.MAX_SAFE_INTEGER;

const encodeCases = {
  "int:0": 0, "int:-0": -0, "int:1": 1, "int:-1": -1, "int:23": 23, "int:24": 24, "int:-24": -24,
  "int:-25": -25, "int:255": 255, "int:256": 256, "int:65535": 65535, "int:65536": 65536,
  "int:u32max": 2 ** 32 - 1, "int:2^32": 2 ** 32, "int:max-safe": MAX, "int:max-safe+1": MAX + 1,
  "int:-max-safe": -MAX, "int:min-safe-1": -(MAX + 1), "int:2^64": 2 ** 64, "int:-2^64": -(2 ** 64),
  "int:1e300": 1e300, "int:epoch-ms": 1760000000123,
  "float:0.5": 0.5, "float:0.1": 0.1, "float:1/3": 1 / 3, "float:fround(0.1)": Math.fround(0.1),
  "float:f32max": 3.4028234663852886e38, "float:f32-subnormal": 1.401298464324817e-45,
  "float:f64-subnormal": 5e-324, "float:-2.5": -2.5, "float:epoch-ms+0.5": 1760000000123.5,
  "float:1e-7": 1e-7, "float:NaN": NaN, "float:Infinity": Infinity, "float:-Infinity": -Infinity,
  "bigint:0": 0n, "bigint:1": 1n, "bigint:-1": -1n, "bigint:2^64": 2n ** 64n, "bigint:-2^64": -(2n ** 64n),
  "bigint:2^64-1": 2n ** 64n - 1n, "bigint:2^100": 2n ** 100n, "bigint:-2^100-5": -(2n ** 100n) - 5n,
  "bigint:255": 255n, "bigint:256": 256n,
  "str:empty": "", "str:a": "a", "str:23": "x".repeat(23), "str:24": "x".repeat(24),
  "str:255": "y".repeat(255), "str:256": "y".repeat(256), "str:utf8": "é日本語😀",
  "str:lone-surrogate": "a\ud83d", "str:70000": "z".repeat(70000), "str:nul": "\u0000",
  "date:0": new Date(0), "date:1000": new Date(1000), "date:1500": new Date(1500),
  "date:epoch-ms": new Date(1760000000123), "date:-1": new Date(-1), "date:-1500": new Date(-1500),
  "date:ms-trunc": new Date(1.9), "date:invalid": new Date(NaN), "date:max": new Date(8.64e15),
  "date:sub-ms-float": new Date(1700000000001), "date:y2038": new Date(2 ** 31 * 1000),
  "simple:undefined": undefined, "simple:null": null, "simple:true": true, "simple:false": false,
  "simple:16": new Simple(16), "simple:255": new Simple(255),
  "bytes:empty": Buffer.alloc(0), "bytes:3": bytes(3), "bytes:300": bytes(300),
  "typed:u8": new Uint8Array([1, 2, 3]), "typed:u16": new Uint16Array([1, 0x0102]),
  "typed:i32": new Int32Array([-1, 7]), "typed:f64": new Float64Array([1.5, -0]),
  "typed:clamped": new Uint8ClampedArray([9, 255]), "typed:i8": new Int8Array([-5]),
  "typed:f32": new Float32Array([0.25]),
  "set:mixed": new Set([1, "a", null, undefined]), "set:empty": new Set(),
  "set:nested": new Set([[1, 2], new Set(["x"])]),
  "map:int-keys": new Map([[1, "a"], ["b", 2]]), "map:string-keys": new Map([["a", 1], ["b", 2]]),
  "map:object-key": new Map([[{ k: 1 }, "v"]]), "map:undefined-key": new Map([[undefined, 1]]),
  "map:empty": new Map(), "map:date-key": new Map([[new Date(5000), true]]),
  "arr:empty": [], "arr:nested": [[[]], [1, [2, [3]]]], "arr:24": Array.from({ length: 24 }, (_, i) => i),
  "arr:300": Array.from({ length: 300 }, (_, i) => i * 1.5), "arr:holes-undefined": [undefined, null],
  "obj:empty": {}, "obj:order": { b: 1, a: 2 },
  "obj:index-keys": JSON.parse('{"b":3,"10":2,"2":1,"01":4,"-1":5,"4294967295":6,"4294967294":7,"1.5":8}'),
  "obj:undefined-field": { present: 1, missing: undefined, nul: null },
  "obj:proto-key": JSON.parse('{"__proto__":1,"a":2}'),
  "obj:24-keys": Object.fromEntries(Array.from({ length: 24 }, (_, i) => [`k${i}`, i])),
  "tagged:1000": new Tagged(1000, "x"), "tagged:embedded": new Tagged(24, Buffer.from([1])),
  "tagged:decimal": new Tagged(4, [-2, 27315]), "tagged:big-tag": new Tagged(2 ** 31 - 1, null),
  "doc:streamer-status": {
    streamerId: "destiny", live: true, viewers: 12345, peak: 2 ** 40, startedAt: new Date(1760000000123),
    titles: ["a", "b"], primary: { platform: "youtube", channelId: "@destiny" }, ratio: 0.62,
    pending: undefined, lastError: null, tags: new Set(["x"]),
  },
  "doc:cost-event": {
    id: "9f6c3c7e-1b2d-4b9a-9d55-0d5c1f0a2b3c", at: 1760000000123, feature: "briefing", operation: "generate",
    model: "openai:gpt-6-luna", inputTokens: 1200, outputTokens: 345, costCents: null, runId: undefined,
  },
};

const out = { encode: [], decode: [], logsGz: [] };
for (const [name, value] of Object.entries(encodeCases)) {
  const encoded = encodeDoc(value);
  const decoded = decodeDoc(encoded);
  out.encode.push({
    name,
    value: describe(value),
    hex: encoded.toString("hex"),
    decoded: describe(decoded),
    reencoded: encodeDoc(decoded).toString("hex"),
  });
}

const decodeCases = {
  "f16:1": "f93c00", "f16:1.5": "f93e00", "f16:max": "f97bff", "f16:min-subnormal": "f90001",
  "f16:-0": "f98000", "f16:inf": "f97c00", "f16:-inf": "f9fc00", "f16:nan": "f97e00", "f16:nan-payload": "f97e01",
  "f32:nan-payload": "fa7fc00001", "f64:nan-payload": "fb7ff8000000000001", "f32:integral": "fa47c35000",
  "f64:integral": "fb3ff0000000000000", "f64:-0": "fb8000000000000000",
  "uint:non-minimal-1": "1800", "uint:non-minimal-2": "190001", "uint:non-minimal-8": "1b0000000000000001",
  "uint:2^53": "1b0020000000000000", "uint:u64max": "1bffffffffffffffff", "nint:-2^53": "3b001fffffffffffff",
  "nint:-2^53+1": "3b001ffffffffffffe", "nint:-2^64": "3bffffffffffffffff",
  "indef:array": "9f0102ff", "indef:empty-array": "9fff", "indef:nested": "9f9f01ffff",
  "indef:map": "bf616101ff", "indef:empty-map": "bfff", "indef:bytes": "5f42010243030405ff",
  "indef:text": "7f616161626163ff", "indef:empty-text": "7fff", "indef:in-definite": "829f01ff02",
  "tag0:iso": `c078${(24).toString(16)}${Buffer.from("2020-01-02T03:04:05.678Z").toString("hex")}`,
  "tag0:date-only": `c06a${Buffer.from("2020-01-02").toString("hex")}`,
  "tag0:offset": `c079${(25).toString(16).padStart(4, "0")}${Buffer.from("2020-01-02T03:04:05+02:00").toString("hex")}`,
  "tag0:garbage": `c067${Buffer.from("garbage").toString("hex")}`,
  "tag0:number": "c01903e8",
  "tag1:int": "c11a5f5e1000", "tag1:float": "c1fb41d958a0a8c7df3b", "tag1:null": "c1f6",
  "tag1:text": "c163616263", "tag1:numeric-text": "c1623132", "tag1:true": "c1f5", "tag1:undefined": "c1f7",
  "tag1:bigint": "c11b7fffffffffffffff", "tag1:nan": "c1fb7ff8000000000000", "tag1:too-big": "c11b000009184e72a000",
  "tag1:negative-float": "c1f9bc00", "tag1:array": "c180",
  "tag2:1": "c24101", "tag2:leading-zero": "c2420001", "tag2:empty": "c240", "tag2:zero": "c24100",
  "tag2:i128max": "c2507fffffffffffffffffffffffffffffff", "tag2:in-array": "82c2410105",
  "tag3:-1": "c34100", "tag3:-257": "c3420100",
  "set:dup": "d9010283010101", "set:null": "d90102f6", "set:number": "d9010201", "set:text": "d90102626162",
  "set:float-int-dup": "d9010282f93c0001", "set:bytes": "d90102420101",
  "typed:u8": "d84043010203", "typed:u16-be": "d8414400010002", "typed:u16-le": "d8454401000200",
  "typed:clamped": "d84443010203", "typed:f64-be": "d852483ff8000000000000",
  "typed:not-bytes": "d84001", "typed:unknown-76": "d84c4100",
  "simple:0": "e0", "simple:19": "f3", "simple:32": "f820", "simple:255": "f8ff", "simple:in-array": "82e0f820",
  "map:int-keys": "a2016102616202", "map:dup-keys": "a3616101616202616203", "map:proto": "a2695f5f70726f746f5f5f016161 02".replace(/ /g, ""),
  "map:dup-int-keys": "a2016161016162", "map:float-int-key": "a201f6f93c00f5",
  "map:mixed-dup": "a361610101f6616102",
  "tag:unknown": "d903e86178",
  "nested:deep-ok": "8181818181818181818100",
  "err:empty": "", "err:break": "ff", "err:ai28": "1c", "err:chunk-major": "5f6101ff", "err:trailing": "0101",
  "err:truncated-text": "61", "err:invalid-utf8": "62c328", "err:split-utf8-chunks": "7f61c361a9ff",
  "err:truncated-map": "a101", "err:unterminated": "9f01", "err:odd-indef-map": "bf6161ff",
  "err:break-in-tag": "9fc1ffff", "err:indef-uint": "1f", "err:indef-tag": "df", "err:indef-nint": "3f",
  "err:simple-24-small": "f818", "err:truncated-u64": "1b0000", "err:break-in-definite": "81ff",
  "err:tag-too-big": "db0020000000000000f6", "err:tag-2^31": "da80000000f6", "tag:2^31-1": "da7ffffffff6", "err:nested-indef-chunk": "5f5f4101ffff",
  "err:truncated-array": "8301", "err:truncated-bytes": "4501",
};
for (const [name, hex] of Object.entries(decodeCases)) {
  try {
    const decoded = decodeDoc(Buffer.from(hex, "hex"));
    out.decode.push({ name, hex, ok: true, decoded: describe(decoded), reencoded: encodeDoc(decoded).toString("hex") });
  } catch (error) {
    out.decode.push({ name, hex, ok: false, error: String(error.message ?? error) });
  }
}

const logSets = [
  [{ t: 1, level: "info", logger: "L", msg: "m" }],
  [
    { t: 1760000000123, level: "warn", logger: "Main:LiveCheck", msg: "é😀 \"quoted\"\n" },
    { t: 1760000000124, level: "error", logger: "Scheduler", msg: "split \ud83d" },
    { t: 1760000000125, level: "debug", logger: "X", msg: "\u0000 " },
  ],
  [],
];
for (const lines of logSets) {
  out.logsGz.push({
    gz: gzipSync(JSON.stringify(lines)).toString("base64"),
    lines: JSON.parse(JSON.stringify(lines).replace(/\\ud83d"/g, '\\ufffd"')),
  });
}

writeFileSync(
  process.argv[2] ?? new URL("../tests/golden/cbor.json", import.meta.url),
  `${JSON.stringify(out)}\n`,
);
console.log(`wrote ${out.encode.length} encode, ${out.decode.length} decode, ${out.logsGz.length} logsGz cases`);
