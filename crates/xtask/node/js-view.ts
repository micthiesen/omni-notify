/**
 * The JSON "JS view" shared by the CBOR golden vectors and node-readback: a lossless
 * JSON rendering of the values node-cbor hands to JS.
 *
 * - plain JSON values stay as they are (object key order preserved);
 * - `undefined` -> {"$undefined": true}
 * - non-finite / negative-zero numbers -> {"$number": "NaN" | "Infinity" | "-Infinity" | "-0"}
 * - Date -> {"$date": <epoch ms as JS holds it, or "NaN">}
 * - Set -> {"$set": [...]}; Map -> {"$map": [[k, v], ...]}
 * - Buffer / typed array -> {"$bytes": "<hex>"}; bigint -> {"$bigint": "<decimal>"}
 * - cbor.Tagged -> {"$tagged": [tag, value]}; any other object -> its own enumerable keys.
 */
export function jsView(value: unknown): unknown {
  if (value === undefined) return { $undefined: true };
  if (value === null || typeof value === "boolean" || typeof value === "string") return value;
  if (typeof value === "number") {
    if (Number.isNaN(value)) return { $number: "NaN" };
    if (value === Infinity) return { $number: "Infinity" };
    if (value === -Infinity) return { $number: "-Infinity" };
    if (Object.is(value, -0)) return { $number: "-0" };
    return value;
  }
  if (typeof value === "bigint") return { $bigint: value.toString() };
  if (value instanceof Date) {
    const ms = value.getTime();
    return { $date: Number.isNaN(ms) ? "NaN" : ms };
  }
  if (value instanceof Set) return { $set: [...value].map(jsView) };
  if (value instanceof Map) return { $map: [...value].map(([k, v]) => [jsView(k), jsView(v)]) };
  if (ArrayBuffer.isView(value)) {
    return { $bytes: Buffer.from(value.buffer, value.byteOffset, value.byteLength).toString("hex") };
  }
  if (Array.isArray(value)) return value.map(jsView);
  const tagged = value as { tag?: unknown; value?: unknown; constructor?: { name?: string } };
  if (tagged.constructor?.name === "Tagged") return { $tagged: [tagged.tag, jsView(tagged.value)] };
  return Object.fromEntries(Object.entries(value as object).map(([k, v]) => [k, jsView(v)]));
}
