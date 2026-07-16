import { EventEmitter } from "node:events";
import type { TaskRunLogLine } from "./persistence.js";

export interface TaskRunEvent {
  type: "run-started" | "run-finished";
  taskName: string;
}

export type RunLogEvent =
  | { type: "line"; runId: string; line: TaskRunLogLine }
  | { type: "end"; runId: string };

class Bus<TEvent> {
  private emitter = new EventEmitter();

  public constructor() {
    // One listener per connected dashboard/log viewer; the default cap of 10
    // is too low.
    this.emitter.setMaxListeners(0);
  }

  public emit(event: TEvent): void {
    this.emitter.emit("event", event);
  }

  /** Returns an unsubscribe function. */
  public subscribe(listener: (event: TEvent) => void): () => void {
    this.emitter.on("event", listener);
    return () => {
      this.emitter.off("event", listener);
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
