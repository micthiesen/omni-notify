import { describe, expect, it } from "vitest";
import { chunkText, splitSections } from "./textChunking.js";

describe("splitSections", () => {
  it("returns one untitled section when there are no headings", () => {
    const sections = splitSections("Just some narration.\n\nA second paragraph.");
    expect(sections).toEqual([
      { title: undefined, body: "Just some narration.\n\nA second paragraph." },
    ]);
  });

  it("splits on ## headings and keeps the intro untitled", () => {
    const text =
      "Opening hook.\n\n## Background\n\nBody one.\n\n## The Turn\n\nBody two.";
    const sections = splitSections(text);
    expect(sections).toEqual([
      { title: undefined, body: "Opening hook." },
      { title: "Background", body: "Body one." },
      { title: "The Turn", body: "Body two." },
    ]);
  });

  it("handles a leading heading with no intro text", () => {
    const sections = splitSections("## First\n\nBody.");
    expect(sections).toEqual([{ title: "First", body: "Body." }]);
  });

  it("never emits an empty-body section", () => {
    const sections = splitSections("## Empty\n\n## Real\n\nText.");
    expect(sections).toEqual([{ title: "Real", body: "Text." }]);
  });
});

describe("chunkText", () => {
  it("keeps a short text as a single chunk", () => {
    expect(chunkText("Short.", 900, 1500)).toEqual(["Short."]);
  });

  it("merges paragraphs up to the target and starts a new chunk past it", () => {
    const p = "A".repeat(300);
    const q = "B".repeat(300);
    const r = "C".repeat(300);
    // p+q = 602 chars fits under 900; adding r (904) tips over into a new chunk.
    const chunks = chunkText(`${p}\n\n${q}\n\n${r}`, 900, 1500);
    expect(chunks).toEqual([`${p}\n\n${q}`, r]);
  });

  it("splits an oversized paragraph on sentence boundaries, never mid-sentence", () => {
    const sentence = `${"word ".repeat(40).trim()}.`;
    const paragraph = `${sentence} ${sentence} ${sentence}`;
    const chunks = chunkText(paragraph, 300, 400);
    expect(chunks.length).toBeGreaterThan(1);
    for (const chunk of chunks) {
      expect(chunk.trim()).toMatch(/\.$/);
    }
    expect(chunks.join(" ").replace(/\s+/g, " ")).toBe(paragraph.replace(/\s+/g, " "));
  });
});
