export function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

export function formatClockTime(hour: number, minute: number): string {
  const ampm = hour >= 12 ? "PM" : "AM";
  const h12 = hour % 12 === 0 ? 12 : hour % 12;
  return `${h12}:${pad2(minute)} ${ampm}`;
}

export function formatRelative(epochMs: number, now = Date.now()): string {
  const diff = epochMs - now;
  const future = diff >= 0;
  const absMs = Math.abs(diff);

  if (absMs < 5_000) return future ? "in a moment" : "just now";

  const totalSec = Math.round(absMs / 1000);
  let text: string;
  if (totalSec < 60) {
    text = `${totalSec}s`;
  } else if (totalSec < 3600) {
    text = `${Math.round(totalSec / 60)}m`;
  } else if (totalSec < 86_400) {
    const hours = Math.floor(totalSec / 3600);
    const mins = Math.round((totalSec % 3600) / 60);
    text = mins > 0 ? `${hours}h ${mins}m` : `${hours}h`;
  } else {
    text = `${Math.round(totalSec / 86_400)}d`;
  }

  return future ? `in ${text}` : `${text} ago`;
}

export function formatDuration(ms: number): string {
  if (ms < 1000) return `${Math.max(0, Math.round(ms))}ms`;
  const totalSec = ms / 1000;
  if (totalSec < 60) return `${totalSec.toFixed(1)}s`;
  const mins = Math.floor(totalSec / 60);
  const secs = Math.round(totalSec % 60);
  return `${mins}m ${secs}s`;
}

/** Ticking countdown: "2h 05m", "3m 12s", "42s", "now". */
export function formatCountdown(ms: number): string {
  if (ms <= 0) return "now";
  const totalSec = Math.floor(ms / 1000);
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  const s = totalSec % 60;
  if (h > 0) return `${h}h ${pad2(m)}m`;
  if (m > 0) return `${m}m ${pad2(s)}s`;
  return `${s}s`;
}

/** Coarse elapsed time for stream uptime: "1h 23m", "23m", "45s". */
export function formatUptime(ms: number): string {
  const totalSec = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(totalSec / 3600);
  const m = Math.floor((totalSec % 3600) / 60);
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m`;
  return `${totalSec}s`;
}

export function formatCompactNumber(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 10_000) return `${Math.round(n / 1000)}k`;
  if (n >= 1_000) return `${(n / 1000).toFixed(1)}k`;
  return String(n);
}

export function formatAbsolute(epochMs: number): string {
  return new Date(epochMs).toLocaleString("en-US", {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}

export function formatDateOnly(epochMs: number): string {
  return new Date(epochMs).toLocaleDateString("en-US", {
    month: "short",
    day: "numeric",
    year: "numeric",
  });
}
