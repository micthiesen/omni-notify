import { Logger, type LogNotification } from "@micthiesen/mitools/logging";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { AlertThrottle, alertKey, installAlertThrottle } from "./throttle.js";

const MINUTE = 60_000;

describe("alertKey", () => {
  it("normalizes case and whitespace", () => {
    expect(alertKey("A", "  Boom   happened ")).toBe(alertKey("A", "Boom happened"));
  });

  it("keeps messages that differ by an embedded identifier apart", () => {
    expect(alertKey("A", 'Failed to submit "1Z999AA10123456784"')).not.toBe(
      alertKey("A", 'Failed to submit "1Z999AA10123456785"'),
    );
  });

  it("scopes keys by logger name", () => {
    expect(alertKey("A", "boom")).not.toBe(alertKey("B", "boom"));
  });
});

describe("AlertThrottle", () => {
  const alert = { key: "k", title: "Boom", body: "details" };

  it("delivers the first occurrence immediately", () => {
    const throttle = new AlertThrottle();
    expect(throttle.admit(alert, 0)).toEqual({ title: "Boom", body: "details" });
  });

  it("suppresses repeats inside the cooldown", () => {
    const throttle = new AlertThrottle();
    throttle.admit(alert, 0);
    expect(throttle.admit(alert, MINUTE)).toBeNull();
    expect(throttle.admit(alert, 5 * MINUTE)).toBeNull();
  });

  it("re-delivers after the cooldown with the repeat count", () => {
    const throttle = new AlertThrottle();
    throttle.admit(alert, 0);
    throttle.admit(alert, MINUTE);
    throttle.admit(alert, 2 * MINUTE);
    const admitted = throttle.admit(alert, 16 * MINUTE);
    expect(admitted?.body).toBe("details\n\nRepeated 3 times in the last 16m.");
  });

  it("leaves the body alone when nothing was suppressed in between", () => {
    const throttle = new AlertThrottle();
    throttle.admit(alert, 0);
    expect(throttle.admit(alert, 20 * MINUTE)).toEqual({
      title: "Boom",
      body: "details",
    });
  });

  it("backs the cooldown off as an alert keeps repeating", () => {
    const throttle = new AlertThrottle();
    throttle.admit(alert, 0);
    expect(throttle.admit(alert, 16 * MINUTE)).not.toBeNull();
    // Second cooldown is 30min, so 16min later is still suppressed.
    expect(throttle.admit(alert, 32 * MINUTE)).toBeNull();
    expect(throttle.admit(alert, 47 * MINUTE)).not.toBeNull();
  });

  it("treats an alert as fresh again after a long silence", () => {
    const throttle = new AlertThrottle();
    throttle.admit(alert, 0);
    expect(throttle.admit(alert, 7 * 60 * MINUTE)).toEqual({
      title: "Boom",
      body: "details",
    });
  });

  it("tracks distinct keys independently", () => {
    const throttle = new AlertThrottle();
    throttle.admit(alert, 0);
    expect(throttle.admit({ ...alert, key: "other" }, MINUTE)).not.toBeNull();
  });

  it("evicts the least recently seen keys past the ceiling", () => {
    const throttle = new AlertThrottle({ maxKeys: 2 });
    throttle.admit({ ...alert, key: "a" }, 0);
    throttle.admit({ ...alert, key: "b" }, 1);
    throttle.admit({ ...alert, key: "c" }, 2);
    // "a" was evicted, so its next occurrence reads as new rather than suppressed.
    expect(throttle.admit({ ...alert, key: "a" }, 3)).not.toBeNull();
    expect(throttle.admit({ ...alert, key: "c" }, 4)).toBeNull();
  });

  it("keeps an actively repeating key over a dormant one when evicting", () => {
    const throttle = new AlertThrottle({ maxKeys: 2 });
    throttle.admit({ ...alert, key: "hot" }, 0);
    throttle.admit({ ...alert, key: "dormant" }, 1);
    throttle.admit({ ...alert, key: "hot" }, 2);
    throttle.admit({ ...alert, key: "new" }, 3);
    // "dormant" was the eviction victim, so "hot" is still throttled.
    expect(throttle.admit({ ...alert, key: "hot" }, 4)).toBeNull();
    expect(throttle.admit({ ...alert, key: "dormant" }, 5)).not.toBeNull();
  });
});

describe("installAlertThrottle", () => {
  const originalOnError = Logger.onError;
  const originalOnWarn = Logger.onWarn;
  let delivered: LogNotification[];

  beforeEach(() => {
    delivered = [];
    Logger.onError = (notification) => {
      delivered.push(notification);
    };
    installAlertThrottle();
  });

  afterEach(() => {
    Logger.onError = originalOnError;
    Logger.onWarn = originalOnWarn;
  });

  it("chains the existing hook and drops immediate repeats", () => {
    const logger = new Logger("Test");
    logger.error("Boom", "first");
    logger.error("Boom", "second");
    logger.error("Different boom");

    expect(delivered.map((d) => d.title)).toEqual(["Boom", "Different boom"]);
    expect(delivered[0].body).toBe("first");
  });

  it("leaves onWarn unset while mitools has no warn hook", () => {
    Logger.onWarn = null;
    installAlertThrottle();
    expect(Logger.onWarn).toBeNull();
  });
});
