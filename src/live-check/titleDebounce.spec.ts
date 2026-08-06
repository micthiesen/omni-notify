import { describe, expect, it } from "vitest";
import { TITLE_CHANGE_COOLDOWN_MS, TitleChangeDebouncer } from "./titleDebounce.js";

describe("TitleChangeDebouncer", () => {
  describe("no prior state (cold start / post-restart)", () => {
    it("notifies immediately on the first observed change", () => {
      const d = new TitleChangeDebouncer();
      const result = d.observe("s1", {
        currentTitle: "B",
        titleChanged: true,
        now: 1000,
      });
      expect(result).toEqual({ action: "notify", title: "B" });
    });

    it("does nothing on an unrelated tick", () => {
      const d = new TitleChangeDebouncer();
      const result = d.observe("s1", {
        currentTitle: "A",
        titleChanged: false,
        now: 1000,
      });
      expect(result).toEqual({ action: "none" });
    });
  });

  describe("seed-on-live behavior", () => {
    it("holds a title change immediately after seeding on go-live", () => {
      const d = new TitleChangeDebouncer();
      d.seed("s1", "Live title", 0);
      const result = d.observe("s1", {
        currentTitle: "Fixed title",
        titleChanged: true,
        now: 5000,
      });
      expect(result).toEqual({ action: "none" });
    });

    it("notifies immediately once the cooldown from seeding has fully elapsed", () => {
      const d = new TitleChangeDebouncer();
      d.seed("s1", "A", 0);
      const result = d.observe("s1", {
        currentTitle: "B",
        titleChanged: true,
        now: TITLE_CHANGE_COOLDOWN_MS,
      });
      expect(result).toEqual({ action: "notify", title: "B" });
    });
  });

  describe("hold and last-wins", () => {
    it("holds multiple changes within the cooldown and fires the last one on expiry", () => {
      const d = new TitleChangeDebouncer();
      d.seed("s1", "T0", 0);
      expect(
        d.observe("s1", { currentTitle: "T1", titleChanged: true, now: 1000 }),
      ).toEqual({ action: "none" });
      expect(
        d.observe("s1", { currentTitle: "T2", titleChanged: true, now: 2000 }),
      ).toEqual({ action: "none" });

      const result = d.observe("s1", {
        currentTitle: "T2",
        titleChanged: false,
        now: TITLE_CHANGE_COOLDOWN_MS + 1,
      });
      expect(result).toEqual({ action: "notify", title: "T2" });
    });

    it("keeps holding on ticks before the cooldown expires", () => {
      const d = new TitleChangeDebouncer();
      d.seed("s1", "T0", 0);
      d.observe("s1", { currentTitle: "T1", titleChanged: true, now: 1000 });
      const stillHeld = d.observe("s1", {
        currentTitle: "T1",
        titleChanged: false,
        now: TITLE_CHANGE_COOLDOWN_MS - 1,
      });
      expect(stillHeld).toEqual({ action: "none" });
    });
  });

  describe("trailing fire restarts the cooldown", () => {
    it("holds a change made right after a trailing fire instead of notifying immediately", () => {
      const d = new TitleChangeDebouncer();
      d.seed("s1", "T0", 0);
      d.observe("s1", { currentTitle: "T1", titleChanged: true, now: 1000 });

      const trailingFire = d.observe("s1", {
        currentTitle: "T1",
        titleChanged: false,
        now: TITLE_CHANGE_COOLDOWN_MS + 1,
      });
      expect(trailingFire).toEqual({ action: "notify", title: "T1" });

      const heldAfterFire = d.observe("s1", {
        currentTitle: "T2",
        titleChanged: true,
        now: TITLE_CHANGE_COOLDOWN_MS + 2,
      });
      expect(heldAfterFire).toEqual({ action: "none" });

      const secondTrailingFire = d.observe("s1", {
        currentTitle: "T2",
        titleChanged: false,
        now: 2 * TITLE_CHANGE_COOLDOWN_MS + 3,
      });
      expect(secondTrailingFire).toEqual({ action: "notify", title: "T2" });
    });
  });

  describe("A→B→A round trip", () => {
    it("does not re-notify when the held title matches what was last notified", () => {
      const d = new TitleChangeDebouncer();
      d.seed("s1", "A", 0);
      d.observe("s1", { currentTitle: "B", titleChanged: true, now: 1000 });
      d.observe("s1", { currentTitle: "A", titleChanged: true, now: 2000 });

      const result = d.observe("s1", {
        currentTitle: "A",
        titleChanged: false,
        now: TITLE_CHANGE_COOLDOWN_MS + 1,
      });
      expect(result).toEqual({ action: "none" });
    });

    it("clears the pending title even when the round trip suppresses the notify", () => {
      const d = new TitleChangeDebouncer();
      d.seed("s1", "A", 0);
      d.observe("s1", { currentTitle: "B", titleChanged: true, now: 1000 });
      d.observe("s1", { currentTitle: "A", titleChanged: true, now: 2000 });
      d.observe("s1", {
        currentTitle: "A",
        titleChanged: false,
        now: TITLE_CHANGE_COOLDOWN_MS + 1,
      });

      // No pending title left to fire on a later tick.
      const later = d.observe("s1", {
        currentTitle: "A",
        titleChanged: false,
        now: 2 * TITLE_CHANGE_COOLDOWN_MS,
      });
      expect(later).toEqual({ action: "none" });
    });
  });

  describe("clear", () => {
    it("drops a pending held title so it can't fire later", () => {
      const d = new TitleChangeDebouncer();
      d.seed("s1", "A", 0);
      d.observe("s1", { currentTitle: "B", titleChanged: true, now: 1000 });
      d.clear("s1");

      const result = d.observe("s1", {
        currentTitle: "B",
        titleChanged: false,
        now: TITLE_CHANGE_COOLDOWN_MS + 1,
      });
      expect(result).toEqual({ action: "none" });
    });

    it("resets to cold-start behavior: the next change notifies immediately", () => {
      const d = new TitleChangeDebouncer();
      d.seed("s1", "A", 0);
      d.clear("s1");

      const result = d.observe("s1", {
        currentTitle: "B",
        titleChanged: true,
        now: 1000,
      });
      expect(result).toEqual({ action: "notify", title: "B" });
    });
  });

  it("tracks state independently per streamer", () => {
    const d = new TitleChangeDebouncer();
    d.seed("s1", "A", 0);
    d.seed("s2", "X", 0);

    const heldS1 = d.observe("s1", {
      currentTitle: "B",
      titleChanged: true,
      now: 1000,
    });
    const notifyS2 = d.observe("s2", {
      currentTitle: "Y",
      titleChanged: true,
      now: TITLE_CHANGE_COOLDOWN_MS,
    });

    expect(heldS1).toEqual({ action: "none" });
    expect(notifyS2).toEqual({ action: "notify", title: "Y" });
  });
});
