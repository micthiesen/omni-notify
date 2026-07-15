import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useState,
} from "react";
import type { ReactNode } from "react";
import { ApiError, fetchSnapshot, runTaskRequest } from "./api";
import type { Snapshot } from "./api";

export type ConnectionState = "connecting" | "live" | "polling";

export interface RunResult {
  ok: boolean;
  message: string;
}

interface LiveDataValue {
  snapshot: Snapshot | null;
  connection: ConnectionState;
  error: string | null;
  runTask: (name: string) => Promise<RunResult>;
}

const LiveDataContext = createContext<LiveDataValue | null>(null);

const POLL_MS = 10_000;
const RECONNECT_MS = 5_000;

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
    let closed = false;
    let source: EventSource | null = null;
    let reconnectTimer: number | undefined;
    let pollTimer: number | undefined;

    const stopPolling = () => {
      window.clearTimeout(pollTimer);
      pollTimer = undefined;
    };

    const poll = async () => {
      try {
        const snap = await fetchSnapshot();
        if (closed) return;
        setSnapshot(snap);
        setError(null);
      } catch (err) {
        if (closed) return;
        setError(err instanceof Error ? err.message : "Failed to fetch snapshot");
      }
      if (!closed) pollTimer = window.setTimeout(() => void poll(), POLL_MS);
    };

    const connect = () => {
      source = new EventSource("/api/events");
      source.addEventListener("snapshot", (event) => {
        if (closed) return;
        stopPolling();
        setConnection("live");
        setError(null);
        setSnapshot(JSON.parse((event as MessageEvent<string>).data) as Snapshot);
      });
      source.onerror = () => {
        if (closed) return;
        // EventSource retries transient failures itself; only rebuild the
        // connection when it gives up. Poll while disconnected either way.
        setConnection("polling");
        if (pollTimer === undefined) void poll();
        if (source?.readyState === EventSource.CLOSED) {
          source.close();
          reconnectTimer = window.setTimeout(connect, RECONNECT_MS);
        }
      };
    };

    connect();
    return () => {
      closed = true;
      source?.close();
      window.clearTimeout(reconnectTimer);
      stopPolling();
    };
  }, []);

  const runTask = useCallback(async (name: string): Promise<RunResult> => {
    try {
      await runTaskRequest(name);
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
  }, []);

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
