import { createHash, randomUUID } from "node:crypto";
import { LogFile } from "@micthiesen/mitools/logfile";
import type { NamedLogger } from "@micthiesen/mitools/logging";
import type { Logger } from "@micthiesen/mitools/logging";
import type { Docstore } from "@micthiesen/mitools/docstore";
import type { Pushover } from "@micthiesen/mitools/pushover";
import { LogLevel } from "@micthiesen/mitools/logging";
import { codeBlock, logTimestamp } from "@micthiesen/mitools/markdown";
import { notify } from "@micthiesen/mitools/pushover";
import { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { generateText, isStepCount, tool } from "ai";
import { Clock, Effect } from "effect";
import { z } from "zod";
import { hasPrice, llmCostCents } from "../ai/cost.js";
import { getBriefingModel } from "../ai/registry.js";
import { makeFetchUrlTool } from "../ai/tools/fetchUrl.js";
import { makeWebSearchTool } from "../ai/tools/webSearch.js";
import { runnerFromContext } from "../effect/appRuntime.js";
import { getCurrentRunId } from "../task-runs/logCapture.js";
import config from "../utils/config.js";
import {
  addBriefingNotification,
  completeBriefingDelivery,
  distributeBriefingRunCost,
  releaseBriefingDelivery,
  reserveBriefingDelivery,
} from "./persistence.js";
import { resolveAllPlaceholders } from "./placeholders.js";

export interface BriefingConfig {
  name: string;
  schedule: string;
  prompt: string;
}

export class BriefingAgentTask implements ScheduledTask<
  unknown,
  Logger | Docstore | Pushover
> {
  public readonly name: string;
  public readonly schedule: string;
  protected readonly prompt: string;

  protected logger: NamedLogger;

  public static create(briefingConfig: BriefingConfig, parentLogger: NamedLogger) {
    return config.TAVILY_API_KEY
      ? Effect.succeed(new BriefingAgentTask(briefingConfig, parentLogger))
      : parentLogger
          .info(`${briefingConfig.name} disabled: missing TAVILY_API_KEY`)
          .pipe(Effect.as(null));
  }

  protected constructor(briefingConfig: BriefingConfig, parentLogger: NamedLogger) {
    this.name = briefingConfig.name;
    this.schedule = briefingConfig.schedule;
    this.prompt = briefingConfig.prompt;
    this.logger = parentLogger.extend(`${briefingConfig.name}Task`);
  }

  public get run() {
    return this.runEffect();
  }

  public runEffect() {
    return Effect.gen({ self: this }, function* () {
      const context = yield* Effect.context<Logger | Docstore | Pushover>();
      const toolRunner = runnerFromContext(yield* Effect.context<Logger | Docstore>());
      const run = <A, E>(effect: Effect.Effect<A, E, Logger | Docstore | Pushover>) =>
        Effect.runPromise(Effect.provide(effect, context));
      const logFile = config.LOGS_PATH
        ? yield* LogFile.make(
            `${config.LOGS_PATH}/briefings/${this.name}-${logTimestamp()}.md`,
            "overwrite",
          )
        : undefined;

      const resolvedPrompt = yield* resolveAllPlaceholders(this.prompt, this.name);
      const { model, modelId } = getBriefingModel();

      if (logFile) {
        yield* logFile.log(
          this.logger,
          LogLevel.INFO,
          `Briefing Prompt (${modelId})`,
          codeBlock(resolvedPrompt),
          {
            consoleSummary: `Starting briefing agent (${modelId}) [${resolvedPrompt.length} chars]`,
          },
        );
      } else {
        yield* this.logger.info(
          `Starting briefing agent (${modelId}) with prompt:\n${resolvedPrompt}`,
        );
      }

      let notificationSent = false;

      const tools = {
        web_search: makeWebSearchTool(toolRunner),
        fetch_url: makeFetchUrlTool(toolRunner),
        send_notification: tool({
          description:
            "Send a push notification to the user with your briefing. Call this once you have something interesting to share.",
          inputSchema: z.object({
            title: z
              .string()
              .describe(
                "Short title for the notification, prefixed with a relevant emoji (e.g. '🌸 Cherry Blossom Festival in Vancouver')",
              ),
            message: z.string().describe("The notification body with your summary"),
            url: z.string().url().describe("URL to the source"),
            url_title: z.string().describe("Link text for the URL (e.g. 'Read more')"),
          }),
          execute: ({ title, message, url, url_title }) =>
            run(
              Effect.gen({ self: this }, function* () {
                const runId = getCurrentRunId();
                const contentHash = createHash("sha256")
                  .update(JSON.stringify({ title, message, url }))
                  .digest("hex")
                  .slice(0, 24);
                const deliveryId = `${runId ?? randomUUID()}:${contentHash}`;
                const reserved = yield* reserveBriefingDelivery(this.name, deliveryId);
                if (!reserved) return { success: true, duplicate: true };
                yield* this.logger.info(`Sending notification: ${title}`);
                yield* notify({
                  title,
                  message,
                  url,
                  url_title,
                  token: config.PUSHOVER_BRIEFING_TOKEN,
                }).pipe(
                  Effect.tapError(() => releaseBriefingDelivery(this.name, deliveryId)),
                );
                // Mark delivered before updating the user-facing archive. If the
                // process dies between these local writes, a retry is suppressed
                // rather than risking a duplicate push.
                yield* completeBriefingDelivery(this.name, deliveryId);
                yield* addBriefingNotification(this.name, {
                  title,
                  message,
                  url,
                  timestamp: yield* Clock.currentTimeMillis,
                  runId,
                });
                notificationSent = true;
                return { success: true };
              }),
            ),
        }),
      };

      const { steps, usage } = yield* Effect.tryPromise(() =>
        generateText({
          model,
          // Per-provider options are ignored by the other providers, so both are
          // safe to declare regardless of which model BRIEFING_MODEL resolves to.
          providerOptions: {
            google: { thinkingConfig: { thinkingLevel: "high" as const } },
            openai: { reasoningEffort: "high" as const },
          },
          tools,
          stopWhen: isStepCount(20),
          onStepFinish: ({ text, reasoning, toolCalls, toolResults }) =>
            run(
              Effect.gen({ self: this }, function* () {
                const reasoningParts = reasoning.filter((r) => r.type === "reasoning");
                if (reasoningParts.length > 0) {
                  const reasoningText = reasoningParts.map((r) => r.text).join("\n");
                  if (logFile)
                    yield* logFile.section("Reasoning", codeBlock(reasoningText));
                }
                if (text) {
                  if (logFile) yield* logFile.section("Step Text", text);
                  yield* this.logger.debug(`Step text: ${text}`);
                }
                for (const call of toolCalls) {
                  if (call.toolName === "web_search") {
                    const input = call.input as { query?: string };
                    yield* this.logger.info(`Search: "${input.query}"`);
                  } else if (call.toolName === "fetch_url") {
                    const input = call.input as { url?: string };
                    yield* this.logger.info(`Fetching: ${input.url}`);
                  } else if (call.toolName !== "send_notification") {
                    yield* this.logger.info(`Tool call: ${call.toolName}`, call.input);
                  }
                  if (logFile)
                    yield* logFile.section(
                      `Tool Call: ${call.toolName}`,
                      codeBlock(JSON.stringify(call.input, null, 2), "json"),
                    );
                }
                for (const result of toolResults) {
                  if (result.toolName === "web_search") {
                    const output = result.output as Record<string, unknown>;
                    const count = Array.isArray(output?.results)
                      ? output.results.length
                      : "?";
                    const time =
                      typeof output?.responseTime === "number"
                        ? ` (${output.responseTime.toFixed(1)}s)`
                        : "";
                    yield* this.logger.debug(`Search returned ${count} results${time}`);
                  } else if (result.toolName === "fetch_url") {
                    const output = result.output as Record<string, unknown>;
                    const chars =
                      typeof output?.content === "string" ? output.content.length : "?";
                    const truncated = output?.truncated ? " (truncated)" : "";
                    yield* this.logger.debug(`Fetched ${chars} chars${truncated}`);
                  } else if (result.toolName !== "send_notification") {
                    yield* this.logger.info(
                      `Tool result: ${result.toolName}`,
                      result.output,
                    );
                  }
                  if (logFile)
                    yield* logFile.section(
                      `Tool Result: ${result.toolName}`,
                      codeBlock(
                        JSON.stringify(result.output, null, 2).slice(0, 5000),
                        "json",
                      ),
                    );
                }
              }),
            ),
          prompt: resolvedPrompt,
        }),
      );

      if (logFile) {
        yield* logFile.log(
          this.logger,
          LogLevel.INFO,
          "Result",
          `Completed in ${steps.length} steps`,
        );
      } else {
        yield* this.logger.info(`Agent completed in ${steps.length} steps`);
      }

      // Total token usage across all steps is only known once generateText
      // resolves, but the notification (if any) was created earlier inside the
      // send_notification tool — so the cost is backfilled onto that row here.
      if (notificationSent) {
        const runId = getCurrentRunId();
        if (hasPrice(modelId)) {
          const costCents = llmCostCents(modelId, {
            inputTokens: usage.inputTokens ?? 0,
            outputTokens: usage.outputTokens ?? 0,
          });
          yield* distributeBriefingRunCost(this.name, runId, costCents);
        } else {
          yield* this.logger.debug(
            `No pricing data for model ${modelId}; cost not recorded`,
          );
          yield* distributeBriefingRunCost(this.name, runId, null);
        }
      }
    });
  }
}
