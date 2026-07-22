import type { Article } from "../types.js";

const FXTWITTER_THREAD_API = "https://api.fxtwitter.com/2/thread";
const REQUEST_TIMEOUT_MS = 20_000;

interface XAuthor {
  id?: string;
  name?: string;
  screen_name?: string;
}

interface XMediaItem {
  url?: string;
  altText?: string;
  alt_text?: string;
}

interface XMedia {
  all?: XMediaItem[];
  photos?: XMediaItem[];
}

interface XArticleBlock {
  text?: string;
  type?: string;
}

interface XArticleData {
  title?: string;
  created_at?: string;
  cover_media?: {
    media_info?: { original_img_url?: string };
  };
  content?: { blocks?: XArticleBlock[] };
}

interface XStatus {
  id?: string;
  url?: string;
  text?: string;
  created_at?: string;
  created_timestamp?: number;
  author?: XAuthor;
  media?: XMedia;
  article?: XArticleData;
  type?: string;
}

interface FxTwitterThreadResponse {
  code?: number;
  status?: XStatus;
  thread?: XStatus[];
  author?: XAuthor;
  message?: string;
  error?: string;
}

export interface XStatusUrl {
  id: string;
  screenName: string;
  canonicalUrl: string;
}

/** Accept only a normal X/Twitter status permalink, while allowing tracking query params. */
export function parseXStatusUrl(value: string): XStatusUrl {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new Error(`Invalid X status URL: ${value}`);
  }

  const hostname = url.hostname.toLowerCase();
  if (
    ![
      "x.com",
      "www.x.com",
      "mobile.x.com",
      "twitter.com",
      "www.twitter.com",
      "mobile.twitter.com",
    ].includes(hostname)
  ) {
    throw new Error(`Unsupported X URL hostname: ${url.hostname}`);
  }

  const namedMatch = /^\/([^/]+)\/status\/(\d+)(?:\/.*)?$/.exec(url.pathname);
  const webMatch = /^\/i\/web\/status\/(\d+)(?:\/.*)?$/.exec(url.pathname);
  if (!namedMatch && !webMatch) {
    throw new Error(`X URL is not a status permalink: ${value}`);
  }

  const screenName = namedMatch?.[1] ?? "i";
  const id = namedMatch?.[2] ?? webMatch![1];
  return {
    id,
    screenName,
    canonicalUrl: `https://x.com/${screenName}/status/${id}`,
  };
}

function isFullStatus(value: unknown): value is XStatus {
  if (!value || typeof value !== "object") return false;
  const status = value as XStatus;
  return (
    typeof status.id === "string" &&
    typeof status.text === "string" &&
    typeof status.author?.id === "string"
  );
}

function parsePublishedAt(status: XStatus, article?: XArticleData): Date | undefined {
  const value = article?.created_at ?? status.created_at;
  if (value) {
    const date = new Date(value);
    if (!Number.isNaN(date.getTime())) return date;
  }
  if (typeof status.created_timestamp === "number") {
    const date = new Date(status.created_timestamp * 1000);
    if (!Number.isNaN(date.getTime())) return date;
  }
  return undefined;
}

function firstPhotoUrl(status: XStatus): string | undefined {
  return status.media?.photos?.find((photo) => photo.url)?.url;
}

function statusMedia(status: XStatus): XMediaItem[] {
  const media = [...(status.media?.all ?? []), ...(status.media?.photos ?? [])];
  const seen = new Set<string>();
  return media.filter((item) => {
    const key = item.url ?? `${item.altText ?? ""}\0${item.alt_text ?? ""}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

function statusText(status: XStatus): string {
  let text = status.text?.trim() ?? "";
  for (const item of statusMedia(status)) {
    if (item.url) text = text.replaceAll(item.url, "");
  }
  // Media CDN links are presentation details, not authored thread prose.
  text = text.replace(/https?:\/\/(?:pbs\.twimg\.com|video\.twimg\.com)\/\S+/gi, "");
  text = text
    .split("\n")
    .map((line) => line.trimEnd())
    .join("\n")
    .trim();

  const descriptions = statusMedia(status)
    .map((item) => item.altText ?? item.alt_text)
    .filter((description): description is string => Boolean(description?.trim()))
    .map((description) => `Image description: ${description.trim()}`);
  return [text, ...descriptions].filter(Boolean).join("\n\n");
}

export function articleBlocksToMarkdown(blocks: XArticleBlock[]): string {
  return blocks
    .map((block) => {
      const text = block.text?.trim();
      if (!text) return undefined;
      switch (block.type) {
        case "atomic":
        case "media":
          return `Image description: ${text}`;
        case "divider":
          return undefined;
        case "header-one":
        case "header-two":
          return `## ${text}`;
        case "header-three":
          return `### ${text}`;
        default:
          return text;
      }
    })
    .filter((block): block is string => Boolean(block))
    .join("\n\n");
}

export function threadTitle(text: string, maxLength = 120): string | undefined {
  const firstLine = text
    .split("\n")
    .map((line) => line.trim())
    .find(Boolean);
  if (!firstLine) return undefined;
  if (firstLine.length <= maxLength) return firstLine;

  const available = firstLine.slice(0, maxLength - 1);
  const lastSpace = available.lastIndexOf(" ");
  const cutAt = lastSpace > maxLength * 0.6 ? lastSpace : available.length;
  return `${available.slice(0, cutAt).trimEnd()}…`;
}

export function parseFxTwitterResponse(
  payload: FxTwitterThreadResponse,
  requested: XStatusUrl,
): Article {
  if (payload.code !== 200) {
    const detail = payload.message ?? payload.error ?? "unknown API error";
    throw new Error(
      `FxTwitter API returned code ${payload.code ?? "missing"}: ${detail}`,
    );
  }

  const responseStatuses = [payload.status, ...(payload.thread ?? [])];
  const root = responseStatuses.find(
    (status): status is XStatus => isFullStatus(status) && status.id === requested.id,
  );
  if (!root)
    throw new Error(`FxTwitter response did not contain status ${requested.id}`);

  const canonicalUrl = `https://x.com/${root.author?.screen_name ?? requested.screenName}/status/${requested.id}`;
  const article = root.article;
  if (article?.content?.blocks && article.title?.trim()) {
    const text = articleBlocksToMarkdown(article.content.blocks);
    if (text) {
      return {
        title: article.title.trim(),
        text,
        author: root.author?.name ?? payload.author?.name,
        domain: "x.com",
        url: canonicalUrl,
        publishedAt: parsePublishedAt(root, article),
        leadImageUrl:
          article.cover_media?.media_info?.original_img_url ?? firstPhotoUrl(root),
      };
    }
  }

  const source = payload.thread?.length ? payload.thread : [root];
  const seen = new Set<string>();
  const statuses = source.filter((status): status is XStatus => {
    if (
      !isFullStatus(status) ||
      status.author?.id !== root.author?.id ||
      seen.has(status.id!)
    ) {
      return false;
    }
    seen.add(status.id!);
    return true;
  });
  if (!seen.has(root.id!)) statuses.unshift(root);

  const text = statuses.map(statusText).filter(Boolean).join("\n\n");
  if (!text) throw new Error(`X thread ${requested.id} has no readable text`);
  return {
    title: article?.title?.trim() || threadTitle(statusText(root)),
    text,
    author: root.author?.name ?? payload.author?.name,
    domain: "x.com",
    url: canonicalUrl,
    publishedAt: parsePublishedAt(root, article),
    leadImageUrl:
      article?.cover_media?.media_info?.original_img_url ?? firstPhotoUrl(root),
  };
}

export async function retrieveArticleX(
  url: string,
  userAgent: string,
): Promise<Article> {
  const requested = parseXStatusUrl(url);
  const response = await fetch(`${FXTWITTER_THREAD_API}/${requested.id}`, {
    headers: { "User-Agent": userAgent, Accept: "application/json" },
    signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
  });
  if (!response.ok) {
    throw new Error(`FxTwitter request failed with HTTP ${response.status}`);
  }

  let payload: FxTwitterThreadResponse;
  try {
    payload = (await response.json()) as FxTwitterThreadResponse;
  } catch (error) {
    throw new Error("FxTwitter returned invalid JSON", { cause: error });
  }
  return parseFxTwitterResponse(payload, requested);
}
