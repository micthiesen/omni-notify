import { z } from "zod";

export const deliveryExtractionSchema = z.object({
  deliveries: z.array(
    z.object({
      tracking_number: z.string().describe("The package tracking number"),
      carrier_code: z
        .string()
        .describe("The carrier code from the provided carrier list"),
      description: z
        .string()
        .describe(
          "Short title for the package in Title Case (e.g. 'Running Shoes', 'Kitchen Knife Set')",
        ),
    }),
  ),
});

export type DeliveryExtraction = z.infer<typeof deliveryExtractionSchema>;
