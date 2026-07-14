import { useCallback, useEffect, useState } from "react";
import { ApiError, fetchTasks, runTaskRequest } from "../api";
import type { TaskInfo } from "../api";

const ACTIVE_POLL_MS = 3_000;
const IDLE_POLL_MS = 15_000;

export interface RunResult {
  ok: boolean;
  message: string;
}

export interface UseTasksResult {
  tasks: TaskInfo[] | null;
  loading: boolean;
  error: string | null;
  runTask: (name: string) => Promise<RunResult>;
  refresh: () => void;
}

/**
 * Polls GET /api/tasks — every 3s while any task is running,
 * every 15s otherwise. `runTask` fires POST /api/tasks/:name/run
 * and immediately restarts polling.
 */
export function useTasks(): UseTasksResult {
  const [tasks, setTasks] = useState<TaskInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pollGen, setPollGen] = useState(0);

  useEffect(() => {
    let cancelled = false;
    let timer: number | undefined;

    async function poll() {
      let delay = IDLE_POLL_MS;
      try {
        const data = await fetchTasks();
        if (cancelled) return;
        setTasks(data.tasks);
        setError(null);
        if (data.tasks.some((t) => t.running)) delay = ACTIVE_POLL_MS;
      } catch (err) {
        if (cancelled) return;
        setError(err instanceof Error ? err.message : "Failed to fetch tasks");
      }
      timer = window.setTimeout(poll, delay);
    }

    poll();
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [pollGen]);

  const refresh = useCallback(() => {
    setPollGen((gen) => gen + 1);
  }, []);

  const runTask = useCallback(
    async (name: string): Promise<RunResult> => {
      try {
        await runTaskRequest(name);
        refresh();
        return { ok: true, message: `${name} started` };
      } catch (err) {
        if (err instanceof ApiError && err.status === 409) {
          refresh();
          return { ok: false, message: `${name} is already running` };
        }
        const message =
          err instanceof Error ? err.message : "Failed to start task";
        return { ok: false, message };
      }
    },
    [refresh],
  );

  return { tasks, loading: tasks === null && error === null, error, runTask, refresh };
}
