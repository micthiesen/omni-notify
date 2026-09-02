import { Docstore } from "@micthiesen/mitools/docstore";
import { Logger, LogLevel } from "@micthiesen/mitools/logging";
import { Pushover } from "@micthiesen/mitools/pushover";
import { Karakeep } from "@micthiesen/mitools/karakeep";
import { Effect, Layer, ManagedRuntime } from "effect";

export const TestLayer = Layer.mergeAll(
  Docstore.layerMemory,
  Logger.layer({ level: LogLevel.ERROR }),
  Pushover.layerNoop,
  Karakeep.layerDisabled,
).pipe(Layer.orDie);

export const testRuntime = ManagedRuntime.make(TestLayer);
export const runTest = testRuntime.runPromise;
export const provideTest = <A, E, R>(effect: Effect.Effect<A, E, R>) =>
  effect.pipe(Effect.provide(TestLayer), Effect.scoped);
