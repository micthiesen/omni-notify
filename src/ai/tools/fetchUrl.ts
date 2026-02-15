import { isProbablyReaderable, Readability } from "@mozilla/readability";
import { tool } from "ai";
import got from "got";
import { parseHTML } from "linkedom";
import TurndownService from "turndown";
import { gfm } from "turndown-plugin-gfm";
import { z } from "zod";

const TIMEOUT_MS = 15_000;
export const MAX_OUTPUT_CHARS = 20_000;

const turndown = new TurndownService({
  headingStyle: "atx",
  codeBlockStyle: "fenced",
});
turndown.use(gfm);

export const fetchUrl = tool({
  description:
    "Fetch a web page and return its content as clean markdown. Use this to read full articles, documentation, or other pages found via web_search.",
  inputSchema: z.object({
    url: z.string().url().describe("The URL to fetch"),
  }),
  execute: async ({ url }) => {
    const html = await got(url, {
      timeout: { request: TIMEOUT_MS },
      headers: {
        "User-Agent":
          "Mozilla/5.0 (compatible; OmniNotify/1.0; +https://github.com/micthiesen/omni-notify)",
        Accept: "text/html",
      },
    }).text();

    return htmlToMarkdown(html);
  },
});

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
