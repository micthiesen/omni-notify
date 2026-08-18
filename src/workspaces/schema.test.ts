import { describe, expect, it } from "vitest";
import { z } from "zod";
import { workspaceOutputSchema } from "./engine.js";

function findKeyword(value: unknown, keyword: string): boolean {
  if (Array.isArray(value)) return value.some((item) => findKeyword(item, keyword));
  if (!value || typeof value !== "object") return false;
  const record = value as Record<string, unknown>;
  return (
    keyword in record ||
    Object.values(record).some((item) => findKeyword(item, keyword))
  );
}

describe("workspace structured output schema", () => {
  it("does not emit oneOf, which OpenAI response formats reject", () => {
    const jsonSchema = z.toJSONSchema(workspaceOutputSchema);

    expect(findKeyword(jsonSchema, "oneOf")).toBe(false);
  });

  it("uses one shared proposal shape for email and calendar actions", () => {
    const base = {
      subject_id: "subject-1",
      title: "Proposal",
      description: "Review this",
      senders: [],
      domains: [],
      subject_keywords: [],
      body_keywords: [],
    };
    const output = {
      response: "Done",
      subjects: [],
      sources: [],
      proposals: [{ ...base, type: "email_scope", event: null }],
      notification: null,
    };

    expect(workspaceOutputSchema.parse(output).proposals[0]?.event).toBeNull();
  });
});
