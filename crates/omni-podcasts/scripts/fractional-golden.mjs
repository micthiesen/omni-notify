// Golden vectors for rocicorp `fractional-indexing` generateKeyBetween (the
// version the TS Castro client uses). Regenerate from the repository root:
//   node crates/omni-podcasts/scripts/fractional-golden.mjs > crates/omni-podcasts/tests/golden/fractional_indexing.json
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "../../..");
const require = createRequire(join(root, "package.json"));
const { generateKeyBetween } = await import(require.resolve("fractional-indexing"));

const keys = [null, "a0", "a1", "a2", "a9", "aZ", "az", "b00", "b0z", "b10", "bzz", "Zz", "Z0", "Zy", "Y00", "Yzz",
  "a0V", "a0l", "a1V", "ZM8", "ZME", "aE", "a0001", "zzzzzzzzzzzzzzzzzzzzzzzzzzzV", "A00000000000000000000000001",
  "a", "a00", "B", "a0+", "b1", "ZMF", "c000"];
const cases = [];
for (const a of keys) {
  for (const b of keys) {
    let out;
    try {
      out = { key: generateKeyBetween(a, b) };
    } catch (error) {
      out = { error: error.message };
    }
    cases.push({ a, b, ...out });
  }
}
// Chains: repeated appends and prepends, and repeated bisection.
let k = null;
const appends = [];
for (let i = 0; i < 70; i++) { k = generateKeyBetween(k, null); appends.push(k); }
k = null;
const prepends = [];
for (let i = 0; i < 70; i++) { k = generateKeyBetween(null, k); prepends.push(k); }
let lo = "a0";
const hi = "a1";
const bisect = [];
for (let i = 0; i < 40; i++) { lo = generateKeyBetween(lo, hi); bisect.push(lo); }
process.stdout.write(JSON.stringify({ cases, appends, prepends, bisect }, null, 1) + "\n");
