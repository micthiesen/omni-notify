import type {
  StreamerLiveBinding,
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
  | { kind: "partial-unknown-keep" }
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

  // Partial info: some unknown, none live → don't transition to offline
  // (we might still be live on the unknown bindings).
  if (!anyLive && anyUnknown) {
    return { kind: "partial-unknown-keep" };
  }

  if (!anyLive) {
    // All bindings confirmed offline.
    if (!previous.isLive) {
      // Already offline; keep as-is, no edge.
      return { kind: "partial-unknown-keep" };
    }
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
  const liveBindings: StreamerLiveBinding[] = lives.map((l) => ({
    platform: l.binding.platform,
    username: l.binding.username,
    title: l.status.title,
    viewerCount: l.status.viewerCount,
  }));

  const pickByPriority = (): PlatformBinding => {
    const sorted = [...lives].sort((a, b) =>
      comparePlatformPriority(a.binding.platform, b.binding.platform),
    );
    return sorted[0].binding;
  };

  if (!previous.isLive) {
    // Fresh go-live. Primary = highest-priority among currently-live bindings.
    const primary = pickByPriority();
    const primaryLive = lives.find(
      (l) =>
        l.binding.platform === primary.platform &&
        l.binding.username === primary.username,
    );
    if (!primaryLive) {
      throw new Error("unreachable: primary must be present in lives");
    }
    const next: StreamerStatusLive = {
      streamerId,
      isLive: true,
      primary,
      primaryTitle: primaryLive.status.title,
      startedAt: now,
      maxViewerCount: summedViewerCount,
      bindings: liveBindings,
    };
    return { kind: "went-live", next, summedViewerCount };
  }

  // Live → Live. Sticky primary unless previous primary fell offline.
  const previousPrimaryStillLive = lives.find(
    (l) =>
      l.binding.platform === previous.primary.platform &&
      l.binding.username === previous.primary.username,
  );
  let primary: PlatformBinding;
  let primarySwitched: boolean;
  if (previousPrimaryStillLive) {
    primary = previous.primary;
    primarySwitched = false;
  } else {
    primary = pickByPriority();
    primarySwitched = true;
  }
  const primaryLive = lives.find(
    (l) =>
      l.binding.platform === primary.platform &&
      l.binding.username === primary.username,
  );
  if (!primaryLive) {
    throw new Error("unreachable: primary must be present in lives");
  }
  const primaryTitle = primaryLive.status.title;

  const next: StreamerStatusLive = {
    streamerId,
    isLive: true,
    primary,
    primaryTitle,
    startedAt: previous.startedAt,
    maxViewerCount: Math.max(previous.maxViewerCount, summedViewerCount),
    bindings: liveBindings,
  };

  const titleChanged = !primarySwitched && primaryTitle !== previous.primaryTitle;
  return {
    kind: "still-live",
    next,
    summedViewerCount,
    titleChanged,
    primarySwitched,
  };
}
