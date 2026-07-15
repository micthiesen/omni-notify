import { EventEmitter } from "node:events";

export interface TaskRunEvent {
  type: "run-started" | "run-finished";
  taskName: string;
}

/**
 * Process-local pub/sub for task run lifecycle events. The dashboard's SSE
 * endpoint subscribes so connected clients get fresh state the moment any
 * task starts or finishes (which is also when streamer status can change).
 */
class TaskRunBus {
  private emitter = new EventEmitter();

  public constructor() {
    // One listener per connected dashboard; the default cap of 10 is too low.
    this.emitter.setMaxListeners(0);
  }

  public emit(event: TaskRunEvent): void {
    this.emitter.emit("event", event);
  }

  /** Returns an unsubscribe function. */
  public subscribe(listener: (event: TaskRunEvent) => void): () => void {
    this.emitter.on("event", listener);
    return () => {
      this.emitter.off("event", listener);
    };
  }
}

export const taskRunBus = new TaskRunBus();
