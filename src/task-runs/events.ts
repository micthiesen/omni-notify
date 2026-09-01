import type { Effect as EffectType } from "effect/Effect";
import { Effect } from "effect";
import type { TaskRunLogLine } from "./persistence.js";

export interface TaskRunEvent {
  type: "run-started" | "run-finished";
  taskName: string;
}

export type RunLogEvent =
  | { type: "line"; runId: string; line: TaskRunLogLine }
  | { type: "end"; runId: string };

class Bus<TEvent> {
  private listeners = new Set<(event: TEvent) => void>();

  public emit(event: TEvent): void {
    for (const listener of this.listeners) {
      try {
        listener(event);
      } catch (error) {
        // A dashboard/SSE observer is never allowed to corrupt task state or
        // replace the task's original failure.
        console.error("Task event subscriber failed", error);
      }
    }
  }

  public emitEffect(event: TEvent): EffectType<void> {
    return Effect.sync(() => this.emit(event));
  }

  /** Returns an unsubscribe function. */
  public subscribe(listener: (event: TEvent) => void): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }
}

/**
 * Process-local pub/sub for task run lifecycle events. The dashboard's SSE
 * endpoint subscribes so connected clients get fresh state the moment any
 * task starts or finishes (which is also when streamer status can change).
 */
export const taskRunBus = new Bus<TaskRunEvent>();

/**
 * Per-line log events for in-flight task runs. The per-run log SSE endpoint
 * subscribes while a log viewer is open; nothing is emitted to the dashboard
 * snapshot stream.
 */
export const runLogBus = new Bus<RunLogEvent>();
