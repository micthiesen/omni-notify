import { describe, expect, it } from "vitest";
import { parseVoices } from "./voices.js";

const FIXTURE = `# Podcast Taste Profile

## Favorite genres

- True crime
- Tech interviews

## Voices I follow — recommend their guest spots anywhere

- Jesse Singal (Blocked and Reported)
- Ezra Klein
- <add more>
- jesse singal

## Shows to avoid

- Some Show I Hate
- Another Bad Show
`;

describe("parseVoices", () => {
  it("collects names from the Voices section only", () => {
    expect(parseVoices(FIXTURE)).toEqual(["Jesse Singal", "Ezra Klein"]);
  });

  it("does not pick up bullets from other sections", () => {
    const names = parseVoices(FIXTURE);
    expect(names).not.toContain("True crime");
    expect(names).not.toContain("Some Show I Hate");
  });

  it("strips a trailing parenthetical", () => {
    expect(parseVoices(FIXTURE)).toContain("Jesse Singal");
  });

  it("dedupes case-insensitively, keeping the first occurrence's casing", () => {
    const names = parseVoices(FIXTURE);
    expect(names.filter((n) => n.toLowerCase() === "jesse singal")).toEqual([
      "Jesse Singal",
    ]);
  });

  it("drops placeholder items", () => {
    expect(parseVoices(FIXTURE)).not.toContain("<add more>");
  });

  it("returns [] when there is no Voices section", () => {
    const noVoices = `# Podcast Taste Profile\n\n## Favorite genres\n\n- True crime\n`;
    expect(parseVoices(noVoices)).toEqual([]);
  });

  it("returns [] for empty input", () => {
    expect(parseVoices("")).toEqual([]);
  });
});

describe("parseVoices edge cases", () => {
  it("preserves hyphenated/apostrophe names and ignores ### subheaders", () => {
    const md = [
      "## Voices I follow — recommend their guest spots anywhere",
      "### Core",
      "- Jean-Luc Picard",
      "- Anne-Marie Slaughter",
      "### More",
      "- Alex O'Connor",
      "",
      "## Taste",
      "- not a voice",
    ].join("\n");
    expect(parseVoices(md)).toEqual([
      "Jean-Luc Picard",
      "Anne-Marie Slaughter",
      "Alex O'Connor",
    ]);
  });
});
