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
      static _store = store;
    },
  };
});

import { Entity } from "@micthiesen/mitools/entities";
import {
  addBriefingNotification,
  type BriefingNotification,
  formatNotifications,
  getBriefingHistory,
  resolveHistoryPlaceholders,
} from "./persistence.js";

function clearStore() {
  (Entity as unknown as { _store: Map<string, unknown> })._store.clear();
}

function makeNotification(
  title: string,
  url = "https://example.com",
): BriefingNotification {
  return { title, message: "msg", url, timestamp: Date.now() };
}

describe("formatNotifications", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-02-06T14:30:00"));
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("returns empty message for empty array", () => {
    const result = formatNotifications([], 5);
    expect(result).toBe("- No previous notifications");
  });

  it("returns empty message when count is 0", () => {
    const result = formatNotifications([makeNotification("A")], 0);
    expect(result).toBe("- No previous notifications");
  });

  it("formats a single notification with timestamp", () => {
    const result = formatNotifications(
      [makeNotification("Cool Article", "https://cbc.ca")],
      5,
    );
    expect(result).toBe("- Cool Article (https://cbc.ca) [Feb 6, 2:30 PM]");
  });

  it("limits to the most recent N notifications", () => {
    const notifications = [
      makeNotification("Old"),
      makeNotification("Middle"),
      makeNotification("Recent"),
    ];
    const result = formatNotifications(notifications, 2);
    expect(result).not.toContain("Old");
    expect(result).toContain("Middle");
    expect(result).toContain("Recent");
  });

  it("returns all when count exceeds available", () => {
    const notifications = [makeNotification("A"), makeNotification("B")];
    const result = formatNotifications(notifications, 10);
    expect(result).toContain("- A");
    expect(result).toContain("- B");
  });
});

describe("addBriefingNotification", () => {
  beforeEach(() => clearStore());

  it("appends a notification to empty history", () => {
    addBriefingNotification("TestBriefing", makeNotification("First"));
    const history = getBriefingHistory("TestBriefing");
    expect(history.notifications).toHaveLength(1);
    expect(history.notifications[0].title).toBe("First");
  });

  it("prunes to last 50 notifications", () => {
    for (let i = 0; i < 55; i++) {
      addBriefingNotification("TestBriefing", makeNotification(`N${i}`));
    }
    const history = getBriefingHistory("TestBriefing");
    expect(history.notifications).toHaveLength(50);
    expect(history.notifications[0].title).toBe("N5");
    expect(history.notifications[49].title).toBe("N54");
  });
});

describe("resolveHistoryPlaceholders", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-02-06T14:30:00"));
    clearStore();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("replaces {{history:N}} with formatted history", () => {
    addBriefingNotification("News", makeNotification("Article", "https://example.com"));
    const result = resolveHistoryPlaceholders(
      "System prompt\n\n{{history:5}}\n\nDo not repeat.",
      "News",
    );
    expect(result).toContain("- Article (https://example.com) [Feb 6, 2:30 PM]");
    expect(result).not.toContain("{{history");
  });

  it("leaves prompts without placeholders unchanged", () => {
    const prompt = "You are an assistant. Do good work.";
    const result = resolveHistoryPlaceholders(prompt, "News");
    expect(result).toBe(prompt);
  });

  it("handles count of 0 as empty history", () => {
    addBriefingNotification("News", makeNotification("Article"));
    const result = resolveHistoryPlaceholders("{{history:0}}", "News");
    expect(result).toBe("- No previous notifications");
  });

  it("handles multiple placeholders", () => {
    const result = resolveHistoryPlaceholders(
      "A: {{history:3}}\nB: {{history:5}}",
      "News",
    );
    expect(result).not.toContain("{{history");
    expect((result.match(/No previous notifications/g) ?? []).length).toBe(2);
  });
});
