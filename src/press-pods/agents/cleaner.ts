import { Logger } from "@micthiesen/mitools/logging";
import { generateText } from "ai";
import { getPressPodsCleaningModel } from "../../ai/registry.js";
import type CostCounter from "../costs.js";
import type { CompletionUsage } from "../costs.js";
import type { Article } from "../types.js";
import { extractBetweenTags } from "./parsing.js";

const LOGGER = new Logger("PressPods.agents.cleaner");

/** Re-synthesize the narration if the model returns unusable/untagged output. */
const MAX_CLEAN_ATTEMPTS = 3;

/** Adapt the article text for TTS narration (junk removal, audio phrasing). */
export async function getCleanedArticle(
  article: Article,
  costCounter: CostCounter,
): Promise<{ content: string }> {
  const { model, modelId } = getPressPodsCleaningModel();
  let lastError: unknown;

  for (let attempt = 1; attempt <= MAX_CLEAN_ATTEMPTS; attempt++) {
    const { text, usage } = await generateText({
      model,
      system: `You adapt a written article into a script for a single podcast host to read aloud (text-to-speech). Preserve the article's content and meaning faithfully — do NOT summarize, editorialize, or invent facts. Your job is to make it sound like an engaging host reading for the ear, not an eye.

The input begins with a header line like "Title. By Author. Published Date on Domain." Use its facts, but restructure the opening (see OPENING).

REMOVE:
- Website chrome: navigation, sidebars, footer links, "← Previous / Next →" links
- Junk: ads, subscribe/signup prompts, social buttons, cookie notices, share links
- Non-narrative elements: author bios, related articles, comments, decorative image captions, photo credits
- Duplicate titles (the article title often appears multiple times)
- Charts, tables, diagrams, ASCII art, emojis
- Footnotes, "[return]" back-links, and footnote references ([1], etc.)
- Academic citations like (Smith, 2024) — except within direct quotes
- Bylines, datelines, and captions read literally; URLs — drop them, or fold into prose ("as the Times reported"). Never read a raw URL aloud.

WRITE FOR THE EAR:
- One idea per sentence. Break long print sentences into several short spoken ones. Avoid stacked dependent clauses.
- Use contractions and a natural, present-tense-leaning voice. If you wouldn't say it aloud, don't write it.
- Attribution comes BEFORE the quote or claim, never after: "The report's author, Jane Doe, argues that…" — not "…, Doe argues." Prefer reported speech ("she said the plan would fail") unless the exact wording matters.
- Blockquotes (lines starting with ">"): integrate naturally. If context already attributes it, just read it; otherwise introduce it ("As the report puts it,"). Remove the ">"; never say "quote"/"unquote".
- Social threads: join consecutive posts into one continuous narration. Remove structural labels, post numbers, handles, and engagement metadata.
- Lines beginning "Image description:" contain author-provided context. Preserve the useful information as a concise spoken description when the surrounding text depends on the image; omit it when decorative or redundant.
- Lists and bullets: rewrite as flowing prose with transitions ("First… Then… Finally…").
- Numbers are hard to hear: round them ("nearly two million dollars", not "$1,987,452"), at most one figure per sentence. Give a percentage a baseline when the article implies one.

OPENING (make it feel like a produced episode):
- Start with a ONE- or TWO-sentence hook drawn from the article's most interesting idea (a question, a striking fact, or a scene). Do not fabricate — the hook must be true to the article.
- Then read in the framing naturally: "That's from [Title], by [Author], published [Date] in [Publication]." Skip fields that are missing or "Anonymous".
- Then the body.

OUTRO:
- End with ONE short spoken line: "That was [Title], by [Author]." (drop author if unknown). Nothing more.

SECTIONS (for chapter markers):
- If the article has clear major sections, mark each with a line "## Short Title" (2-5 words) on its own line, immediately before that section's narration. These lines are NOT spoken — they only mark chapters — so keep them short and descriptive.
- Do NOT add a "## " marker before the opening hook/intro, and do NOT invent sections. If the article has no natural sections, use no "## " markers at all.

NORMALIZE (spell out what TTS mispronounces):
- Dates: "3/15/24" → "March 15th, 2024", "Q3 2023" → "third quarter of 2023". Ranges: "2019–2023" → "2019 to 2023".
- Currency: "$4.2B" → "4.2 billion dollars", "$45.67" → "forty-five dollars and sixty-seven cents".
- Version numbers: "v2.1" → "version two point one". Leading decimals: ".22 rifle" → "twenty-two rifle".
- Abbreviations: expand ones a voice stumbles on ("govt" → "government", "approx." → "approximately"). Strip periods from initialisms ("U.F.O." → "UFO", "C.I.A." → "CIA"). Keep naturally-spoken ones (Dr., Mr., U.S., AI, CEO).
- Code/math: short expressions can stay; describe longer code blocks in words.

Output ONLY the finished script wrapped in <cleaned_article> and </cleaned_article> tags — nothing before or after. Always emit the closing </cleaned_article> tag.`,
      prompt: article.text,
    });

    const completionUsage: CompletionUsage = {
      promptTokens: usage.inputTokens || 0,
      completionTokens: usage.outputTokens || 0,
      totalTokens: usage.totalTokens || 0,
    };
    costCounter.recordLlmUsage(modelId, "clean", completionUsage);

    try {
      return { content: extractBetweenTags(text, "cleaned_article") };
    } catch (error) {
      lastError = error;
      LOGGER.info(
        `Narration cleaning attempt ${attempt}/${MAX_CLEAN_ATTEMPTS} produced no usable output; retrying`,
      );
    }
  }

  throw lastError;
}
