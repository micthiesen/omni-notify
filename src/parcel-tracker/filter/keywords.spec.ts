import type { Logger } from "@micthiesen/mitools/logging";
import { describe, expect, it, vi } from "vitest";
import { isTrackingCandidate } from "./keywords.js";

const mockLogger = {
  debug: vi.fn(),
  info: vi.fn(),
  warn: vi.fn(),
  error: vi.fn(),
  extend: vi.fn(),
} as unknown as Logger;

describe("isTrackingCandidate", () => {
  it("should reject blacklisted amazon senders", async () => {
    expect(
      await isTrackingCandidate(
        {
          from: "shipment-tracking@amazon.com",
          subject: "Your order has shipped",
          textBody: "Here is your order confirmation.",
        },
        mockLogger,
      ),
    ).toBe(false);
  });

  it("should match UPS sender domain", async () => {
    expect(
      await isTrackingCandidate(
        { from: "noreply@ups.com", subject: "Delivery update", textBody: "" },
        mockLogger,
      ),
    ).toBe(true);
  });

  it("should reject blacklisted amazon subdomains", async () => {
    expect(
      await isTrackingCandidate(
        {
          from: "ship-confirm@amazon.co.uk",
          subject: "Your order",
          textBody: "",
        },
        mockLogger,
      ),
    ).toBe(false);
  });

  it("should match tracking keywords in subject", async () => {
    expect(
      await isTrackingCandidate(
        {
          from: "orders@somestore.com",
          subject: "Your order has shipped!",
          textBody: "Thank you for your purchase.",
        },
        mockLogger,
      ),
    ).toBe(true);
  });

  it("should match tracking keywords in body", async () => {
    expect(
      await isTrackingCandidate(
        {
          from: "orders@somestore.com",
          subject: "Order confirmation",
          textBody: "Your tracking number is 1Z999AA10123456784",
        },
        mockLogger,
      ),
    ).toBe(true);
  });

  it("should match 'in transit' keyword", async () => {
    expect(
      await isTrackingCandidate(
        {
          from: "orders@somestore.com",
          subject: "Your package is in transit",
          textBody: "",
        },
        mockLogger,
      ),
    ).toBe(true);
  });

  it("should reject blacklisted senders even with tracking keywords", async () => {
    const blacklisted = [
      "noreply@uber.com",
      "no-reply@doordash.com",
      "orders@skipthedishes.com",
      "noreply@instacart.com",
    ];
    for (const from of blacklisted) {
      expect(
        await isTrackingCandidate(
          {
            from,
            subject: "Your delivery is in transit",
            textBody: "Track your shipment",
          },
          mockLogger,
        ),
        `Expected ${from} to be rejected`,
      ).toBe(false);
    }
  });

  it("should reject unrelated emails", async () => {
    expect(
      await isTrackingCandidate(
        {
          from: "newsletter@example.com",
          subject: "Weekly digest",
          textBody: "Here are this week's top stories.",
        },
        mockLogger,
      ),
    ).toBe(false);
  });

  it("should reject promotional emails", async () => {
    expect(
      await isTrackingCandidate(
        {
          from: "marketing@store.com",
          subject: "50% off sale!",
          textBody: "Don't miss our biggest sale of the year.",
        },
        mockLogger,
      ),
    ).toBe(false);
  });

  it("should be case-insensitive for sender domains", async () => {
    expect(
      await isTrackingCandidate(
        { from: "noreply@FedEx.com", subject: "Update", textBody: "" },
        mockLogger,
      ),
    ).toBe(true);
  });

  it("should be case-insensitive for keywords", async () => {
    expect(
      await isTrackingCandidate(
        {
          from: "orders@somestore.com",
          subject: "YOUR ORDER HAS SHIPPED",
          textBody: "",
        },
        mockLogger,
      ),
    ).toBe(true);
  });
});
