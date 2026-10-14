import { expect, test } from "bun:test";
import { nets, parts } from "./board-spec";

test("designators are unique", () => {
  const refs = parts.map((part) => part.ref);
  expect(new Set(refs).size).toBe(refs.length);
});

test("every net endpoint names a planned part", () => {
  const refs = new Set(parts.map((part) => part.ref));
  const unknown = Object.values(nets)
    .flat()
    .map((endpoint) => endpoint.split(".")[0])
    .filter((ref) => !refs.has(ref));
  expect(unknown).toEqual([]);
});
