import { afterEach, describe, expect, it } from "vitest";
import { Effect } from "effect";
import { runTest } from "../live-check/testRuntime.js";
import {
  deleteIOSControlRegistration,
  IOSControlRegistrationEntity,
  listIOSControlRegistrations,
  markIOSControlDelivered,
  replaceDeviceRegistrations,
} from "./persistence.js";

afterEach(() => runTest(IOSControlRegistrationEntity.deleteAll()));

const control = (controlId: string, slot: number, token: string) => ({
  controlId,
  slot,
  pushToken: token.repeat(64),
  environment: "sandbox" as const,
});

describe("replaceDeviceRegistrations", () => {
  it("replaces one device without touching another", async () => {
    await runTest(replaceDeviceRegistrations("device-one", [control("old", 1, "a")]));
    await runTest(replaceDeviceRegistrations("device-two", [control("keep", 2, "b")]));
    await runTest(replaceDeviceRegistrations("device-one", [control("new", 4, "c")]));
    expect(
      (await runTest(listIOSControlRegistrations()))
        .map((row) => row.registrationId)
        .sort(),
    ).toEqual(["device-one:new", "device-two:keep"]);
  });

  it("preserves delivered state only while token configuration is unchanged", async () => {
    await runTest(
      replaceDeviceRegistrations("device-one", [control("slot-one", 1, "a")]),
    );
    await runTest(
      markIOSControlDelivered("device-one:slot-one", "a".repeat(64), "state-hash"),
    );
    const unchanged = await runTest(
      replaceDeviceRegistrations("device-one", [control("slot-one", 1, "a")]),
    );
    expect(unchanged[0].lastDeliveredHash).toBe("state-hash");
    const rotated = await runTest(
      replaceDeviceRegistrations("device-one", [control("slot-one", 1, "b")]),
    );
    expect(rotated[0].lastDeliveredHash).toBeUndefined();
    await runTest(
      markIOSControlDelivered("device-one:slot-one", "a".repeat(64), "stale"),
    );
    await runTest(deleteIOSControlRegistration("device-one:slot-one", "a".repeat(64)));
    expect((await runTest(listIOSControlRegistrations()))[0].pushToken).toBe(
      "b".repeat(64),
    );
  });

  it("never commits a mixed registration set under concurrent replacements", async () => {
    const first = [control("a", 1, "a"), control("b", 2, "b")];
    const second = [control("c", 3, "c"), control("d", 4, "d")];
    await runTest(
      Effect.all(
        [
          replaceDeviceRegistrations("device-one", first),
          replaceDeviceRegistrations("device-one", second),
        ],
        { concurrency: "unbounded", discard: true },
      ),
    );
    const ids = (await runTest(listIOSControlRegistrations()))
      .map((row) => row.controlId)
      .sort();
    expect([
      ["a", "b"],
      ["c", "d"],
    ]).toContainEqual(ids);
  });
});
