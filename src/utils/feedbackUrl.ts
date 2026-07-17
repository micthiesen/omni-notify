import config from "./config.js";

/** One-tap rating page; it deep-links onward to the full recommendation view. */
export function feedbackUrl(
  kind: "recommendations" | "podcasts",
  recommendationId: string,
): string {
  const base = config.RECS_PUBLIC_URL.replace(/\/$/, "");
  return `${base}/feedback/${kind}/${encodeURIComponent(recommendationId)}`;
}
