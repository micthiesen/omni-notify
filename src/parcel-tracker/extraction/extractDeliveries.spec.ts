import { describe, expect, it } from "vitest";
import { deliveryExtractionSchema } from "./schema.js";

describe("deliveryExtractionSchema", () => {
  it("should accept valid extraction with deliveries", () => {
    const result = deliveryExtractionSchema.parse({
      deliveries: [
        {
          tracking_number: "1Z999AA10123456784",
          carrier: "UPS",
          description: "Electronics order",
        },
      ],
    });
    expect(result.deliveries).toHaveLength(1);
    expect(result.deliveries[0].tracking_number).toBe("1Z999AA10123456784");
  });

  it("should accept empty deliveries array", () => {
    const result = deliveryExtractionSchema.parse({ deliveries: [] });
    expect(result.deliveries).toHaveLength(0);
  });

  it("should accept multiple deliveries", () => {
    const result = deliveryExtractionSchema.parse({
      deliveries: [
        {
          tracking_number: "1Z999AA10123456784",
          carrier: "UPS",
          description: "Order 1",
        },
        {
          tracking_number: "9400111899223100315842",
          carrier: "USPS",
          description: "Order 2",
        },
      ],
    });
    expect(result.deliveries).toHaveLength(2);
  });

  it("should reject missing tracking_number", () => {
    expect(() =>
      deliveryExtractionSchema.parse({
        deliveries: [{ carrier: "UPS", description: "Test" }],
      }),
    ).toThrow();
  });

  it("should reject missing carrier", () => {
    expect(() =>
      deliveryExtractionSchema.parse({
        deliveries: [{ tracking_number: "1Z999AA10123456784", description: "Test" }],
      }),
    ).toThrow();
  });

  it("should reject missing deliveries key", () => {
    expect(() => deliveryExtractionSchema.parse({})).toThrow();
  });
});
