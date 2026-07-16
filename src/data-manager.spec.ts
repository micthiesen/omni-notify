import { Injector } from "@micthiesen/mitools/config";
import { getDb } from "@micthiesen/mitools/docstore";
import { Entity } from "@micthiesen/mitools/entities";
import { LogLevel } from "@micthiesen/mitools/logging";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { createManagedEntity, MALFORMED_ROW_KEY } from "./data-manager.js";

Injector.configure({
  config: {
    LOG_LEVEL: LogLevel.ERROR,
    PUSHOVER_TOKEN: "fake-token",
    PUSHOVER_USER: "fake-user",
    DOCKERIZED: false,
    DB_NAME: "data-manager.spec.db",
  },
});

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
  beforeEach(() => TestEntity.deleteAll());
  afterEach(() => TestEntity.deleteAll());

  it("exposes metadata and rows without losing composite keys", () => {
    TestEntity.upsert({ group: "a#b", id: "1", status: "ready", value: 42 });
    const managed = createManagedEntity(TestEntity, {
      label: "Test rows",
      description: "Test data",
    });

    expect(managed.primaryKey).toEqual(["group", "id"]);
    expect(managed.count()).toBe(1);
    expect(managed.storageBytes()).toBeGreaterThan(0);
    expect(managed.rows()).toEqual([
      { group: "a#b", id: "1", status: "ready", value: 42 },
    ]);
  });

  it("requires the exact primary key and deletes only the matching row", () => {
    TestEntity.upsert({ group: "a", id: "1", status: "ready", value: 1 });
    TestEntity.upsert({ group: "a", id: "2", status: "ready", value: 2 });
    const managed = createManagedEntity(TestEntity, {
      label: "Test rows",
      description: "Test data",
    });

    expect(managed.delete({ group: "a" })).toEqual({ status: "invalid-key" });
    expect(managed.delete({ group: "a", id: "1", extra: true })).toEqual({
      status: "invalid-key",
    });
    expect(managed.delete({ group: "a", id: "1" }).status).toBe("deleted");
    expect(TestEntity.get({ group: "a", id: "1" })).toBeUndefined();
    expect(TestEntity.get({ group: "a", id: "2" })?.value).toBe(2);
    expect(managed.delete({ group: "a", id: "1" })).toEqual({
      status: "not-found",
    });
  });

  it("supports deletion guards and post-delete cleanup", () => {
    TestEntity.upsert({ group: "a", id: "1", status: "running", value: 1 });
    TestEntity.upsert({ group: "a", id: "2", status: "ready", value: 2 });
    const cleaned: string[] = [];
    const managed = createManagedEntity(TestEntity, {
      label: "Test rows",
      description: "Test data",
      canDelete: (row) =>
        row.status === "running" ? "Running rows are protected." : undefined,
      afterDelete: (row) => cleaned.push(row.id),
    });

    expect(managed.delete({ group: "a", id: "1" })).toEqual({
      status: "blocked",
      reason: "Running rows are protected.",
    });
    expect(managed.delete({ group: "a", id: "2" }).status).toBe("deleted");
    expect(cleaned).toEqual(["2"]);
  });

  it("isolates malformed blobs and allows exact raw-key deletion", () => {
    TestEntity.upsert({ group: "a", id: "1", status: "ready", value: 1 });
    const rawKey = TestEntity.getPk({ group: "a", id: "1" });
    getDb()
      .prepare("UPDATE blobs SET data = ? WHERE pk = ?")
      .run(Buffer.alloc(0), rawKey);
    const managed = createManagedEntity(TestEntity, {
      label: "Test rows",
      description: "Test data",
    });

    const rows = managed.rows();
    expect(rows).toHaveLength(1);
    expect(rows[0]?.[MALFORMED_ROW_KEY]).toMatchObject({ rawKey });
    expect(managed.delete(rows[0] ?? {})).toMatchObject({ status: "deleted" });
    expect(managed.count()).toBe(0);
  });
});
