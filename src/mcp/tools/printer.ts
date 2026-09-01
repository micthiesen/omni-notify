import { Effect } from "effect";
import { z } from "zod";
import type { McpRuntime } from "../runtime.js";
import {
  annotations,
  defineTool,
  emptyInputSchema,
  type McpToolDefinition,
} from "../tool.js";

const paperSchema = z.enum(["letter", "a4", "legal"]);
const sidesSchema = z.enum([
  "one-sided",
  "two-sided-long-edge",
  "two-sided-short-edge",
]);

function requirePrinter(runtime: McpRuntime) {
  if (!runtime.printer) throw new Error("Printer is not configured");
  return runtime.printer;
}

export function createPrinterTools(runtime: McpRuntime): McpToolDefinition[] {
  return [
    defineTool({
      name: "get_printer_status",
      title: "Get Monochrome Printer Status",
      description:
        "Read readiness, queue depth, toner level, and capabilities from Michael's fixed Brother HL-L2370DW monochrome laser printer. This does not print anything.",
      inputSchema: emptyInputSchema,
      outputSchema: z.object({
        configured: z.literal(true),
        name: z.string().nullable(),
        uri: z.string(),
        state: z.string(),
        stateReasons: z.array(z.string()),
        ready: z.boolean(),
        acceptingJobs: z.boolean().nullable(),
        queuedJobCount: z.number().int().nonnegative().nullable(),
        tonerPercent: z.number().min(0).max(100).nullable(),
        monochromeOnly: z.literal(true),
        defaultSides: z.literal("two-sided-long-edge"),
        supportedFormats: z.array(z.string()),
        supportedMedia: z.array(z.string()),
      }),
      annotations: annotations(true, false, true, true),
      policy: {
        sideEffects: ["Reads status from the fixed LAN printer"],
        cost: "none",
        recommendedPolicy: "allow",
      },
      execute: () => Effect.tryPromise(() => requirePrinter(runtime).status()),
    }),
    defineTool({
      name: "print_document",
      title: "Print PDF in Black and White",
      description:
        "Print a public HTTPS PDF on Michael's Brother HL-L2370DW monochrome laser printer. Black-and-white only. Uses two-sided long-edge printing and Letter paper by default. Every call requires approval because it consumes paper and toner and physically exposes the document.",
      inputSchema: z
        .object({
          url: z.string().url().max(2_048).describe("Public HTTPS PDF URL"),
          jobName: z.string().trim().min(1).max(80).optional(),
          copies: z.number().int().min(1).max(3).default(1),
          paper: paperSchema.default("letter"),
          sides: sidesSchema.default("two-sided-long-edge"),
          allowDuplicate: z
            .boolean()
            .default(false)
            .describe(
              "Allow the same PDF and settings to print again within 5 minutes",
            ),
        })
        .strict(),
      outputSchema: z.object({
        accepted: z.literal(true),
        completed: z.boolean(),
        jobId: z.number().int().nullable(),
        jobUri: z.string(),
        jobState: z.string(),
        jobName: z.string(),
        pages: z.number().int().positive(),
        copies: z.number().int().min(1).max(3),
        paper: paperSchema,
        sides: sidesSchema,
        impressionsCompleted: z.number().int().nonnegative().nullable(),
        message: z.string(),
      }),
      annotations: annotations(false, false, false, true),
      policy: {
        sideEffects: [
          "Downloads a public PDF",
          "Sends a physical print job to the fixed Brother printer",
          "Physically exposes the printed document",
        ],
        cost: "Consumes paper, toner, and electricity; no paid API",
        recommendedPolicy: "require_approval",
      },
      execute: (input) =>
        Effect.tryPromise(() => requirePrinter(runtime).printPdf(input)),
    }),
  ];
}
