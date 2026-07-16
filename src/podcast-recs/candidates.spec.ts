import { describe, expect, it } from "vitest";
import { pickBestByTitle } from "./candidates.js";

describe("pickBestByTitle", () => {
  const shows = [
    { title: "The Gray Area with Sean Illing", feedUrl: "a" },
    { title: "Past Present Future", feedUrl: "b" },
    { title: "Very Bad Wizards", feedUrl: "c" },
  ];

  it("prefers an exact normalized match", () => {
    expect(pickBestByTitle(shows, "very bad wizards!")?.feedUrl).toBe("c");
  });

  it("falls back to containment in either direction", () => {
    expect(pickBestByTitle(shows, "The Gray Area")?.feedUrl).toBe("a");
    expect(pickBestByTitle(shows, "Past, Present & Future")?.feedUrl).toBe("b");
  });

  it("returns undefined when nothing matches", () => {
    expect(pickBestByTitle(shows, "Hardcore History")).toBeUndefined();
  });
});
