import { z } from "zod";

export const deliveryExtractionSchema = z.object({
  deliveries: z.array(
    z.object({
      tracking_number: z.string().describe("The package tracking number"),
      carrier: z.string().describe("The shipping carrier name (e.g. FedEx, UPS, USPS)"),
      description: z
        .string()
        .describe("Brief description of the package contents or order"),
    }),
  ),
});

export type DeliveryExtraction = z.infer<typeof deliveryExtractionSchema>;
