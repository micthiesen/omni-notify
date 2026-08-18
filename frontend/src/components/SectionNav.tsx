import { Link } from "../router";

interface SectionLink {
  to: string;
  label: string;
}

function active(path: string, to: string): boolean {
  return path === to || path.startsWith(`${to}/`);
}

export function SectionNav({ path }: { path: string }) {
  let label: string | null = null;
  let links: SectionLink[] = [];

  if (path.startsWith("/podcasts") || path.startsWith("/pods")) {
    label = "Listen";
    links = [
      { to: "/podcasts", label: "Podcast Picks" },
      { to: "/pods", label: "PressPods" },
    ];
  } else if (path.startsWith("/workspaces") || path.startsWith("/briefings")) {
    label = "Research";
    links = [
      { to: "/workspaces", label: "Workspaces" },
      { to: "/briefings", label: "Briefings" },
    ];
  }

  if (!label) return null;
  return (
    <nav className="section-nav" aria-label={`${label} sections`}>
      <span className="section-nav-label">{label}</span>
      {links.map((item) => (
        <Link
          key={item.to}
          to={item.to}
          className={`section-nav-link ${active(path, item.to) ? "active" : ""}`}
          ariaCurrent={active(path, item.to) ? "page" : undefined}
        >
          {item.label}
        </Link>
      ))}
    </nav>
  );
}
