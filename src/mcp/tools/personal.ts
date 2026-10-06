import { Effect } from "effect";
import { z } from "zod";
import { CostEventEntity, getCostEvents } from "../../costs/persistence.js";
import { summarizeCosts } from "../../costs/summary.js";
import {
  getAllPetsWithHistory,
  getDailyVisitCounts,
  getPet,
  getWeightHistory,
} from "../../pet-tracker/persistence.js";
import config from "../../utils/config.js";
import {
  annotations,
  defineTool,
  type McpToolDefinition,
  paginate,
  paginationInputShape,
} from "../tool.js";
import { pageSchema } from "./media-shared.js";

const nullableString = z.string().nullable();

const nullableNumber = z.number().nullable();

const MAX_MCP_COST_EVENTS = 100_000;

const costUsageSchema = z.object({
  inputTokens: z.number().nonnegative().optional(),
  inputNoCacheTokens: z.number().nonnegative().optional(),
  cacheReadTokens: z.number().nonnegative().optional(),
  cacheWriteTokens: z.number().nonnegative().optional(),
  outputTokens: z.number().nonnegative().optional(),
  reasoningTokens: z.number().nonnegative().optional(),
  characters: z.number().nonnegative().optional(),
  requests: z.number().nonnegative().optional(),
  credits: z.number().nonnegative().optional(),
});

export function createPersonalTools(): McpToolDefinition[] {
  return [
    defineTool({
      name: "pets_read",
      title: "Read Pet Weight Data",
      description:
        "List pets with bounded recent weight and visit history, or read a bounded slice for one pet. This uses only Omni's local synchronized data.",
      inputSchema: z.discriminatedUnion("resource", [
        z.object({
          resource: z.literal("list"),
          historyLimit: z.number().int().min(0).max(100).default(10),
        }),
        z.object({
          resource: z.literal("history"),
          petId: z.string().min(1).max(200),
          cursor: paginationInputShape.cursor,
          limit: paginationInputShape.limit,
        }),
      ]),
      outputSchema: z.discriminatedUnion("resource", [
        z.object({
          resource: z.literal("list"),
          pets: z.array(
            z.object({
              petId: z.string(),
              name: z.string(),
              currentWeight: z.number(),
              updatedAt: z.string(),
              recentWeights: z.array(
                z.object({ timestamp: z.string(), weight: z.number() }),
              ),
              recentVisits: z.array(
                z.object({ date: z.string(), count: z.number().int().nonnegative() }),
              ),
            }),
          ),
        }),
        pageSchema.extend({
          resource: z.literal("history"),
          pet: z.object({
            petId: z.string(),
            name: z.string(),
            currentWeight: z.number(),
            updatedAt: z.string(),
          }),
          items: z.array(z.object({ timestamp: z.string(), weight: z.number() })),
        }),
      ]),
      annotations: annotations(true, false, true, false),
      policy: {
        sideEffects: [],
        cost: "No external traffic or monetary cost",
        recommendedPolicy: "allow",
      },
      execute: (input) =>
        Effect.gen(function* () {
          if (input.resource === "list") {
            const pets = yield* getAllPetsWithHistory();
            return {
              resource: "list",
              pets: yield* Effect.forEach(pets, (pet) =>
                getDailyVisitCounts(pet.pet_id).pipe(
                  Effect.map((visits) => ({
                    petId: pet.pet_id,
                    name: pet.name,
                    currentWeight: pet.current_weight,
                    updatedAt: pet.updated_at,
                    recentWeights: pet.weightHistory
                      .slice(-input.historyLimit)
                      .map((row) => ({
                        timestamp: row.timestamp,
                        weight: row.weight,
                      })),
                    recentVisits: visits.slice(-input.historyLimit),
                  })),
                ),
              ),
            };
          }
          const pet = yield* getPet(input.petId);
          if (!pet) throw new Error("Pet not found");
          const history = yield* getWeightHistory(input.petId);
          return {
            resource: "history",
            pet: {
              petId: pet.pet_id,
              name: pet.name,
              currentWeight: pet.current_weight,
              updatedAt: pet.updated_at,
            },
            ...paginate(
              history.map((row) => ({
                timestamp: row.timestamp,
                weight: row.weight,
              })),
              input.cursor,
              input.limit,
            ),
          };
        }),
    }),
    defineTool({
      name: "costs_read",
      title: "Read Omni Cost Telemetry",
      description:
        "Summarize Omni's persisted model, search, TTS, retrieval, and transcription costs for a fixed time range. Unknown-price events remain explicit.",
      inputSchema: z
        .object({
          days: z.union([z.literal(7), z.literal(30), z.literal(90)]).default(30),
        })
        .strict(),
      outputSchema: z.object({
        range: z.object({
          days: z.number().nullable(),
          from: nullableNumber,
          to: z.number(),
        }),
        summary: z.object({
          selectedCostCents: z.number(),
          allTimeCostCents: z.number(),
          allTimeUnknownEventCount: z.number().int().nonnegative(),
          averageDailyCostCents: z.number(),
          highestDay: z.object({ date: z.string(), costCents: z.number() }).nullable(),
          eventCount: z.number().int().nonnegative(),
          unknownEventCount: z.number().int().nonnegative(),
          inputTokens: z.number().nonnegative(),
          outputTokens: z.number().nonnegative(),
          characters: z.number().nonnegative(),
          requests: z.number().nonnegative(),
          credits: z.number().nonnegative(),
        }),
        daily: z.array(
          z.object({
            date: z.string(),
            costCents: z.number(),
            byFeature: z.record(z.string(), z.number()),
            pricedEventCount: z.number().int().nonnegative(),
            unknownEventCount: z.number().int().nonnegative(),
          }),
        ),
        byFeature: z.array(
          z.object({
            feature: z.string(),
            costCents: z.number(),
            eventCount: z.number().int().nonnegative(),
            unknownEventCount: z.number().int().nonnegative(),
          }),
        ),
        byService: z.array(
          z.object({
            service: z.string(),
            model: nullableString,
            category: z.string(),
            costCents: z.number(),
            eventCount: z.number().int().nonnegative(),
            unknownEventCount: z.number().int().nonnegative(),
            inputTokens: z.number().nonnegative(),
            inputNoCacheTokens: z.number().nonnegative(),
            cacheReadTokens: z.number().nonnegative(),
            cacheWriteTokens: z.number().nonnegative(),
            outputTokens: z.number().nonnegative(),
            reasoningTokens: z.number().nonnegative(),
            characters: z.number().nonnegative(),
            requests: z.number().nonnegative(),
            credits: z.number().nonnegative(),
          }),
        ),
        recent: z.array(
          z.object({
            eventId: z.string(),
            incurredAt: z.number(),
            category: z.string(),
            feature: z.string(),
            operation: z.string(),
            service: z.string(),
            model: nullableString,
            costCents: nullableNumber,
            priceStatus: z.string(),
            usage: costUsageSchema,
            runId: nullableString,
          }),
        ),
      }),
      annotations: annotations(true, false, true, false),
      policy: {
        sideEffects: [],
        cost: "No external traffic or monetary cost",
        recommendedPolicy: "allow",
      },
      execute: ({ days }) =>
        Effect.gen(function* () {
          const count = yield* CostEventEntity.count();
          if (count > MAX_MCP_COST_EVENTS) {
            throw new Error(
              `Cost telemetry exceeds the MCP scan limit (${count} events; maximum ${MAX_MCP_COST_EVENTS})`,
            );
          }
          return summarizeCosts(yield* getCostEvents(), { days, timeZone: config.TZ });
        }),
    }),
  ];
}
