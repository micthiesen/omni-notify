import { extractDomain } from "@micthiesen/mitools/strings";
import { Effect, Schema } from "effect";
import { PressPodsError } from "../effect.js";
import { currentCostFeature, recordCostEventSafely } from "../../costs/persistence.js";
import config from "../../utils/config.js";
import { cleanText } from "../formatting/index.js";
import { fetchPublicJson } from "../publicHttp.js";
import { extractTitleFromHtml } from "./constants.js";

const JINA_API_BASE = "https://r.jina.ai";
/** Standard prepaid Jina API rate: US $0.05 per million tokens. */
export const JINA_READER_CENTS_PER_TOKEN = 5 / 1_000_000;

const JinaReaderResponseSchema = Schema.Struct({
  data: Schema.Struct({
    title: Schema.optional(Schema.String),
    content: Schema.String,
    usage: Schema.optional(Schema.Struct({ tokens: Schema.optional(Schema.Number) })),
  }),
});

/**
 * Retrieve article using Jina.ai Reader API.
 * Uses a headless browser to render JS-heavy pages.
 */
export function retrieveArticleJina(url: string, _userAgent: string) {
  const jinaUrl = `${JINA_API_BASE}/${url}`;
  return fetchPublicJson(
    jinaUrl,
    {
      headers: {
        Accept: "application/json",
        Authorization: `Bearer ${config.JINA_API_KEY}`,
        "X-Respond-With": "html",
        "X-Target-Selector":
          "article, main, [role=main], .article-body, .post-content, .entry-content",
        "X-Remove-Selector":
          "nav, footer, header, aside, .sidebar, .comments, .related, .social-share, .advertisement, [role=navigation], [role=banner], [role=contentinfo]",
      },
      timeout: { request: 30000 },
      retry: { limit: 2, methods: ["GET"] },
    },
    "retrieve article with Jina",
  ).pipe(
    Effect.flatMap((raw) =>
      Schema.decodeUnknownEffect(JinaReaderResponseSchema)(raw).pipe(
        Effect.mapError(
          (cause) =>
            new PressPodsError({ operation: "decode Jina Reader response", cause }),
        ),
      ),
    ),
    Effect.flatMap((response) =>
      Effect.gen(function* () {
        const html = response.data.content;
        if (!html || html.length < 100) {
          return yield* Effect.fail(
            new PressPodsError({
              operation: "retrieve article with Jina",
              cause: new Error("Jina returned empty or too short content"),
            }),
          );
        }
        const reportedTokens = response.data.usage?.tokens;
        const tokens =
          typeof reportedTokens === "number" &&
          Number.isFinite(reportedTokens) &&
          reportedTokens >= 0
            ? reportedTokens
            : null;
        yield* recordCostEventSafely({
          category: "retrieval",
          feature: currentCostFeature("press-pods"),
          operation: "retrieve-article",
          service: "jina",
          model: "reader",
          costCents: tokens === null ? null : tokens * JINA_READER_CENTS_PER_TOKEN,
          priceStatus: tokens === null ? "unknown" : "estimated",
          usage: { requests: 1, ...(tokens === null ? {} : { outputTokens: tokens }) },
        });
        return {
          title: response.data.title?.trim() || extractTitleFromHtml(html),
          text: cleanText(html),
          author: undefined,
          domain: extractDomain(url) ?? undefined,
          publishedAt: undefined,
          leadImageUrl: undefined,
          url,
        };
      }),
    ),
  );
}
