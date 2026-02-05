export abstract class ScheduledTask {
  /** Human-readable name for logging */
  public abstract readonly name: string;

  /** Cron expression for node-cron (6 fields: second minute hour day month weekday) */
  public abstract readonly schedule: string;

  /** Optional max jitter in ms added before each run (default: 0) */
  public readonly jitterMs: number = 0;

  /** Whether to run immediately on startup (default: false) */
  public readonly runOnStartup: boolean = false;

  /** Execute the task */
  public abstract run(): Promise<void>;
}
