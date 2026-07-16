import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, describe, expect, it } from "vitest";
import { describeUnreadableFile } from "./task.js";

describe("describeUnreadableFile", () => {
  const dir = mkdtempSync(join(tmpdir(), "podcast-taste-"));
  const file = join(dir, "taste.md");
  writeFileSync(file, "profile");

  afterAll(() => rmSync(dir, { recursive: true, force: true }));

  it("accepts a readable file", () => {
    expect(describeUnreadableFile(file)).toBeUndefined();
  });

  it("rejects a directory (the EISDIR footgun)", () => {
    expect(describeUnreadableFile(dir)).toContain("not a file");
  });

  it("rejects a missing path", () => {
    expect(describeUnreadableFile(join(dir, "nope.md"))).toContain("could not be read");
  });
});
