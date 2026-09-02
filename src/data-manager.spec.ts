import { Entity } from "@micthiesen/mitools/entities";
import { Effect, Option } from "effect";
import { afterAll, afterEach, beforeEach, describe, expect, it } from "vitest";
import { createManagedEntity, MALFORMED_ROW_KEY } from "./data-manager.js";
import { createMitoolsTestRuntime } from "./test/mitools.js";

const mitools = createMitoolsTestRuntime();
afterAll(() => mitools.dispose());

type TestRow = {
  group: string;
  id: string;
  status: "ready" | "running";
  value: number;
};

const TestEntity = new Entity<TestRow, ["group", "id"]>("data-manager-test", [
  "group",
  "id",
]);

describe("managed entities", () => {
  beforeEach(() => mitools.run(TestEntity.deleteAll()));
  afterEach(() => mitools.run(TestEntity.deleteAll()));

  it("exposes metadata and rows without losing composite keys", async () => {
    await mitools.run(
      TestEntity.upsert({ group: "a#b", id: "1", status: "ready", value: 42 }),
    );
    const managed = createManagedEntity(TestEntity, {
      label: "Test rows",
      description: "Test data",
    });

    expect(managed.primaryKey).toEqual(["group", "id"]);
    expect(await mitools.run(managed.count)).toBe(1);
    expect(await mitools.run(managed.storageBytes())).toBeGreaterThan(0);
    expect(await mitools.run(managed.rows())).toEqual([
      { group: "a#b", id: "1", status: "ready", value: 42 },
    ]);
  });

  it("requires the exact primary key and deletes only the matching row", async () => {
    await mitools.run(
      TestEntity.upsert({ group: "a", id: "1", status: "ready", value: 1 }),
    );
    await mitools.run(
      TestEntity.upsert({ group: "a", id: "2", status: "ready", value: 2 }),
    );
    const managed = createManagedEntity(TestEntity, {
      label: "Test rows",
      description: "Test data",
    });

    expect(await mitools.run(managed.delete({ group: "a" }))).toEqual({
      status: "invalid-key",
    });
    expect(
      await mitools.run(managed.delete({ group: "a", id: "1", extra: true })),
    ).toEqual({
      status: "invalid-key",
    });
    expect((await mitools.run(managed.delete({ group: "a", id: "1" }))).status).toBe(
      "deleted",
    );
    expect(
      Option.getOrUndefined(await mitools.run(TestEntity.get({ group: "a", id: "1" }))),
    ).toBeUndefined();
    expect(
      Option.getOrUndefined(await mitools.run(TestEntity.get({ group: "a", id: "2" })))
        ?.value,
    ).toBe(2);
    expect(await mitools.run(managed.delete({ group: "a", id: "1" }))).toEqual({
      status: "not-found",
    });
  });

  it("supports deletion guards and post-delete cleanup", async () => {
    await mitools.run(
      TestEntity.upsert({ group: "a", id: "1", status: "running", value: 1 }),
    );
    await mitools.run(
      TestEntity.upsert({ group: "a", id: "2", status: "ready", value: 2 }),
    );
    const cleaned: string[] = [];
    const managed = createManagedEntity(TestEntity, {
      label: "Test rows",
      description: "Test data",
      canDelete: (row) =>
        row.status === "running" ? "Running rows are protected." : undefined,
      afterDelete: (row) => Effect.sync(() => cleaned.push(row.id)),
    });

    expect(await mitools.run(managed.delete({ group: "a", id: "1" }))).toEqual({
      status: "blocked",
      reason: "Running rows are protected.",
    });
    expect((await mitools.run(managed.delete({ group: "a", id: "2" }))).status).toBe(
      "deleted",
    );
    expect(cleaned).toEqual(["2"]);
  });

  it("isolates malformed blobs and allows exact raw-key deletion", async () => {
    await mitools.run(
      TestEntity.upsert({ group: "a", id: "1", status: "ready", value: 1 }),
    );
    const rawKey = TestEntity.getPk({ group: "a", id: "1" });
    (await mitools.database)
      .prepare("UPDATE blobs SET data = ? WHERE pk = ?")
      .run(Buffer.alloc(0), rawKey);
    const managed = createManagedEntity(TestEntity, {
      label: "Test rows",
      description: "Test data",
    });

    const rows = await mitools.run(managed.rows());
    expect(rows).toHaveLength(1);
    expect(rows[0]?.[MALFORMED_ROW_KEY]).toMatchObject({ rawKey });
    expect(await mitools.run(managed.delete(rows[0] ?? {}))).toMatchObject({
      status: "deleted",
    });
    expect(await mitools.run(managed.count)).toBe(0);
  });
});
