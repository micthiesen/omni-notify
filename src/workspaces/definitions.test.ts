import { describe, expect, it } from "vitest";
import { marketplaceSellingWorkspace, workspaceDefinitions } from "./definitions.js";

describe("workspace definitions", () => {
  it("keeps workspace, task, and artifact identifiers unique", () => {
    expect(new Set(workspaceDefinitions.map(({ id }) => id)).size).toBe(
      workspaceDefinitions.length,
    );
    expect(new Set(workspaceDefinitions.map(({ taskName }) => taskName)).size).toBe(
      workspaceDefinitions.length,
    );

    for (const workspace of workspaceDefinitions) {
      expect(new Set(workspace.artifacts.map(({ key }) => key)).size).toBe(
        workspace.artifacts.length,
      );
    }
  });

  it("keeps Marketplace Selling user-driven and listing-complete", () => {
    expect(marketplaceSellingWorkspace.scheduledRuns).toBe(false);
    expect(marketplaceSellingWorkspace.artifacts.map(({ key }) => key)).toEqual(
      expect.arrayContaining([
        "item-details",
        "listing-fields",
        "pricing",
        "photos",
        "progress",
      ]),
    );
    expect(marketplaceSellingWorkspace.instructions).toContain("inactivity is normal");
    expect(marketplaceSellingWorkspace.instructions).toContain("Never publish");
  });
});
