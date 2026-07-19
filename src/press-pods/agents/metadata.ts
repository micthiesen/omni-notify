import { generateObject } from "ai";
import { isValid } from "date-fns";
import { z } from "zod";
import { getPressPodsMetadataModel } from "../../ai/registry.js";
import type CostCounter from "../costs.js";
import type { CompletionUsage } from "../costs.js";
import type { Article } from "../types.js";

const rawMetadataInfoSchema = z.object({
  isValidArticle: z.boolean(),
  title: z.string().nullable(),
  author: z.string().nullable(),
  authorGender: z
    .union([z.literal("male"), z.literal("female"), z.literal("unknown")])
    .nullable(),
  coauthors: z.array(z.string()).nullable(),
  publication: z.string().nullable(),
  publishedAtISO: z.string().nullable(),
  leadImageUrl: z.string().nullable(),
  shortSummary: z.string().nullable(),
  contentRating: z.number().nullable(),
});

export interface MetadataInfo {
  isValidArticle: boolean;
  title: string | null;
  author: string | undefined;
  authorGender: "male" | "female" | "unknown" | null;
  coauthors: string[] | null;
  publication: string | null;
  publishedAtISO: Date | undefined;
  leadImageUrl: string | null;
  shortSummary: string | null;
  contentRating: number;
}
export interface Metadata {
  info: MetadataInfo;
}

export async function getArticleMetadata(
  article: Article,
  costCounter: CostCounter,
): Promise<Metadata> {
  const { model, modelId } = getPressPodsMetadataModel();
  const { object, usage } = await generateObject({
    model,
    schema: rawMetadataInfoSchema,
    messages: [
      {
        role: "system",
        content: `You are an expert at extracting article metadata for podcast generation. Your goal is to extract useful metadata that enhances the listening experience. When uncertain, make reasonable inferences rather than leaving fields empty - approximate data is better than none for our use case.

Given webpage content, determine if it's valid and extract metadata. In the case of multiple authors, choose one as the primary.

Mark as INVALID only if:
  - Error page (404, 500, access denied, etc.)
  - Login or paywall with no content preview
  - Completely empty or blank page
  - Non-article media page (just a video/audio player with no text)

Content Rating (0-10) - Rate how successfully we extracted the article:
  - 10: Complete article captured perfectly
  - 7-9: Main article content captured, minor ads/nav elements included
  - 4-6: Partial content, significant sections missing or truncated
  - 0-3: Extraction mostly failed (got mainly ads/nav instead of article)
  - This measures extraction quality, NOT article quality

For author extraction, try in order:
  1. Byline (e.g., "By Jane Smith")
  2. Author bio section
  3. URL pattern (e.g., /author/jane-smith/)
  4. Copyright or attribution notice
  5. Reasonable inference from domain/publication

For author gender:
  - Infer from pronouns in bio (she/her → female, he/him → male)
  - Statistical inference from first name is acceptable
  - Only use 'unknown' if truly ambiguous - educated guesses preferred

For shortSummary: Create a one-sentence description of what this article is about (for podcast descriptions). Do NOT include URLs in the summary.

Other webpages (blog posts, wiki pages, forum threads, documentation) should be treated as valid articles.`,
      },
      {
        role: "user",
        content: `Please validate the following article and extract its metadata if it is valid.

Potential Article Info:
Title: ${article.title || ""}
Author: ${article.author || ""}
Domain: ${article.domain || ""}
Article URL: ${article.url || ""}
Published At: ${article.publishedAt || ""}
Lead Image URL: ${article.leadImageUrl || ""}

Webpage Content (HTML converted to plain text):
${article.text}`,
      },
    ],
  });

  const completionUsage: CompletionUsage = {
    promptTokens: usage.inputTokens || 0,
    completionTokens: usage.outputTokens || 0,
    totalTokens: usage.totalTokens || 0,
  };
  costCounter.recordLlmUsage(modelId, "meta", completionUsage);

  return { info: transformMetadataInfo(object) };
}

function stripUrls(text: string): string {
  return text
    .replace(/https?:\/\/\S+/g, "")
    .replace(/\n{2,}/g, "\n")
    .trim();
}

function transformMetadataInfo(
  raw: z.infer<typeof rawMetadataInfoSchema>,
): MetadataInfo {
  return {
    ...raw,
    author: !raw.author || raw.author.includes("unknown") ? undefined : raw.author,
    shortSummary: raw.shortSummary ? stripUrls(raw.shortSummary) : null,
    publishedAtISO: (() => {
      if (!raw.publishedAtISO) return undefined;
      const date = new Date(raw.publishedAtISO);
      return isValid(date) ? date : undefined;
    })(),
    contentRating: raw.contentRating ?? 0,
  };
}
