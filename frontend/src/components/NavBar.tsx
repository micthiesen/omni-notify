import { useLiveData } from "../live";
import { Link } from "../router";

const LINKS = [
  { to: "/", label: "Dashboard" },
  { to: "/media", label: "Media" },
  { to: "/podcasts", label: "Podcasts" },
  { to: "/pods", label: "PressPods" },
  { to: "/briefings", label: "Briefings" },
  { to: "/emails", label: "Email" },
  { to: "/pets", label: "Pets" },
  { to: "/data", label: "Data" },
];

function isActive(path: string, to: string): boolean {
  if (to === "/") return path === "/";
  return path === to || path.startsWith(`${to}/`);
}

const CONNECTION_LABELS = {
  connecting: "Connecting",
  live: "Live",
  polling: "Reconnecting",
} as const;

const CONNECTION_TITLES = {
  connecting: "Establishing realtime connection…",
  live: "Realtime updates connected",
  polling: "Realtime stream down — polling every 10s",
} as const;

function ConnectionBadge() {
  const { connection } = useLiveData();
  return (
    <span
      className={`conn-badge conn-${connection}`}
      title={CONNECTION_TITLES[connection]}
    >
      <span className="conn-dot" />
      {CONNECTION_LABELS[connection]}
    </span>
  );
}

function BellIcon() {
  return (
    <svg
      width="18"
      height="18"
      viewBox="0 0 24 24"
      fill="currentColor"
      aria-hidden="true"
    >
      <path d="M12 2a7 7 0 0 0-7 7v3.3l-1.7 2.7A1.5 1.5 0 0 0 4.6 17.3h14.8a1.5 1.5 0 0 0 1.3-2.3L19 12.3V9a7 7 0 0 0-7-7Zm-2.5 16.3a2.5 2.5 0 0 0 5 0Z" />
    </svg>
  );
}

export function NavBar({ path }: { path: string }) {
  return (
    <nav className="nav-bar">
      <div className="nav-inner">
        <Link to="/" className="nav-brand">
          <BellIcon />
          <span>Omni Notify</span>
        </Link>
        <div className="nav-links">
          {LINKS.map((link) => (
            <Link
              key={link.to}
              to={link.to}
              className={`nav-link ${isActive(path, link.to) ? "active" : ""}`}
            >
              {link.label}
            </Link>
          ))}
        </div>
        <ConnectionBadge />
      </div>
    </nav>
  );
}
