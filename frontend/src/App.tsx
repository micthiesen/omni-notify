import { lazy, Suspense, useEffect } from "react";
import type { ReactNode } from "react";
import { NavBar } from "./components/NavBar";
import { LiveDataProvider } from "./live";
import HomePage from "./pages/HomePage";
import RecommendationsPage from "./pages/RecommendationsPage";
import { usePath } from "./router";

// PetsPage pulls in recharts (~500kB minified); keep it out of the main chunk.
const PetsPage = lazy(() => import("./pages/PetsPage"));

function normalizePath(path: string): string {
  if (path.length > 1 && path.endsWith("/")) return path.slice(0, -1);
  return path;
}

const PAGE_TITLES: Record<string, string> = {
  "/pets": "Pets",
  "/recommendations": "Recommendations",
};

export default function App() {
  const path = normalizePath(usePath());

  useEffect(() => {
    const section = PAGE_TITLES[path];
    document.title = section ? `${section} · Omni Notify` : "Omni Notify";
  }, [path]);

  let page: ReactNode;
  switch (path) {
    case "/pets":
      page = (
        <Suspense fallback={<div className="loading">Loading…</div>}>
          <PetsPage />
        </Suspense>
      );
      break;
    case "/recommendations":
      page = <RecommendationsPage />;
      break;
    default:
      page = <HomePage />;
      break;
  }

  return (
    <LiveDataProvider>
      <NavBar path={path} />
      <main className="page">{page}</main>
    </LiveDataProvider>
  );
}
