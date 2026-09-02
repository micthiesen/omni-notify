import { isProbablyReaderable, Readability } from "@mozilla/readability";
import { tool } from "ai";
import { Data, Effect } from "effect";
import { parseHTML } from "linkedom";
import TurndownService from "turndown";
import { gfm } from "turndown-plugin-gfm";
import { z } from "zod";
import { fetchPublicText, PUBLIC_HTTP_USER_AGENT } from "../../effect/publicHttp.js";

const TIMEOUT_MS = 15_000;
export const MAX_OUTPUT_CHARS = 20_000;

const turndown = new TurndownService({
  headingStyle: "atx",
  codeBlockStyle: "fenced",
});
turndown.use(gfm);

export class FetchUrlError extends Data.TaggedError("FetchUrlError")<{
  readonly url: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    return `Fetch ${this.url} failed: ${this.cause instanceof Error ? this.cause.message : String(this.cause)}`;
  }
}

export function fetchUrlEffect(
  url: string,
): Effect.Effect<HtmlToMarkdownResult, FetchUrlError> {
  return fetchPublicText(
    url,
    {
      timeout: { request: TIMEOUT_MS },
      headers: {
        "User-Agent": PUBLIC_HTTP_USER_AGENT,
        Accept: "text/html",
      },
    },
    "fetch public page",
  ).pipe(
    Effect.mapError((cause) => new FetchUrlError({ url, cause })),
    Effect.map(htmlToMarkdown),
  );
}

export function makeFetchUrlTool(runner: EffectRunner<Logger | Docstore>) {
  return tool({
    description:
      "Fetch a web page and return its content as clean markdown. Use this to read full articles, documentation, or other pages found via web_search.",
    inputSchema: z.object({
      url: z.string().url().describe("The URL to fetch"),
    }),
    execute: ({ url }) => runner.runPromise(fetchUrlEffect(url)),
  });
}

export interface HtmlToMarkdownResult {
  title: string | null;
  content: string;
  truncated: boolean;
}

export function htmlToMarkdown(html: string): HtmlToMarkdownResult {
  const { document } = parseHTML(html);

  let title: string | null = null;
  let contentHtml: string;

  if (isProbablyReaderable(document as unknown as Document)) {
    const article = new Readability(document as unknown as Document).parse();
    if (article?.content) {
      contentHtml = article.content;
      title = article.title ?? null;
    } else {
      contentHtml = fallbackExtract(html);
    }
  } else {
    contentHtml = fallbackExtract(html);
    title = document.querySelector("title")?.textContent?.trim() ?? null;
  }

  let markdown = turndown.turndown(contentHtml);
  if (title) {
    markdown = `# ${title}\n\n${markdown}`;
  }

  const truncated = markdown.length > MAX_OUTPUT_CHARS;
  if (truncated) {
    markdown = markdown.slice(0, MAX_OUTPUT_CHARS);
  }

  return { title, content: markdown, truncated };
}

function fallbackExtract(html: string): string {
  const { document } = parseHTML(html);
  for (const tag of ["script", "style", "nav", "footer", "header", "aside", "svg"]) {
    for (const el of document.querySelectorAll(tag)) el.remove();
  }
  const main = document.querySelector("main, article, [role='main']");
  return (main ?? document.body)?.innerHTML ?? html;
}
import type { EffectRunner } from "@micthiesen/mitools/boundary";
import type { Docstore } from "@micthiesen/mitools/docstore";
import type { Logger } from "@micthiesen/mitools/logging";
