import type { NamedLogger } from "@micthiesen/mitools/logging";
import { Effect } from "effect";
import { afterAll, beforeEach, describe, expect, it, vi } from "vitest";
import { createMitoolsTestRuntime } from "../test/mitools.js";
import type { WorkspaceDefinition } from "./types.js";

const mocks = vi.hoisted(() => ({
  runWorkspace: vi.fn(),
  subjects: [
    {
      workspaceId: "marketplace-selling",
      subjectId: "desk",
      title: "Standing Desk",
      status: "active" as const,
      summary: "Drafting the listing",
      createdAt: 1,
      updatedAt: 1,
    },
  ],
}));

vi.mock("./engine.js", () => ({
  runWorkspaceEffect: (...args: unknown[]) =>
    Effect.promise(() => mocks.runWorkspace(...args)),
}));
vi.mock("./persistence.js", () => ({
  listWorkspaceSubjects: () => Effect.succeed(mocks.subjects),
}));

import { WorkspaceTask } from "./task.js";

const runtime = createMitoolsTestRuntime();
afterAll(() => runtime.dispose());

const definition: WorkspaceDefinition = {
  id: "marketplace-selling",
  title: "Marketplace Selling",
  description: "",
  subjectLabel: "Item",
  subjectLabelPlural: "Items",
  taskName: "MarketplaceSelling",
  schedule: "0 0 9 * * 0",
  scheduledRuns: false,
  instructions: "",
  artifacts: [],
};

const logger = {
  extend: vi.fn().mockReturnValue({ warn: vi.fn(() => Effect.void) }),
} as unknown as NamedLogger;

describe("WorkspaceTask on-demand mode", () => {
  beforeEach(() => {
    mocks.runWorkspace.mockReset();
  });

  it("skips scheduled work even when an active subject exists", async () => {
    const task = new WorkspaceTask(definition, logger);

    await runtime.run(task.run);

    expect(mocks.runWorkspace).not.toHaveBeenCalled();
    expect(task.getLastRunSummary()).toBe(
      "On-demand workspace; scheduled refresh skipped",
    );
  });

  it("still responds to a manual message", async () => {
    mocks.runWorkspace.mockResolvedValue({
      summary: "Updated the listing draft",
      updatedSubjects: 1,
      createdActions: 0,
    });
    const task = new WorkspaceTask(definition, logger);

    await runtime.run(task.runManual({ message: "The desk is 60 inches wide" }));

    expect(mocks.runWorkspace).toHaveBeenCalledWith(
      definition,
      {
        trigger: "message",
        message: "The desk is 60 inches wide",
        subjectId: undefined,
      },
      expect.anything(),
    );
    expect(task.getLastRunSummary()).toBe("Updated the listing draft");
  });
});
