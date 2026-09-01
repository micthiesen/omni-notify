import { useEffect, useState } from "react";
import { Clock, Effect, Schedule } from "effect";
import { forkUiEffect } from "../effect";

/** Re-renders on an interval; returns the current epoch ms. */
export function useNow(intervalMs: number): number {
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    return forkUiEffect(
      Effect.repeat(
        Clock.currentTimeMillis.pipe(
          Effect.tap((value) => Effect.sync(() => setNow(value))),
        ),
        Schedule.spaced(`${intervalMs} millis`),
      ),
    );
  }, [intervalMs]);

  return now;
}
