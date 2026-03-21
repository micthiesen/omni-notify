import type { Logger } from "@micthiesen/mitools/logging";
import { describe, expect, it, vi } from "vitest";
import { filterTrackingCandidate } from "./keywords.js";

const mockLogger = {
  debug: vi.fn(),
  info: vi.fn(),
  warn: vi.fn(),
  error: vi.fn(),
  extend: vi.fn(),
} as unknown as Logger;

describe("filterTrackingCandidate", () => {
  it("should reject blacklisted amazon senders", async () => {
    const result = await filterTrackingCandidate(
      {
        from: "shipment-tracking@amazon.com",
        subject: "Your order has shipped",
        textBody: "Here is your order confirmation.",
      },
      mockLogger,
    );
    expect(result.pass).toBe(false);
    expect(result.reason).toBe("blacklisted sender");
  });

  it("should match UPS sender domain", async () => {
    const result = await filterTrackingCandidate(
      { from: "noreply@ups.com", subject: "Delivery update", textBody: "" },
      mockLogger,
    );
    expect(result.pass).toBe(true);
    expect(result.reason).toBe("carrier sender");
  });

  it("should reject blacklisted amazon subdomains", async () => {
    const result = await filterTrackingCandidate(
      {
        from: "ship-confirm@amazon.co.uk",
        subject: "Your order",
        textBody: "",
      },
      mockLogger,
    );
    expect(result.pass).toBe(false);
  });

  it("should match tracking keywords in subject", async () => {
    const result = await filterTrackingCandidate(
      {
        from: "orders@somestore.com",
        subject: "Your order has shipped!",
        textBody: "Thank you for your purchase.",
      },
      mockLogger,
    );
    expect(result.pass).toBe(true);
    expect(result.reason).toBe('keyword "shipped"');
  });

  it("should match tracking keywords in body", async () => {
    const result = await filterTrackingCandidate(
      {
        from: "orders@somestore.com",
        subject: "Order confirmation",
        textBody: "Your tracking number is 1Z999AA10123456784",
      },
      mockLogger,
    );
    expect(result.pass).toBe(true);
  });

  it("should match 'in transit' keyword", async () => {
    const result = await filterTrackingCandidate(
      {
        from: "orders@somestore.com",
        subject: "Your package is in transit",
        textBody: "",
      },
      mockLogger,
    );
    expect(result.pass).toBe(true);
    expect(result.reason).toBe('keyword "in transit"');
  });

  it("should reject blacklisted senders even with tracking keywords", async () => {
    const blacklisted = [
      "noreply@uber.com",
      "no-reply@doordash.com",
      "orders@skipthedishes.com",
      "noreply@instacart.com",
    ];
    for (const from of blacklisted) {
      const result = await filterTrackingCandidate(
        {
          from,
          subject: "Your delivery is in transit",
          textBody: "Track your shipment",
        },
        mockLogger,
      );
      expect(result.pass, `Expected ${from} to be rejected`).toBe(false);
      expect(result.reason).toBe("blacklisted sender");
    }
  });

  it("should reject unrelated emails", async () => {
    const result = await filterTrackingCandidate(
      {
        from: "newsletter@example.com",
        subject: "Weekly digest",
        textBody: "Here are this week's top stories.",
      },
      mockLogger,
    );
    expect(result.pass).toBe(false);
    expect(result.reason).toBe("no keyword match");
  });

  it("should reject promotional emails", async () => {
    const result = await filterTrackingCandidate(
      {
        from: "marketing@store.com",
        subject: "50% off sale!",
        textBody: "Don't miss our biggest sale of the year.",
      },
      mockLogger,
    );
    expect(result.pass).toBe(false);
  });

  it("should be case-insensitive for sender domains", async () => {
    const result = await filterTrackingCandidate(
      { from: "noreply@FedEx.com", subject: "Update", textBody: "" },
      mockLogger,
    );
    expect(result.pass).toBe(true);
    expect(result.reason).toBe("carrier sender");
  });

  it("should be case-insensitive for keywords", async () => {
    const result = await filterTrackingCandidate(
      {
        from: "orders@somestore.com",
        subject: "YOUR ORDER HAS SHIPPED",
        textBody: "",
      },
      mockLogger,
    );
    expect(result.pass).toBe(true);
  });
});
