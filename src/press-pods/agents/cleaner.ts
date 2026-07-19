import { generateText } from "ai";
import { getPressPodsCleaningModel } from "../../ai/registry.js";
import type CostCounter from "../costs.js";
import type { CompletionUsage } from "../costs.js";
import type { Article } from "../types.js";
import { extractBetweenTags } from "./parsing.js";

/** Adapt the article text for TTS narration (junk removal, audio phrasing). */
export async function getCleanedArticle(
  article: Article,
  costCounter: CostCounter,
): Promise<{ content: string }> {
  const { model, modelId } = getPressPodsCleaningModel();
  const { text, usage } = await generateText({
    model,
    system: `Adapt this article for podcast narration. The output will be read aloud by a text-to-speech voice.

Preserve the article's content faithfully. Your job is to make it sound natural when spoken, not to summarize or editorialize.

The input begins with a spoken header line like "Title. By Author. Published Date on Domain." — keep this as the opening line, unchanged.

REMOVE:
- Website chrome: navigation, sidebars, footer links, "← Previous / Next →" links
- Junk: ads, subscribe/signup prompts, social buttons, cookie notices, share links
- Non-narrative elements: author bios, related articles, comments, image captions, photo credits
- Duplicate titles (the article title often appears multiple times — keep only the first)
- Charts, tables, diagrams, ASCII art, emojis
- Footnotes, "[return]" back-links, and footnote references ([1], etc.)
- Academic citations like (Smith, 2024) — except within direct quotes
- Complex URLs — replace with "link to [domain]" only if essential, otherwise drop

ADAPT FOR AUDIO:

Blockquotes (lines starting with ">"):
- These are quoted passages. Integrate them naturally into the narration.
- If surrounding text provides attribution (e.g., "The CEO said:"), just read the quote directly — the context is enough.
- If a blockquote has no clear attribution, introduce it naturally (e.g., "As the article states:" or "To quote the report:").
- Remove the ">" prefix; never say the word "quote" or "unquote" literally.

Lists and bullet points:
- Convert to flowing prose with natural transitions.

Dates:
- Use pronounceable format: "3/15/24" → "March 15th, 2024", "Q3 2023" → "third quarter of 2023"

Code and math:
- Short expressions (under ~10 characters) can stay as-is.
- Longer code blocks: replace with a natural description of what the code does.

Abbreviations:
- Expand shortened forms that a voice would stumble on: "govt" → "government", "approx." → "approximately", "dept" → "department", etc.
- Strip periods from letter-by-letter abbreviations: "U.F.O." → "UFO", "C.I.A." → "CIA", "F.D.R." → "FDR", "S.T.D.s" → "STDs", "A.D.D." → "ADD". TTS handles these better without periods.
- Common abbreviations that are naturally spoken (Dr., Mr., U.S., AI, CEO) can stay as-is.

Numbers and measurements:
- Leading decimals: ".40-caliber" → "40-caliber", ".22 rifle" → "22 rifle"
- Spell out numbers that would sound unnatural as digits when read aloud.

Output the adapted text inside <cleaned_article> tags.`,
    prompt: article.text,
  });

  const completionUsage: CompletionUsage = {
    promptTokens: usage.inputTokens || 0,
    completionTokens: usage.outputTokens || 0,
    totalTokens: usage.totalTokens || 0,
  };
  costCounter.recordLlmUsage(modelId, "clean", completionUsage);

  return { content: extractBetweenTags(text, "cleaned_article") };
}
