import { describe, expect, it } from "vitest";
import { isTrackingCandidate } from "./keywords.js";

describe("isTrackingCandidate", () => {
  it("should match known carrier sender domains", () => {
    expect(
      isTrackingCandidate({
        from: "shipment-tracking@amazon.com",
        subject: "Your order",
        textBody: "Here is your order confirmation.",
      }),
    ).toBe(true);
  });

  it("should match UPS sender domain", () => {
    expect(
      isTrackingCandidate({
        from: "noreply@ups.com",
        subject: "Delivery update",
        textBody: "",
      }),
    ).toBe(true);
  });

  it("should match amazon subdomains", () => {
    expect(
      isTrackingCandidate({
        from: "ship-confirm@amazon.co.uk",
        subject: "Your order",
        textBody: "",
      }),
    ).toBe(true);
  });

  it("should match tracking keywords in subject", () => {
    expect(
      isTrackingCandidate({
        from: "orders@somestore.com",
        subject: "Your order has shipped!",
        textBody: "Thank you for your purchase.",
      }),
    ).toBe(true);
  });

  it("should match tracking keywords in body", () => {
    expect(
      isTrackingCandidate({
        from: "orders@somestore.com",
        subject: "Order confirmation",
        textBody: "Your tracking number is 1Z999AA10123456784",
      }),
    ).toBe(true);
  });

  it("should match 'in transit' keyword", () => {
    expect(
      isTrackingCandidate({
        from: "orders@somestore.com",
        subject: "Your package is in transit",
        textBody: "",
      }),
    ).toBe(true);
  });

  it("should reject unrelated emails", () => {
    expect(
      isTrackingCandidate({
        from: "newsletter@example.com",
        subject: "Weekly digest",
        textBody: "Here are this week's top stories.",
      }),
    ).toBe(false);
  });

  it("should reject promotional emails", () => {
    expect(
      isTrackingCandidate({
        from: "marketing@store.com",
        subject: "50% off sale!",
        textBody: "Don't miss our biggest sale of the year.",
      }),
    ).toBe(false);
  });

  it("should be case-insensitive for sender domains", () => {
    expect(
      isTrackingCandidate({
        from: "noreply@FedEx.com",
        subject: "Update",
        textBody: "",
      }),
    ).toBe(true);
  });

  it("should be case-insensitive for keywords", () => {
    expect(
      isTrackingCandidate({
        from: "orders@somestore.com",
        subject: "YOUR ORDER HAS SHIPPED",
        textBody: "",
      }),
    ).toBe(true);
  });
});
