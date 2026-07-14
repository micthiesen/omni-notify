import { Link } from "../router";

const LINKS = [
  { to: "/", label: "Home" },
  { to: "/pets", label: "Pets" },
  { to: "/recommendations", label: "Recommendations" },
];

function isActive(path: string, to: string): boolean {
  if (to === "/") return path === "/";
  return path === to || path.startsWith(`${to}/`);
}

export function NavBar({ path }: { path: string }) {
  return (
    <nav className="nav-bar">
      <Link to="/" className="nav-brand">
        omni-notify
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
    </nav>
  );
}
