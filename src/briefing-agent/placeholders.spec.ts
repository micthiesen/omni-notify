import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@micthiesen/mitools/entities", () => {
  const store = new Map<string, unknown>();
  return {
    Entity: class {
      constructor(
        private name: string,
        private keys: string[],
      ) {}
      get(key: Record<string, string>) {
        return store.get(`${this.name}:${JSON.stringify(key)}`) ?? null;
      }
      upsert(data: Record<string, unknown>) {
        const keyObj: Record<string, unknown> = {};
        for (const k of this.keys) keyObj[k] = data[k];
        store.set(`${this.name}:${JSON.stringify(keyObj)}`, data);
      }
    },
  };
});

import {
  resolveAllPlaceholders,
  resolveDatePlaceholder,
  resolveTimePlaceholder,
} from "./placeholders.js";

describe("resolveDatePlaceholder", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-02-06T14:30:00"));
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("replaces {{date}} with formatted date", () => {
    const result = resolveDatePlaceholder("Today is {{date}}.");
    expect(result).toBe("Today is Friday, February 6, 2026.");
  });

  it("replaces multiple {{date}} placeholders", () => {
    const result = resolveDatePlaceholder("{{date}} and {{date}}");
    expect(result).toContain("February 6, 2026");
    expect(result).not.toContain("{{date}}");
  });

  it("leaves prompts without {{date}} unchanged", () => {
    const prompt = "No placeholders here.";
    expect(resolveDatePlaceholder(prompt)).toBe(prompt);
  });
});

describe("resolveTimePlaceholder", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-02-06T14:30:00"));
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("replaces {{time}} with formatted time", () => {
    const result = resolveTimePlaceholder("It is {{time}}.");
    expect(result).toMatch(/^\S/);
    expect(result).not.toContain("{{time}}");
    expect(result).toMatch(/\d{1,2}:\d{2}\s*(AM|PM)/);
  });

  it("replaces multiple {{time}} placeholders", () => {
    const result = resolveTimePlaceholder("{{time}} and {{time}}");
    expect(result).not.toContain("{{time}}");
  });

  it("leaves prompts without {{time}} unchanged", () => {
    const prompt = "No placeholders here.";
    expect(resolveTimePlaceholder(prompt)).toBe(prompt);
  });
});

describe("resolveAllPlaceholders", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-02-06T14:30:00"));
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("resolves {{date}}, {{time}}, and {{history:N}} together", () => {
    const prompt =
      "Date: {{date}}\nTime: {{time}}\nHistory:\n{{history:5}}";
    const result = resolveAllPlaceholders(prompt, "TestBriefing");
    expect(result).not.toContain("{{date}}");
    expect(result).not.toContain("{{time}}");
    expect(result).not.toContain("{{history");
    expect(result).toContain("February 6, 2026");
    expect(result).toContain("No previous notifications");
  });

  it("works with no placeholders", () => {
    const prompt = "Just a plain prompt.";
    expect(resolveAllPlaceholders(prompt, "Test")).toBe(prompt);
  });
});
