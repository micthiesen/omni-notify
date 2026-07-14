import type { ReactNode } from "react";
import { NavBar } from "./components/NavBar";
import HomePage from "./pages/HomePage";
import PetsPage from "./pages/PetsPage";
import RecommendationsPage from "./pages/RecommendationsPage";
import { usePath } from "./router";

function normalizePath(path: string): string {
  if (path.length > 1 && path.endsWith("/")) return path.slice(0, -1);
  return path;
}

export default function App() {
  const path = normalizePath(usePath());

  let page: ReactNode;
  switch (path) {
    case "/pets":
      page = <PetsPage />;
      break;
    case "/recommendations":
      page = <RecommendationsPage />;
      break;
    default:
      page = <HomePage />;
      break;
  }

  return (
    <>
      <NavBar path={path} />
      {page}
    </>
  );
}
