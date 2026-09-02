import { Docstore } from "@micthiesen/mitools/docstore";
import { Karakeep } from "@micthiesen/mitools/karakeep";
import {
  Logger,
  type CapturedLogs,
  type LoggerOptions,
} from "@micthiesen/mitools/logging";
import { Pushover } from "@micthiesen/mitools/pushover";
import { Scheduler } from "@micthiesen/mitools/scheduling";
import { Sqlite } from "@micthiesen/mitools/sqlite";
import { Effect, Layer, ManagedRuntime } from "effect";
import type { EffectRunner } from "@micthiesen/mitools/boundary";
import type { AppServices } from "../effect/appRuntime.js";

export type TestServices = AppServices | CapturedLogs;

/** Isolated in-memory mitools runtime for a single test module. */
export function createMitoolsTestRuntime(options?: LoggerOptions) {
  const infrastructure = Layer.mergeAll(
    Sqlite.layer({ path: ":memory:" }),
    Pushover.layerNoop,
    Karakeep.layerDisabled,
    Scheduler.layer,
  );
  const services = Layer.mergeAll(Docstore.layer, Logger.layerCapture(options)).pipe(
    Layer.provideMerge(infrastructure),
  );
  const layer = Logger.layerAdapter.pipe(Layer.provideMerge(services), Layer.orDie);
  const runtime = ManagedRuntime.make(layer);
  const runner: EffectRunner<AppServices> = {
    runPromise: (effect, options) => runtime.runPromise(effect, options),
    runFork: (effect, options) => runtime.runFork(effect, options),
  };
  return {
    logger: Logger.named("Test"),
    run: <A, E, R>(effect: Effect.Effect<A, E, R>) =>
      runtime.runPromise(effect as Effect.Effect<A, E, TestServices>),
    runFork: <A, E, R>(effect: Effect.Effect<A, E, R>) =>
      runtime.runFork(effect as Effect.Effect<A, E, TestServices>),
    dispose: () => runtime.dispose(),
    database: runtime.runPromise(Sqlite.pipe(Effect.map(({ db }) => db))),
    runner,
  };
}
