import { useCallback, useEffect, useRef, useState } from "react";
import { Effect, Schedule } from "effect";
import { forkUiEffect } from "../effect";

function pageVisible(): boolean {
  return typeof document === "undefined" || document.visibilityState === "visible";
}

/**
 * Run `load` now and every `intervalMs` while the tab is visible. Hidden tabs
 * stop polling; returning to the tab fetches immediately. Changing `key`
 * restarts the loop (use it for filters). `refresh` forces an immediate reload.
 */
export function useVisiblePoll<A, E>(
  key: string,
  load: () => Effect.Effect<A, E>,
  handlers: { onSuccess: (value: A) => void; onFailure: (error: E) => void },
  intervalMs = 10_000,
): { refresh: () => void } {
  const [visible, setVisible] = useState(pageVisible);
  const [nonce, setNonce] = useState(0);
  const loadRef = useRef(load);
  const handlersRef = useRef(handlers);
  useEffect(() => {
    loadRef.current = load;
    handlersRef.current = handlers;
  });

  useEffect(() => {
    const onChange = () => setVisible(pageVisible());
    document.addEventListener("visibilitychange", onChange);
    return () => document.removeEventListener("visibilitychange", onChange);
  }, []);

  useEffect(() => {
    if (!visible) return;
    const once = Effect.suspend(() => loadRef.current()).pipe(
      Effect.match({
        onFailure: (error) => handlersRef.current.onFailure(error),
        onSuccess: (value) => handlersRef.current.onSuccess(value),
      }),
    );
    return forkUiEffect(Effect.repeat(once, Schedule.spaced(`${intervalMs} millis`)));
  }, [visible, key, nonce, intervalMs]);

  const refresh = useCallback(() => setNonce((n) => n + 1), []);
  return { refresh };
}
