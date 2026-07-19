import { lazy, Suspense, useEffect } from "react";
import type { ReactNode } from "react";
import { NavBar } from "./components/NavBar";
import { LiveDataProvider } from "./live";
import BriefingsPage from "./pages/BriefingsPage";
import FeedbackPage, { type FeedbackKind } from "./pages/FeedbackPage";
import HomePage from "./pages/HomePage";
import PodcastDetailPage from "./pages/PodcastDetailPage";
import PodcastsPage from "./pages/PodcastsPage";
import RecommendationDetailPage from "./pages/RecommendationDetailPage";
import RecommendationsPage from "./pages/RecommendationsPage";
import { usePath } from "./router";

// These pages pull in recharts (~500kB minified); keep it out of the main chunk.
const PetsPage = lazy(() => import("./pages/PetsPage"));
const PodsPage = lazy(() => import("./pages/PodsPage"));
const StreamerPage = lazy(() => import("./pages/StreamerPage"));
const DataPage = lazy(() => import("./pages/DataPage"));
const EmailActivityPage = lazy(() => import("./pages/EmailActivityPage"));

function normalizePath(path: string): string {
  if (path.length > 1 && path.endsWith("/")) return path.slice(0, -1);
  // Legacy alias: old Pushover notifications and bookmarks link here.
  if (path === "/recommendations") return "/media";
  return path;
}

const PAGE_TITLES: Record<string, string> = {
  "/pets": "Pets",
  "/media": "Media",
  "/podcasts": "Podcasts",
  "/pods": "PressPods",
  "/briefings": "Briefings",
  "/emails": "Email activity",
  "/data": "Data",
};

export default function App() {
  const path = normalizePath(usePath());

  useEffect(() => {
    const section = PAGE_TITLES[path];
    document.title = section ? `${section} · Omni Notify` : "Omni Notify";
  }, [path]);

  let page: ReactNode;
  const feedbackMatch = path.match(
    /^\/feedback\/(recommendations|podcasts)\/([^/]+)$/,
  );
  const mediaDetailMatch = path.match(/^\/media\/([^/]+)$/);
  const podcastDetailMatch = path.match(/^\/podcasts\/([^/]+)$/);
  if (feedbackMatch) {
    const kind = feedbackMatch[1] as FeedbackKind;
    const id = decodeURIComponent(feedbackMatch[2]);
    page = <FeedbackPage key={`${kind}/${id}`} kind={kind} id={id} />;
  } else if (mediaDetailMatch) {
    const id = decodeURIComponent(mediaDetailMatch[1]);
    page = <RecommendationDetailPage key={id} id={id} />;
  } else if (podcastDetailMatch) {
    const id = decodeURIComponent(podcastDetailMatch[1]);
    page = <PodcastDetailPage key={id} id={id} />;
  } else if (path.startsWith("/streamers/")) {
    const streamerId = decodeURIComponent(path.slice("/streamers/".length));
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
      default:
        page = <HomePage />;
        break;
    }
  }

  return (
    <LiveDataProvider>
      <NavBar path={path} />
      <main className={`page ${path === "/data" ? "page-data" : ""}`}>{page}</main>
    </LiveDataProvider>
  );
}
