import { Docstore } from "@micthiesen/mitools/docstore";
import { expect, layer } from "@effect/vitest";
import { Effect, Option } from "effect";
import {
  applyWorkspaceTransaction,
  WorkspaceSubjectEntity,
  type WorkspaceSubjectData,
} from "./persistence.js";

layer(Docstore.layerMemory)("workspace transactions", (it) => {
  it.effect("rolls back every write when the commit fails", () =>
    Effect.gen(function* () {
      const subject: WorkspaceSubjectData = {
        workspaceId: "purchase-research",
        subjectId: "subject-1",
        title: "Camera",
        status: "active",
        summary: "Researching cameras",
        createdAt: 1,
        updatedAt: 1,
      };

      const result = yield* Effect.result(
        applyWorkspaceTransaction((transaction) => {
          transaction.upsert(
            WorkspaceSubjectEntity,
            {
              workspaceId: subject.workspaceId,
              subjectId: subject.subjectId,
            },
            subject,
          );
          throw new Error("abort commit");
        }),
      );

      expect(result._tag).toBe("Failure");
      expect(
        Option.getOrUndefined(
          yield* WorkspaceSubjectEntity.get({
            workspaceId: subject.workspaceId,
            subjectId: subject.subjectId,
          }),
        ),
      ).toBeUndefined();
    }),
  );
});
