import { useEffect, useRef, useState, type ReactNode } from "react";
import { fetchWorkspaces, type WorkspaceSummary } from "../api";
import { useLiveData } from "../live";
import { Link } from "../router";

type IconName =
  | "home"
  | "watch"
  | "listen"
  | "research"
  | "more"
  | "operations"
  | "email"
  | "pets"
  | "costs"
  | "data";

interface NavItem {
  to: string;
  label: string;
  icon: IconName;
  paths?: string[];
}

const PRIMARY_LINKS: NavItem[] = [
  { to: "/", label: "Home", icon: "home" },
  { to: "/media", label: "Watch", icon: "watch" },
  {
    to: "/podcasts",
    label: "Listen",
    icon: "listen",
    paths: ["/podcasts", "/pods"],
  },
  {
    to: "/workspaces",
    label: "Research",
    icon: "research",
    paths: ["/workspaces", "/briefings"],
  },
];

const MORE_LINKS: NavItem[] = [
  { to: "/operations", label: "Operations", icon: "operations" },
  { to: "/emails", label: "Email", icon: "email" },
  { to: "/pets", label: "Pets", icon: "pets" },
  { to: "/costs", label: "Costs", icon: "costs" },
  { to: "/data", label: "Data", icon: "data" },
];

function isPathActive(path: string, item: NavItem): boolean {
  const candidates = item.paths ?? [item.to];
  return candidates.some((candidate) =>
    candidate === "/"
      ? path === "/"
      : path === candidate || path.startsWith(`${candidate}/`),
  );
}

function NavIcon({ name }: { name: IconName }) {
  const paths: Record<IconName, ReactNode> = {
    home: (
      <>
        <path d="m3 11 9-8 9 8" />
        <path d="M5 10v10h14V10" />
        <path d="M9 20v-6h6v6" />
      </>
    ),
    watch: (
      <>
        <rect x="3" y="5" width="18" height="14" rx="2" />
        <path d="m10 9 5 3-5 3Z" />
      </>
    ),
    listen: (
      <>
        <path d="M4 13a8 8 0 0 1 16 0" />
        <path d="M4 13v5a2 2 0 0 0 2 2h2v-8H4" />
        <path d="M20 13v5a2 2 0 0 1-2 2h-2v-8h4" />
      </>
    ),
    research: (
      <>
        <path d="M4 5.5A2.5 2.5 0 0 1 6.5 3H11v16H6.5A2.5 2.5 0 0 0 4 21.5Z" />
        <path d="M20 5.5A2.5 2.5 0 0 0 17.5 3H13v16h4.5a2.5 2.5 0 0 1 2.5 2.5Z" />
      </>
    ),
    more: (
      <>
        <circle cx="5" cy="12" r="1" fill="currentColor" />
        <circle cx="12" cy="12" r="1" fill="currentColor" />
        <circle cx="19" cy="12" r="1" fill="currentColor" />
      </>
    ),
    operations: (
      <>
        <path d="M4 6h16M4 12h16M4 18h16" />
        <circle cx="8" cy="6" r="2" fill="var(--bg-card)" />
        <circle cx="16" cy="12" r="2" fill="var(--bg-card)" />
        <circle cx="10" cy="18" r="2" fill="var(--bg-card)" />
      </>
    ),
    email: (
      <>
        <rect x="3" y="5" width="18" height="14" rx="2" />
        <path d="m4 7 8 6 8-6" />
      </>
    ),
    pets: (
      <>
        <circle cx="8" cy="8" r="2" />
        <circle cx="16" cy="8" r="2" />
        <circle cx="5" cy="13" r="2" />
        <circle cx="19" cy="13" r="2" />
        <path d="M8 18c0-3 2-5 4-5s4 2 4 5c0 2-2 3-4 3s-4-1-4-3Z" />
      </>
    ),
    costs: (
      <>
        <circle cx="12" cy="12" r="9" />
        <path d="M15 8.5c-.8-.6-1.8-1-3-1-1.7 0-3 1-3 2.3 0 3.7 6 1.4 6 5.2 0 1.4-1.3 2.5-3.2 2.5-1.2 0-2.4-.4-3.3-1.2M12 5v14" />
      </>
    ),
    data: (
      <>
        <ellipse cx="12" cy="5" rx="8" ry="3" />
        <path d="M4 5v7c0 1.7 3.6 3 8 3s8-1.3 8-3V5" />
        <path d="M4 12v7c0 1.7 3.6 3 8 3s8-1.3 8-3v-7" />
      </>
    ),
  };
  return (
    <svg
      className="nav-icon"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.7"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {paths[name]}
    </svg>
  );
}

const CONNECTION_LABELS = {
  connecting: "Connecting",
  live: "Live",
  polling: "Reconnecting",
} as const;

const CONNECTION_TITLES = {
  connecting: "Establishing realtime connection…",
  live: "Realtime updates connected",
  polling: "Realtime stream down, polling every 10s",
} as const;

function ConnectionBadge() {
  const { connection } = useLiveData();
  return (
    <button
      type="button"
      className={`conn-badge conn-${connection}`}
      title={`${CONNECTION_TITLES[connection]} (click to refresh)`}
      onClick={() => window.location.reload()}
    >
      <span className="conn-dot" />
      {CONNECTION_LABELS[connection]}
    </button>
  );
}

function Brand() {
  return (
    <Link to="/" className="nav-brand">
      <svg
        width="19"
        height="19"
        viewBox="0 0 24 24"
        fill="currentColor"
        aria-hidden="true"
      >
        <path d="M12 2a7 7 0 0 0-7 7v3.3l-1.7 2.7A1.5 1.5 0 0 0 4.6 17.3h14.8a1.5 1.5 0 0 0 1.3-2.3L19 12.3V9a7 7 0 0 0-7-7Zm-2.5 16.3a2.5 2.5 0 0 0 5 0Z" />
      </svg>
      <span>Omni Notify</span>
    </Link>
  );
}

function ActionBadge({ value, title }: { value: number | null; title?: string }) {
  if (value === 0) return null;
  return (
    <span
      className={`nav-action-badge ${value === null ? "nav-action-unknown" : ""}`}
      title={title}
    >
      {value === null ? "!" : value > 99 ? "99+" : value}
    </span>
  );
}

export function NavBar({ path }: { path: string }) {
  const { snapshot } = useLiveData();
  const [moreOpen, setMoreOpen] = useState(false);
  const [workspaces, setWorkspaces] = useState<WorkspaceSummary[] | null>(null);
  const [workspaceLoadFailed, setWorkspaceLoadFailed] = useState(false);
  const moreSheetRef = useRef<HTMLDivElement>(null);
  const moreButtonRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    setMoreOpen(false);
  }, [path]);

  useEffect(() => {
    let cancelled = false;
    const refresh = () => {
      fetchWorkspaces()
        .then(({ workspaces: next }) => {
          if (!cancelled) {
            setWorkspaces(next);
            setWorkspaceLoadFailed(false);
          }
        })
        .catch(() => {
          if (!cancelled) setWorkspaceLoadFailed(true);
        });
    };
    refresh();
    window.addEventListener("workspace-updated", refresh);
    return () => {
      cancelled = true;
      window.removeEventListener("workspace-updated", refresh);
    };
  }, []);

  useEffect(() => {
    if (!moreOpen) return;
    const previouslyFocused = document.activeElement as HTMLElement | null;
    const sheet = moreSheetRef.current;
    const closeButton = sheet?.querySelector<HTMLButtonElement>(".mobile-more-close");
    closeButton?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        setMoreOpen(false);
        return;
      }
      if (event.key !== "Tab" || !sheet) return;
      const focusable = Array.from(
        sheet.querySelectorAll<HTMLElement>("a[href], button:not([disabled])"),
      );
      if (focusable.length === 0) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      previouslyFocused?.focus();
    };
  }, [moreOpen]);

  const researchActions = workspaces
    ? workspaces.reduce((total, workspace) => total + workspace.pendingActionCount, 0)
    : workspaceLoadFailed
      ? null
      : 0;
  const failingTasks =
    snapshot?.tasks.filter((task) => task.lastRun?.status === "error").length ?? 0;
  const moreActive = MORE_LINKS.some((item) => isPathActive(path, item));

  const link = (item: NavItem, mobile = false) => (
    <Link
      key={item.to}
      to={item.to}
      className={`${mobile ? "mobile-nav-link" : "sidebar-link"} ${isPathActive(path, item) ? "active" : ""}`}
      ariaCurrent={isPathActive(path, item) ? "page" : undefined}
    >
      <NavIcon name={item.icon} />
      <span>{item.label}</span>
      {item.icon === "research" && (
        <ActionBadge
          value={researchActions}
          title={researchActions === null ? "Research status unavailable" : undefined}
        />
      )}
      {item.icon === "operations" && <ActionBadge value={failingTasks} />}
    </Link>
  );

  return (
    <>
      <aside className="app-sidebar">
        <div className="sidebar-header">
          <Brand />
        </div>
        <nav className="sidebar-nav" aria-label="Primary navigation">
          <div className="sidebar-group">{PRIMARY_LINKS.map((item) => link(item))}</div>
          <div className="sidebar-group">
            <div className="sidebar-group-label">More</div>
            {MORE_LINKS.map((item) => link(item))}
          </div>
        </nav>
        <div className="sidebar-footer">
          <ConnectionBadge />
        </div>
      </aside>

      <header className="mobile-header">
        <Brand />
        <ConnectionBadge />
      </header>

      <nav className="mobile-bottom-nav" aria-label="Primary navigation">
        {PRIMARY_LINKS.map((item) => link(item, true))}
        <button
          ref={moreButtonRef}
          type="button"
          className={`mobile-nav-link ${moreOpen || moreActive ? "active" : ""}`}
          onClick={() => setMoreOpen((open) => !open)}
          aria-expanded={moreOpen}
          aria-controls="mobile-more-menu"
        >
          <NavIcon name="more" />
          <span>More</span>
          <ActionBadge value={failingTasks} />
        </button>
      </nav>

      {moreOpen && (
        <div className="mobile-more-backdrop" onClick={() => setMoreOpen(false)}>
          <div
            ref={moreSheetRef}
            id="mobile-more-menu"
            className="mobile-more-sheet"
            role="dialog"
            aria-modal="true"
            aria-label="More navigation"
            onClick={(event) => event.stopPropagation()}
          >
            <div className="mobile-more-handle" />
            <div className="mobile-more-title">
              <h2>More</h2>
              <button
                type="button"
                className="mobile-more-close"
                onClick={() => setMoreOpen(false)}
              >
                Close
              </button>
            </div>
            <div className="mobile-more-links">
              {MORE_LINKS.map((item) => link(item))}
            </div>
          </div>
        </div>
      )}
    </>
  );
}
