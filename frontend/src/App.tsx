import { lazy, Suspense, useEffect, useRef } from "react";
import type { ReactNode } from "react";
import { NavBar } from "./components/NavBar";
import { SectionNav } from "./components/SectionNav";
import { LiveDataProvider } from "./live";
import BriefingsPage from "./pages/BriefingsPage";
import FeedbackPage, { type FeedbackKind } from "./pages/FeedbackPage";
import HomePage from "./pages/HomePage";
import PodcastDetailPage from "./pages/PodcastDetailPage";
import PodcastsPage from "./pages/PodcastsPage";
import RecommendationDetailPage from "./pages/RecommendationDetailPage";
import RecommendationsPage from "./pages/RecommendationsPage";
import { Link, usePath } from "./router";

// These pages pull in recharts (~500kB minified); keep it out of the main chunk.
const PetsPage = lazy(() => import("./pages/PetsPage"));
const PodsPage = lazy(() => import("./pages/PodsPage"));
const PodsDetailPage = lazy(() => import("./pages/PodsDetailPage"));
const StreamerPage = lazy(() => import("./pages/StreamerPage"));
const LivestreamIntelligencePage = lazy(
  () => import("./pages/LivestreamIntelligencePage"),
);
const DataPage = lazy(() => import("./pages/DataPage"));
const EmailActivityPage = lazy(() => import("./pages/EmailActivityPage"));
const CostsPage = lazy(() => import("./pages/CostsPage"));
const WorkspacesPage = lazy(() => import("./pages/WorkspacesPage"));
const OperationsPage = lazy(() => import("./pages/OperationsPage"));

function normalizePath(path: string): string {
  if (path.length > 1 && path.endsWith("/")) path = path.slice(0, -1);
  // Legacy alias: old Pushover notifications and bookmarks link here.
  if (path === "/recommendations") return "/media";
  return path;
}

const PAGE_TITLES: Record<string, string> = {
  "/pets": "Pets",
  "/media": "Watch",
  "/podcasts": "Podcasts",
  "/pods": "PressPods",
  "/briefings": "Briefings",
  "/emails": "Email Activity",
  "/data": "Data",
  "/costs": "Costs",
  "/workspaces": "Workspaces",
  "/operations": "Operations",
};

export default function App() {
  const path = normalizePath(usePath());
  const previousPath = useRef(path);

  useEffect(() => {
    if (previousPath.current === path) return;
    previousPath.current = path;
    document.getElementById("main-content")?.focus({ preventScroll: true });
  }, [path]);

  useEffect(() => {
    const section = path.startsWith("/workspaces") ? "Workspaces" : PAGE_TITLES[path];
    document.title = section ? `${section} · Omni Notify` : "Omni Notify";
  }, [path]);

  let page: ReactNode;
  let malformedPath = false;
  try {
    decodeURIComponent(path);
  } catch {
    malformedPath = true;
  }
  const notFound = (
    <div className="not-found-page">
      <span className="home-eyebrow">404</span>
      <h1>Page Not Found</h1>
      <p className="page-subtitle">This link does not lead to a page in Omni Notify.</p>
      <Link to="/" className="section-view-all">
        Back to Home ›
      </Link>
    </div>
  );
  const feedbackMatch = path.match(/^\/feedback\/(recommendations|podcasts)\/([^/]+)$/);
  const mediaDetailMatch = path.match(/^\/media\/([^/]+)$/);
  const podcastDetailMatch = path.match(/^\/podcasts\/([^/]+)$/);
  const podsDetailMatch = path.match(/^\/pods\/([^/]+)$/);
  const workspaceSubjectMatch = path.match(/^\/workspaces\/([^/]+)\/([^/]+)$/);
  const workspaceMatch = path.match(/^\/workspaces\/([^/]+)$/);
  const streamerMatch = path.match(/^\/streamers\/([^/]+)$/);
  const streamerIntelligenceMatch = path.match(/^\/streamers\/([^/]+)\/intelligence$/);
  if (malformedPath) {
    page = notFound;
  } else if (workspaceSubjectMatch) {
    page = (
      <Suspense fallback={<div className="loading">Loading…</div>}>
        <WorkspacesPage
          workspaceId={decodeURIComponent(workspaceSubjectMatch[1])}
          subjectId={decodeURIComponent(workspaceSubjectMatch[2])}
        />
      </Suspense>
    );
  } else if (workspaceMatch) {
    page = (
      <Suspense fallback={<div className="loading">Loading…</div>}>
        <WorkspacesPage workspaceId={decodeURIComponent(workspaceMatch[1])} />
      </Suspense>
    );
  } else if (feedbackMatch) {
    const kind = feedbackMatch[1] as FeedbackKind;
    const id = decodeURIComponent(feedbackMatch[2]);
    page = <FeedbackPage key={`${kind}/${id}`} kind={kind} id={id} />;
  } else if (mediaDetailMatch) {
    const id = decodeURIComponent(mediaDetailMatch[1]);
    page = <RecommendationDetailPage key={id} id={id} />;
  } else if (podcastDetailMatch) {
    const id = decodeURIComponent(podcastDetailMatch[1]);
    page = <PodcastDetailPage key={id} id={id} />;
  } else if (podsDetailMatch) {
    const id = decodeURIComponent(podsDetailMatch[1]);
    page = (
      <Suspense fallback={<div className="loading">Loading…</div>}>
        <PodsDetailPage key={id} id={id} />
      </Suspense>
    );
  } else if (streamerIntelligenceMatch) {
    const streamerId = decodeURIComponent(streamerIntelligenceMatch[1]);
    page = (
      <Suspense fallback={<div className="loading">Loading…</div>}>
        <LivestreamIntelligencePage key={streamerId} streamerId={streamerId} />
      </Suspense>
    );
  } else if (streamerMatch) {
    const streamerId = decodeURIComponent(streamerMatch[1]);
    page = (
      <Suspense fallback={<div className="loading">Loading…</div>}>
        <StreamerPage key={streamerId} streamerId={streamerId} />
      </Suspense>
    );
  } else {
    switch (path) {
      case "/pets":
        page = (
          <Suspense fallback={<div className="loading">Loading…</div>}>
            <PetsPage />
          </Suspense>
        );
        break;
      case "/media":
        page = <RecommendationsPage />;
        break;
      case "/data":
        page = (
          <Suspense fallback={<div className="loading">Loading…</div>}>
            <DataPage />
          </Suspense>
        );
        break;
      case "/podcasts":
        page = <PodcastsPage />;
        break;
      case "/pods":
        page = (
          <Suspense fallback={<div className="loading">Loading…</div>}>
            <PodsPage />
          </Suspense>
        );
        break;
      case "/briefings":
        page = <BriefingsPage />;
        break;
      case "/emails":
        page = (
          <Suspense fallback={<div className="loading">Loading…</div>}>
            <EmailActivityPage />
          </Suspense>
        );
        break;
      case "/costs":
        page = (
          <Suspense fallback={<div className="loading">Loading…</div>}>
            <CostsPage />
          </Suspense>
        );
        break;
      case "/workspaces":
        page = (
          <Suspense fallback={<div className="loading">Loading…</div>}>
            <WorkspacesPage />
          </Suspense>
        );
        break;
      case "/operations":
        page = (
          <Suspense fallback={<div className="loading">Loading…</div>}>
            <OperationsPage />
          </Suspense>
        );
        break;
      case "/":
        page = <HomePage />;
        break;
      default:
        page = notFound;
        break;
    }
  }

  return (
    <LiveDataProvider>
      <div className="app-shell">
        <a href="#main-content" className="skip-link">
          Skip to Content
        </a>
        <NavBar path={path} />
        <main
          id="main-content"
          tabIndex={-1}
          className={`page ${path === "/data" ? "page-data" : ""}`}
        >
          <SectionNav path={path} />
          {page}
        </main>
      </div>
    </LiveDataProvider>
  );
}
