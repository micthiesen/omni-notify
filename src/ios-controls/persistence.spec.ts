import { Injector } from "@micthiesen/mitools/config";
import { LogLevel } from "@micthiesen/mitools/logging";
import { afterEach, describe, expect, it } from "vitest";
import {
  deleteIOSControlRegistration,
  IOSControlRegistrationEntity,
  listIOSControlRegistrations,
  markIOSControlDelivered,
  replaceDeviceRegistrations,
} from "./persistence.js";

Injector.configure({
  config: {
    LOG_LEVEL: LogLevel.INFO,
    PUSHOVER_TOKEN: "fake-token",
    PUSHOVER_USER: "fake-user",
    DOCKERIZED: false,
    DB_NAME: "ios-control-persistence.spec.db",
  },
});

afterEach(() => IOSControlRegistrationEntity.deleteAll());

describe("replaceDeviceRegistrations", () => {
  it("replaces one device without touching another", () => {
    replaceDeviceRegistrations(
      "device-one",
      [
        {
          controlId: "old",
          slot: 1,
          pushToken: "a".repeat(64),
          environment: "sandbox",
        },
      ],
      1,
    );
    replaceDeviceRegistrations(
      "device-two",
      [
        {
          controlId: "keep",
          slot: 2,
          pushToken: "b".repeat(64),
          environment: "production",
        },
      ],
      2,
    );
    replaceDeviceRegistrations(
      "device-one",
      [
        {
          controlId: "new",
          slot: 4,
          pushToken: "c".repeat(64),
          environment: "sandbox",
        },
      ],
      3,
    );

    expect(
      listIOSControlRegistrations()
        .map((row) => row.registrationId)
        .sort(),
    ).toEqual(["device-one:new", "device-two:keep"]);
  });

  it("preserves delivered state only while token configuration is unchanged", () => {
    replaceDeviceRegistrations("device-one", [
      {
        controlId: "slot-one",
        slot: 1,
        pushToken: "a".repeat(64),
        environment: "sandbox",
      },
    ]);
    markIOSControlDelivered("device-one:slot-one", "a".repeat(64), "state-hash");

    const unchanged = replaceDeviceRegistrations("device-one", [
      {
        controlId: "slot-one",
        slot: 1,
        pushToken: "a".repeat(64),
        environment: "sandbox",
      },
    ]);
    expect(unchanged[0].lastDeliveredHash).toBe("state-hash");

    const rotated = replaceDeviceRegistrations("device-one", [
      {
        controlId: "slot-one",
        slot: 1,
        pushToken: "b".repeat(64),
        environment: "sandbox",
      },
    ]);
    expect(rotated[0].lastDeliveredHash).toBeUndefined();

    markIOSControlDelivered(
      "device-one:slot-one",
      "a".repeat(64),
      "stale-in-flight-hash",
    );
    expect(listIOSControlRegistrations()[0].lastDeliveredHash).toBeUndefined();

    deleteIOSControlRegistration("device-one:slot-one", "a".repeat(64));
    expect(listIOSControlRegistrations()[0].pushToken).toBe("b".repeat(64));
  });
});
