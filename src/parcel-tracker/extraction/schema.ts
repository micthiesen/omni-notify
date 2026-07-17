import { z } from "zod";

export const deliveryExtractionSchema = z.object({
  deliveries: z.array(
    z.object({
      tracking_number: z.string().describe("The package tracking number"),
      carrier_candidates: z
        .array(z.string())
        .min(1)
        .describe(
          "Carrier codes from the provided carrier list, ranked most likely first (up to 3)",
        ),
      description: z
        .string()
        .describe(
          "Short title for the package prefixed with a relevant emoji in Title Case (e.g. '👟 Running Shoes', '🔪 Kitchen Knife Set')",
        ),
    }),
  ),
});

export type DeliveryExtraction = z.infer<typeof deliveryExtractionSchema>;
