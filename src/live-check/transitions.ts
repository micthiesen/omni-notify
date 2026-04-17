import type {
  StreamerStatus,
  StreamerStatusLive,
  StreamerStatusOffline,
} from "./persistence.js";
import { type FetchedStatus, LiveStatus } from "./platforms/index.js";
import { comparePlatformPriority, type PlatformBinding } from "./streamers.js";

export type BindingFetchResult = {
  binding: PlatformBinding;
  status: FetchedStatus;
};

export type TickDecision =
  | { kind: "all-unknown"; errors: string[] }
  | { kind: "no-change" }
  | {
      kind: "went-live";
      next: StreamerStatusLive;
      summedViewerCount: number;
    }
  | {
      kind: "went-offline";
      previousLive: StreamerStatusLive;
      next: StreamerStatusOffline;
    }
  | {
      kind: "still-live";
      next: StreamerStatusLive;
      summedViewerCount: number;
      /**
       * True when the primary binding is unchanged AND its title changed since
       * the last observation. Only case where a title-change notification fires.
       */
      titleChanged: boolean;
      primarySwitched: boolean;
    };

export function decideTransition(
  streamerId: string,
  previous: StreamerStatus,
  results: BindingFetchResult[],
  now: Date = new Date(),
): TickDecision {
  const unknowns = results.filter((r) => r.status.status === LiveStatus.Unknown);
  const lives = results.filter(
    (
      r,
    ): r is {
      binding: PlatformBinding;
      status: Extract<FetchedStatus, { status: LiveStatus.Live }>;
    } => r.status.status === LiveStatus.Live,
  );
  const allUnknown = unknowns.length === results.length && results.length > 0;
  const anyLive = lives.length > 0;
  const anyUnknown = unknowns.length > 0;

  if (allUnknown) {
    return {
      kind: "all-unknown",
      errors: unknowns.map((u) =>
        u.status.status === LiveStatus.Unknown ? u.status.error : "",
      ),
    };
  }

  if (!anyLive) {
    // Either some unknown + no lives (keep previous — might still be live on
    // the unknown bindings), or fully offline. Either way, no transition
    // unless previously live.
    if (!previous.isLive || anyUnknown) return { kind: "no-change" };
    const next: StreamerStatusOffline = {
      streamerId,
      isLive: false,
      lastEndedAt: now,
      lastStartedAt: previous.startedAt,
      lastMaxViewerCount: previous.maxViewerCount,
    };
    return { kind: "went-offline", previousLive: previous, next };
  }

  // At least one binding is live.
  const summedViewerCount = lives.reduce(
    (acc, l) => acc + (l.status.viewerCount ?? 0),
    0,
  );
  const pickByPriority = (): PlatformBinding => {
    const sorted = [...lives].sort((a, b) =>
      comparePlatformPriority(a.binding.platform, b.binding.platform),
    );
    return sorted[0].binding;
  };

  let primary: PlatformBinding;
  let primarySwitched = false;
  if (!previous.isLive) {
    primary = pickByPriority();
  } else {
    // Sticky primary unless the previous primary fell offline.
    const previousPrimaryStillLive = lives.some(
      (l) =>
        l.binding.platform === previous.primary.platform &&
        l.binding.username === previous.primary.username,
    );
    if (previousPrimaryStillLive) {
      primary = previous.primary;
    } else {
      primary = pickByPriority();
      primarySwitched = true;
    }
  }

  const primaryLive = lives.find(
    (l) =>
      l.binding.platform === primary.platform &&
      l.binding.username === primary.username,
  );
  if (!primaryLive) throw new Error("unreachable: primary must be in lives");
  const primaryTitle = primaryLive.status.title;

  if (!previous.isLive) {
    const next: StreamerStatusLive = {
      streamerId,
      isLive: true,
      primary,
      primaryTitle,
      startedAt: now,
      maxViewerCount: summedViewerCount,
    };
    return { kind: "went-live", next, summedViewerCount };
  }

  const next: StreamerStatusLive = {
    streamerId,
    isLive: true,
    primary,
    primaryTitle,
    startedAt: previous.startedAt,
    maxViewerCount: Math.max(previous.maxViewerCount, summedViewerCount),
  };
  const titleChanged = !primarySwitched && primaryTitle !== previous.primaryTitle;
  return { kind: "still-live", next, summedViewerCount, titleChanged, primarySwitched };
}
