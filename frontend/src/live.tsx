import { createContext, useCallback, useContext, useEffect, useState } from "react";
import type { ReactNode } from "react";
import { Effect, Ref, Schedule, Schema } from "effect";
import { ApiError, fetchSnapshot, runTaskRequest, SnapshotSchema } from "./api";
import type { ManualRunOptions, Snapshot } from "./api";
import { forkUiEffect, makeUiCallbackRuntime, runUiEffect } from "./effect";

export type ConnectionState = "connecting" | "live" | "polling";

export interface RunResult {
  ok: boolean;
  message: string;
}

interface LiveDataValue {
  snapshot: Snapshot | null;
  connection: ConnectionState;
  error: string | null;
  runTask: (name: string, options?: ManualRunOptions) => Promise<RunResult>;
}

const LiveDataContext = createContext<LiveDataValue | null>(null);

/**
 * Single source of dashboard state for the whole app. Subscribes to the
 * server's SSE stream (`/api/events`) for realtime snapshots, and falls back
 * to polling `/api/snapshot` whenever the stream is down.
 */
export function LiveDataProvider({ children }: { children: ReactNode }) {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [connection, setConnection] = useState<ConnectionState>("connecting");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const lifecycle = Effect.scoped(
      Effect.gen(function* () {
        const isLive = yield* Ref.make(false);

        const poll = Ref.get(isLive).pipe(
          Effect.flatMap((live) =>
            live
              ? Effect.void
              : fetchSnapshot().pipe(
                  Effect.tap((next) =>
                    Ref.get(isLive).pipe(
                      Effect.tap((becameLive) =>
                        becameLive
                          ? Effect.void
                          : Effect.sync(() => {
                              setSnapshot(next);
                              setError(null);
                            }),
                      ),
                    ),
                  ),
                ),
          ),
          Effect.catch((cause) =>
            Effect.sync(() => {
              setError(
                cause instanceof Error ? cause.message : "Failed to fetch snapshot",
              );
            }),
          ),
        );

        // Fetch immediately for first paint, then continue as a fallback while
        // EventSource is disconnected. Ref prevents stale polls replacing SSE data.
        yield* Effect.forkScoped(Effect.repeat(poll, Schedule.spaced("10 seconds")));

        const connect = Effect.scoped(
          Effect.gen(function* () {
            const runCallback = yield* makeUiCallbackRuntime();
            yield* Effect.acquireUseRelease(
              Effect.sync(() => new EventSource("/api/events")),
              (source) =>
                Effect.callback<void>((resume) => {
                  const onSnapshot = (event: Event) => {
                    const data = (event as MessageEvent<string>).data;
                    runCallback(
                      Effect.try({
                        try: () => JSON.parse(data) as unknown,
                        catch: (cause) => cause,
                      }).pipe(
                        Effect.flatMap(Schema.decodeUnknownEffect(SnapshotSchema)),
                        Effect.tap(() => Ref.set(isLive, true)),
                        Effect.tap((next) =>
                          Effect.sync(() => {
                            setConnection("live");
                            setError(null);
                            setSnapshot(next);
                          }),
                        ),
                        Effect.catch((cause) =>
                          Effect.sync(() =>
                            setError(
                              cause instanceof Error
                                ? cause.message
                                : "Invalid live snapshot",
                            ),
                          ),
                        ),
                      ),
                    );
                  };
                  const onError = () => {
                    runCallback(
                      Ref.set(isLive, false).pipe(
                        Effect.tap(() => Effect.sync(() => setConnection("polling"))),
                        Effect.tap(() =>
                          source.readyState === EventSource.CLOSED
                            ? Effect.sync(() => resume(Effect.void))
                            : Effect.void,
                        ),
                      ),
                    );
                  };
                  source.addEventListener("snapshot", onSnapshot);
                  source.addEventListener("error", onError);
                  return Effect.sync(() => {
                    source.removeEventListener("snapshot", onSnapshot);
                    source.removeEventListener("error", onError);
                  });
                }),
              (source) => Effect.sync(() => source.close()),
            );
          }),
        );

        yield* Effect.repeat(connect, Schedule.spaced("5 seconds"));
      }),
    );
    return forkUiEffect(lifecycle);
  }, []);

  const runTask = useCallback(
    async (name: string, options?: ManualRunOptions): Promise<RunResult> => {
      try {
        await runUiEffect(runTaskRequest(name, options));
        // The SSE snapshot lands ~200ms later; flip the flag now so the button
        // reacts instantly.
        setSnapshot((prev) =>
          prev
            ? {
                ...prev,
                tasks: prev.tasks.map((t) =>
                  t.name === name ? { ...t, running: true } : t,
                ),
              }
            : prev,
        );
        return { ok: true, message: `${name} started` };
      } catch (err) {
        if (err instanceof ApiError && err.status === 409) {
          return { ok: false, message: `${name} is already running` };
        }
        return {
          ok: false,
          message: err instanceof Error ? err.message : "Failed to start task",
        };
      }
    },
    [],
  );

  return (
    <LiveDataContext.Provider value={{ snapshot, connection, error, runTask }}>
      {children}
    </LiveDataContext.Provider>
  );
}

export function useLiveData(): LiveDataValue {
  const value = useContext(LiveDataContext);
  if (!value) throw new Error("useLiveData must be used within LiveDataProvider");
  return value;
}
