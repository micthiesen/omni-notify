import type { Logger } from "@micthiesen/mitools/logging";
import { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { z } from "zod";
import { runWorkspace } from "./engine.js";
import { listWorkspaceSubjects } from "./persistence.js";
import type {
  WorkspaceDefinition,
  WorkspaceManualInput,
  WorkspaceRunResult,
} from "./types.js";

const manualInputSchema: z.ZodType<WorkspaceManualInput> = z.object({
  message: z.string().trim().min(1).max(20_000),
  subjectId: z.string().min(1).optional(),
  trigger: z.enum(["message", "email"]).optional(),
});

export class WorkspaceTask extends ScheduledTask {
  public readonly name: string;
  public readonly displayName: string;
  public readonly schedule: string;
  private readonly logger: Logger;
  private lastSummary?: string;

  public constructor(
    private readonly definition: WorkspaceDefinition,
    parentLogger: Logger,
  ) {
    super();
    this.name = definition.taskName;
    this.displayName = definition.title;
    this.schedule = definition.schedule;
    this.logger = parentLogger.extend(`${definition.taskName}Task`);
  }

  public async run(): Promise<void> {
    if (this.definition.scheduledRuns === false) {
      this.lastSummary = "On-demand workspace; scheduled refresh skipped";
      return;
    }
    const subjects = listWorkspaceSubjects(this.definition.id).filter(
      (subject) => subject.status === "active",
    );
    if (subjects.length === 0) {
      this.lastSummary = "No active subjects to research";
      return;
    }
    let updated = 0;
    let actions = 0;
    const failures: string[] = [];
    for (const subject of subjects) {
      try {
        const result = await runWorkspace(
          this.definition,
          { trigger: "scheduled", subjectId: subject.subjectId },
          this.logger,
        );
        updated += result.updatedSubjects;
        actions += result.createdActions;
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        failures.push(`${subject.title}: ${message}`);
        this.logger.warn(`Workspace research failed for ${subject.title}`, message);
      }
    }
    this.lastSummary = `Updated ${updated} subject(s), proposed ${actions} action(s), ${failures.length} failed`;
    if (failures.length > 0) {
      throw new AggregateError(
        failures.map((failure) => new Error(failure)),
        this.lastSummary,
      );
    }
  }

  public async runManual(input: unknown): Promise<void> {
    const parsed = manualInputSchema.parse(input);
    const result: WorkspaceRunResult = await runWorkspace(
      this.definition,
      {
        trigger: parsed.trigger ?? "message",
        message: parsed.message,
        subjectId: parsed.subjectId,
      },
      this.logger,
    );
    this.lastSummary = result.summary;
  }

  public getLastRunSummary(): string | undefined {
    return this.lastSummary;
  }
}
