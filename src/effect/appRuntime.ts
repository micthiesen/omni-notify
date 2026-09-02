import type { EffectRunner } from "@micthiesen/mitools/boundary";
import { Docstore } from "@micthiesen/mitools/docstore";
import { Karakeep } from "@micthiesen/mitools/karakeep";
import { Logger } from "@micthiesen/mitools/logging";
import { Pushover } from "@micthiesen/mitools/pushover";
import { Scheduler } from "@micthiesen/mitools/scheduling";
import { Sqlite } from "@micthiesen/mitools/sqlite";
import { Context, Effect, Layer } from "effect";
import { FetchHttpClient } from "effect/unstable/http";
import { throttleLogHook } from "../alerts/throttle.js";
import { taskLogTap } from "../task-runs/logCapture.js";
import type { TaskServices } from "../task-runs/registry.js";
import config from "../utils/config.js";

export type AppServices = TaskServices | Sqlite | Scheduler;

const sqliteLayer = Sqlite.layer({
  path: config.DOCKERIZED ? `/data/${config.DB_NAME}` : config.DB_NAME,
});

const pushoverLayer = config.PUSHOVER_USER
  ? Pushover.layer({ user: config.PUSHOVER_USER, token: config.PUSHOVER_TOKEN }).pipe(
      Layer.provide(FetchHttpClient.layer),
    )
  : Pushover.layerNoop;

const karakeepLayer =
  config.KARAKEEP_URL && config.KARAKEEP_API_KEY
    ? Karakeep.layer({
        baseUrl: config.KARAKEEP_URL,
        apiKey: config.KARAKEEP_API_KEY,
      }).pipe(Layer.provide(FetchHttpClient.layer))
    : Karakeep.layerDisabled;

const loggerSinks = config.LOGS_PATH
  ? [
      Logger.consoleSink,
      Logger.dailyFileSink({
        directory: config.LOGS_PATH,
        prefix: "omni-notify",
      }),
    ]
  : undefined;

const infrastructureLayer = Layer.mergeAll(
  sqliteLayer,
  pushoverLayer,
  karakeepLayer,
  Scheduler.layer,
);

const servicesLayer = Layer.mergeAll(
  Docstore.layer,
  Logger.layer({
    level: config.LOG_LEVEL,
    sinks: loggerSinks,
    onLog: taskLogTap,
    onError: throttleLogHook(Pushover.logHook),
  }),
).pipe(Layer.provideMerge(infrastructureLayer));

export const AppLayer = Logger.layerAdapter.pipe(Layer.provideMerge(servicesLayer));

/** Build an Effect runner from the current application context for framework callbacks. */
export function runnerFromContext<R>(context: Context.Context<R>): EffectRunner<R> {
  return {
    runPromise: (effect, options) =>
      Effect.runPromise(Effect.provide(effect, context), options),
    runFork: (effect, options) =>
      Effect.runFork(Effect.provide(effect, context), options),
  };
}
