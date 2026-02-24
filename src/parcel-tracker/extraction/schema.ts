import { z } from "zod";

export const deliveryExtractionSchema = z.object({
  deliveries: z.array(
    z.object({
      tracking_number: z.string().describe("The package tracking number"),
      carrier: z.string().describe("The shipping carrier name (e.g. FedEx, UPS, USPS)"),
      description: z
        .string()
        .describe(
          "Short title for the package in Title Case (e.g. 'Running Shoes', 'Kitchen Knife Set')",
        ),
    }),
  ),
});

export type DeliveryExtraction = z.infer<typeof deliveryExtractionSchema>;
