import { describe, expect, it } from "vitest";
import type { StreamerStatus } from "./persistence.js";
import { type FetchedStatus, LiveStatus, Platform } from "./platforms/index.js";
import type { PlatformBinding } from "./streamers.js";
import { type BindingFetchResult, decideTransition } from "./transitions.js";

const YT: PlatformBinding = { platform: Platform.YouTube, username: "@yt" };
const TW: PlatformBinding = { platform: Platform.Twitch, username: "tw" };
const KI: PlatformBinding = { platform: Platform.Kick, username: "ki" };

const now = new Date("2026-04-17T00:00:00Z");
const earlier = new Date("2026-04-16T00:00:00Z");

const live = (title = "t", viewers = 100): FetchedStatus => ({
  status: LiveStatus.Live,
  title,
  viewerCount: viewers,
});
const offline = (): FetchedStatus => ({ status: LiveStatus.Offline });
const unknown = (err = "boom"): FetchedStatus => ({
  status: LiveStatus.Unknown,
  error: err,
});

const offlineStatus: StreamerStatus = { streamerId: "s", isLive: false };
const liveStatus = (
  primary: PlatformBinding = KI,
  primaryTitle = "prev",
  maxViewerCount = 50,
): StreamerStatus => ({
  streamerId: "s",
  isLive: true,
  primary,
  primaryTitle,
  startedAt: earlier,
  maxViewerCount,
});

describe("decideTransition", () => {
  it("returns all-unknown when every binding errors", () => {
    const results: BindingFetchResult[] = [
      { binding: YT, status: unknown("e1") },
      { binding: KI, status: unknown("e2") },
    ];
    const d = decideTransition("s", offlineStatus, results, now);
    expect(d.kind).toBe("all-unknown");
    if (d.kind === "all-unknown") {
      expect(d.errors).toEqual(["e1", "e2"]);
    }
  });

  it("keeps previous state when partial unknown + no lives", () => {
    const results: BindingFetchResult[] = [
      { binding: YT, status: unknown() },
      { binding: KI, status: offline() },
    ];
    const d = decideTransition("s", liveStatus(KI), results, now);
    expect(d.kind).toBe("no-change");
  });

  it("fires went-live when offline → any live (priority tiebreak)", () => {
    const results: BindingFetchResult[] = [
      { binding: YT, status: live("yt-title", 10) },
      { binding: KI, status: live("kick-title", 50) },
    ];
    const d = decideTransition("s", offlineStatus, results, now);
    expect(d.kind).toBe("went-live");
    if (d.kind === "went-live") {
      // YouTube beats Kick in priority tiebreak
      expect(d.next.primary).toEqual(YT);
      expect(d.next.primaryTitle).toBe("yt-title");
      expect(d.next.maxViewerCount).toBe(60);
      expect(d.summedViewerCount).toBe(60);
      expect(d.next.startedAt).toEqual(now);
    }
  });

  it("fires went-offline when live → all confirmed offline", () => {
    const results: BindingFetchResult[] = [
      { binding: YT, status: offline() },
      { binding: KI, status: offline() },
    ];
    const d = decideTransition("s", liveStatus(KI, "t", 200), results, now);
    expect(d.kind).toBe("went-offline");
    if (d.kind === "went-offline") {
      expect(d.next.isLive).toBe(false);
      expect(d.previousLive.primary).toEqual(KI);
      expect(d.next.lastEndedAt).toEqual(now);
      if (d.next.lastEndedAt) {
        expect(d.next.lastStartedAt).toEqual(earlier);
        expect(d.next.lastMaxViewerCount).toBe(200);
      }
    }
  });

  it("keeps previous primary when it is still live (stickiness)", () => {
    // Previously primary was Kick. Now YouTube is also live; Kick still live.
    // Primary must stay Kick even though YouTube has higher priority.
    const results: BindingFetchResult[] = [
      { binding: YT, status: live("yt", 10) },
      { binding: KI, status: live("kick-new-title", 100) },
    ];
    const d = decideTransition("s", liveStatus(KI, "kick-old"), results, now);
    expect(d.kind).toBe("still-live");
    if (d.kind === "still-live") {
      expect(d.next.primary).toEqual(KI);
      expect(d.primarySwitched).toBe(false);
      expect(d.titleChanged).toBe(true);
      expect(d.next.primaryTitle).toBe("kick-new-title");
    }
  });

  it("re-elects primary when the previous primary drops and other bindings remain live", () => {
    // Previously primary was Kick. Now Kick is offline but YouTube is still live.
    const results: BindingFetchResult[] = [
      { binding: YT, status: live("yt-title", 10) },
      { binding: KI, status: offline() },
    ];
    const d = decideTransition("s", liveStatus(KI, "kick-title"), results, now);
    expect(d.kind).toBe("still-live");
    if (d.kind === "still-live") {
      expect(d.next.primary).toEqual(YT);
      expect(d.primarySwitched).toBe(true);
      // Primary switched → no title-change notification even though "title" is different
      expect(d.titleChanged).toBe(false);
    }
  });

  it("does not fire title change when primary unchanged and title unchanged", () => {
    const results: BindingFetchResult[] = [{ binding: KI, status: live("same", 60) }];
    const d = decideTransition("s", liveStatus(KI, "same", 50), results, now);
    expect(d.kind).toBe("still-live");
    if (d.kind === "still-live") {
      expect(d.titleChanged).toBe(false);
      expect(d.primarySwitched).toBe(false);
      expect(d.next.maxViewerCount).toBe(60);
    }
  });

  it("sums viewer counts across live bindings", () => {
    const results: BindingFetchResult[] = [
      { binding: YT, status: live("t", 1200) },
      { binding: TW, status: live("t", 800) },
      { binding: KI, status: live("t", 500) },
    ];
    const d = decideTransition("s", offlineStatus, results, now);
    expect(d.kind).toBe("went-live");
    if (d.kind === "went-live") {
      expect(d.summedViewerCount).toBe(2500);
      expect(d.next.primary).toEqual(YT);
    }
  });

  it("returns no-change when already offline and all bindings still offline", () => {
    const results: BindingFetchResult[] = [
      { binding: YT, status: offline() },
      { binding: KI, status: offline() },
    ];
    const d = decideTransition("s", offlineStatus, results, now);
    expect(d.kind).toBe("no-change");
  });

  it("treats unknown + live as live (keeps streamer live)", () => {
    const results: BindingFetchResult[] = [
      { binding: YT, status: unknown() },
      { binding: KI, status: live("kick", 100) },
    ];
    const d = decideTransition("s", offlineStatus, results, now);
    expect(d.kind).toBe("went-live");
    if (d.kind === "went-live") {
      expect(d.next.primary).toEqual(KI);
      expect(d.summedViewerCount).toBe(100);
    }
  });
});
