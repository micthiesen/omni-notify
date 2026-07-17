import { describe, expect, it } from "vitest";
import { deliveryExtractionSchema } from "./schema.js";

describe("deliveryExtractionSchema", () => {
  it("should accept valid extraction with deliveries", () => {
    const result = deliveryExtractionSchema.parse({
      deliveries: [
        {
          tracking_number: "1Z999AA10123456784",
          carrier_candidates: ["ups"],
          description: "Electronics order",
        },
      ],
    });
    expect(result.deliveries).toHaveLength(1);
    expect(result.deliveries[0].tracking_number).toBe("1Z999AA10123456784");
    expect(result.deliveries[0].carrier_candidates).toEqual(["ups"]);
  });

  it("should accept multiple ranked carrier candidates", () => {
    const result = deliveryExtractionSchema.parse({
      deliveries: [
        {
          tracking_number: "DCM123456789",
          carrier_candidates: ["dicom", "gls", "canpost"],
          description: "Kitchen Knife Set",
        },
      ],
    });
    expect(result.deliveries[0].carrier_candidates).toEqual([
      "dicom",
      "gls",
      "canpost",
    ]);
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
          carrier_candidates: ["ups"],
          description: "Order 1",
        },
        {
          tracking_number: "9400111899223100315842",
          carrier_candidates: ["usps", "canpost"],
          description: "Order 2",
        },
      ],
    });
    expect(result.deliveries).toHaveLength(2);
  });

  it("should reject missing tracking_number", () => {
    expect(() =>
      deliveryExtractionSchema.parse({
        deliveries: [{ carrier_candidates: ["ups"], description: "Test" }],
      }),
    ).toThrow();
  });

  it("should reject missing carrier_candidates", () => {
    expect(() =>
      deliveryExtractionSchema.parse({
        deliveries: [{ tracking_number: "1Z999AA10123456784", description: "Test" }],
      }),
    ).toThrow();
  });

  it("should reject empty carrier_candidates", () => {
    expect(() =>
      deliveryExtractionSchema.parse({
        deliveries: [
          {
            tracking_number: "1Z999AA10123456784",
            carrier_candidates: [],
            description: "Test",
          },
        ],
      }),
    ).toThrow();
  });

  it("should reject missing deliveries key", () => {
    expect(() => deliveryExtractionSchema.parse({})).toThrow();
  });
});
