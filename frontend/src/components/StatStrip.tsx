import type { Snapshot } from "../api";
import { useNow } from "../hooks/useNow";
import { formatCountdown, taskLabel } from "../utils/format";

interface Tile {
  label: string;
  value: string;
  detail?: string;
  tone?: "accent" | "danger" | "live";
}

export function StatStrip({ snapshot }: { snapshot: Snapshot }) {
  const now = useNow(1000);

  const liveCount = snapshot.streamers.filter((s) => s.live).length;
  const running = snapshot.tasks.filter((t) => t.running).length;
  const failing = snapshot.tasks.filter(
    (t) => t.lastRun?.status === "error",
  ).length;

  const next = snapshot.tasks
    .flatMap((t) => {
      const iso = t.nextRuns[0];
      if (!iso) return [];
      const at = new Date(iso).getTime();
      return Number.isNaN(at) ? [] : [{ task: t, at }];
    })
    .sort((a, b) => a.at - b.at)[0];

  const tiles: Tile[] = [];
  if (snapshot.streamers.length > 0) {
    tiles.push({
      label: "Live Channels",
      value: String(liveCount),
      tone: liveCount > 0 ? "live" : undefined,
    });
  }
  tiles.push(
    {
      label: "Tasks Running",
      value: String(running),
      detail: `${snapshot.tasks.length} registered`,
      tone: running > 0 ? "accent" : undefined,
    },
    {
      label: "Tasks Failing",
      value: String(failing),
      tone: failing > 0 ? "danger" : undefined,
    },
    next
      ? {
          label: "Next Run",
          value: formatCountdown(next.at - now),
          detail: taskLabel(next.task),
        }
      : { label: "Next Run", value: "—" },
  );

  return (
    <div className="stat-strip">
      {tiles.map((tile) => (
        <div key={tile.label} className={`stat-tile ${tile.tone ?? ""}`}>
          <span className="stat-label">{tile.label}</span>
          <span className="stat-value">{tile.value}</span>
          {tile.detail && <span className="stat-detail">{tile.detail}</span>}
        </div>
      ))}
    </div>
  );
}
