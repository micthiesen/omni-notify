import { Injector } from "@micthiesen/mitools/config";
import type { Logger } from "@micthiesen/mitools/logging";
import { LogLevel } from "@micthiesen/mitools/logging";
import { afterEach, describe, expect, it, vi } from "vitest";
import { EmailFeedbackEntity, recordEmailFeedback } from "./feedback.js";
import {
  buildTriagePrompt,
  EmailTriageService,
  MAX_TRIAGE_CACHE_ENTRIES,
  type TriageEmail,
  type TriageVerdict,
} from "./triage.js";

Injector.configure({
  config: {
    LOG_LEVEL: LogLevel.INFO,
    PUSHOVER_TOKEN: "fake-token",
    PUSHOVER_USER: "fake-user",
    DOCKERIZED: false,
    DB_NAME: "triage.spec.db",
  },
});

const mockLogger = {
  debug: vi.fn(),
  info: vi.fn(),
  warn: vi.fn(),
  error: vi.fn(),
  extend: vi.fn(),
} as unknown as Logger;

const verdict: TriageVerdict = { parcel: true, calendar: false, reason: "tracking" };

function makeEmail(id: string, overrides: Partial<TriageEmail> = {}): TriageEmail {
  return {
    id,
    subject: `Subject ${id}`,
    from: "orders@shop.com",
    textBody: "body text",
    links: [],
    ...overrides,
  };
}

afterEach(() => {
  EmailFeedbackEntity.deleteAll();
  vi.clearAllMocks();
});

describe("EmailTriageService memoization", () => {
  it("shares one in-flight call between concurrent classifies of the same email", async () => {
    const classifyFn = vi.fn(async (): Promise<TriageVerdict> => {
      await Promise.resolve();
      return verdict;
    });
    const triage = new EmailTriageService(mockLogger, classifyFn);

    const email = makeEmail("e1");
    const [a, b] = await Promise.all([triage.classify(email), triage.classify(email)]);
    expect(a).toEqual(verdict);
    expect(b).toEqual(verdict);
    expect(classifyFn).toHaveBeenCalledTimes(1);
  });

  it("classifies distinct emails separately", async () => {
    const classifyFn = vi.fn(async () => verdict);
    const triage = new EmailTriageService(mockLogger, classifyFn);

    await triage.classify(makeEmail("e1"));
    await triage.classify(makeEmail("e2"));
    expect(classifyFn).toHaveBeenCalledTimes(2);
  });

  it("does not cache failures: a later classify retries and can succeed", async () => {
    const classifyFn = vi
      .fn<(email: TriageEmail) => Promise<TriageVerdict>>()
      .mockRejectedValueOnce(new Error("model down"))
      .mockResolvedValueOnce(verdict);
    const triage = new EmailTriageService(mockLogger, classifyFn);

    const email = makeEmail("e1");
    await expect(triage.classify(email)).rejects.toThrow("model down");
    expect(mockLogger.warn).toHaveBeenCalled();
    await expect(triage.classify(email)).resolves.toEqual(verdict);
    expect(classifyFn).toHaveBeenCalledTimes(2);
  });

  it("evicts the oldest entry once the cache cap is exceeded", async () => {
    const classifyFn = vi.fn(async () => verdict);
    const triage = new EmailTriageService(mockLogger, classifyFn);

    await triage.classify(makeEmail("first"));
    for (let i = 0; i < MAX_TRIAGE_CACHE_ENTRIES; i++) {
      await triage.classify(makeEmail(`filler-${i}`));
    }
    // "first" was evicted, so classifying it again calls the model again
    await triage.classify(makeEmail("first"));
    expect(classifyFn).toHaveBeenCalledTimes(MAX_TRIAGE_CACHE_ENTRIES + 2);
  });
});

describe("buildTriagePrompt", () => {
  it("includes sender, subject, and a truncated body", () => {
    const prompt = buildTriagePrompt(
      makeEmail("e1", { textBody: `${"x".repeat(1500)}TAIL` }),
    );
    expect(prompt).toContain("From: orders@shop.com");
    expect(prompt).toContain("Subject: Subject e1");
    expect(prompt).toContain("x".repeat(1500));
    expect(prompt).not.toContain("TAIL");
  });

  it("caps links at five and omits the section when there are none", () => {
    const links = Array.from({ length: 7 }, (_, i) => `https://l.test/${i}`);
    const prompt = buildTriagePrompt(makeEmail("e1", { links }));
    expect(prompt).toContain("https://l.test/4");
    expect(prompt).not.toContain("https://l.test/5");
    expect(buildTriagePrompt(makeEmail("e2"))).not.toContain("Links:");
  });

  it("appends user-correction digests only when feedback exists", () => {
    const heading = "Recent user corrections — follow these";
    expect(buildTriagePrompt(makeEmail("e1"))).not.toContain(heading);

    recordEmailFeedback({
      pipeline: "ParcelTracker",
      emailId: "fb1",
      subject: "npm package published",
      from: "support@npmjs.com",
      verdict: "not_relevant",
    });
    const prompt = buildTriagePrompt(makeEmail("e2"));
    expect(prompt).toContain(heading);
    expect(prompt).toContain(
      '- "npm package published" from support@npmjs.com: user marked NOT relevant',
    );
  });
});
