import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { WorkspaceMarkdown } from "./WorkspaceMarkdown";

function render(content: string) {
  return renderToStaticMarkup(createElement(WorkspaceMarkdown, { content }));
}

describe("WorkspaceMarkdown", () => {
  it("renders research comparisons as headings, lists, and semantic GFM tables", () => {
    const html = render(`# Shortlist

A **durable** choice for daily use.

- Replaceable battery
- Two-year warranty

| Model | Price |
| --- | --- |
| Example | $20 |

[Manufacturer](https://example.com/product)
`);

    expect(html).toContain("<h4>Shortlist</h4>");
    expect(html).toContain("<strong>durable</strong>");
    expect(html).toContain("<li>Replaceable battery</li>");
    expect(html).toContain("<table>");
    expect(html).toContain("<th>Model</th>");
    expect(html).toContain("<td>$20</td>");
    expect(html).toContain('href="https://example.com/product"');
    expect(html).toContain('rel="noopener noreferrer"');
  });

  it("does not execute raw HTML embedded in generated research", () => {
    const html = render(`Useful research.

<script>alert(document.cookie)</script>

<img src=x onerror="alert(1)">

<iframe src="https://example.com"></iframe>

Still useful research.`);

    expect(html).toContain("Useful research.");
    expect(html).toContain("Still useful research.");
    expect(html).not.toContain("<script");
    expect(html).not.toContain("<img");
    expect(html).not.toContain("<iframe");
    expect(html).not.toContain("onerror");
  });

  it.each([
    "javascript:alert%281%29",
    "JaVaScRiPt:alert%281%29",
    "data:text/html;base64,PHNjcmlwdD4=",
    "vbscript:msgbox%281%29",
  ])("keeps link text but removes unsafe destination %s", (destination) => {
    const html = render(`[Research source](${destination})`);

    expect(html).toContain("Research source");
    expect(html).not.toContain("href=");
    expect(html).not.toContain(destination);
  });
});
