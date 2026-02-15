import { describe, expect, it } from "vitest";
import { htmlToMarkdown, MAX_OUTPUT_CHARS } from "./fetchUrl.js";

describe("htmlToMarkdown", () => {
  it("converts a simple article page", () => {
    const html = `
      <html><head><title>Test Article</title></head>
      <body>
        <nav><a href="/">Home</a></nav>
        <article>
          <h1>Test Article</h1>
          <p>This is the first paragraph of the article.</p>
          <p>This is the second paragraph with <strong>bold</strong> text.</p>
          <p>Another paragraph here with enough content to make readability happy.</p>
          <p>More content to ensure the article is long enough for extraction.</p>
          <p>Final paragraph with a <a href="https://example.com">link</a>.</p>
        </article>
        <footer>Copyright 2025</footer>
      </body></html>
    `;
    const result = htmlToMarkdown(html);
    expect(result.content).toContain("first paragraph");
    expect(result.content).toContain("**bold**");
    expect(result.content).toContain("[link](https://example.com)");
    expect(result.truncated).toBe(false);
  });

  it("strips scripts, styles, and nav elements in fallback", () => {
    const html = `
      <html><head><title>Simple Page</title></head>
      <body>
        <script>alert("xss")</script>
        <style>.red { color: red }</style>
        <nav><a href="/">Home</a><a href="/about">About</a></nav>
        <main><p>Main content here.</p></main>
        <footer><p>Footer stuff</p></footer>
        <aside><p>Sidebar</p></aside>
      </body></html>
    `;
    const result = htmlToMarkdown(html);
    expect(result.content).toContain("Main content here");
    expect(result.content).not.toContain("alert");
    expect(result.content).not.toContain(".red");
    expect(result.content).not.toContain("Footer stuff");
    expect(result.content).not.toContain("Sidebar");
  });

  it("extracts title from <title> tag in fallback mode", () => {
    const html = `
      <html><head><title>My Page Title</title></head>
      <body><main><p>Short content.</p></main></body></html>
    `;
    const result = htmlToMarkdown(html);
    expect(result.title).toBe("My Page Title");
    expect(result.content).toContain("# My Page Title");
  });

  it("converts HTML tables to markdown", () => {
    const html = `
      <html><head><title>Data</title></head>
      <body><main>
        <table>
          <thead><tr><th>Name</th><th>Value</th></tr></thead>
          <tbody><tr><td>Alpha</td><td>100</td></tr></tbody>
        </table>
      </main></body></html>
    `;
    const result = htmlToMarkdown(html);
    expect(result.content).toContain("Name");
    expect(result.content).toContain("Alpha");
    expect(result.content).toContain("100");
  });

  it("converts code blocks", () => {
    const html = `
      <html><head><title>Code</title></head>
      <body><main><pre><code>const x = 42;</code></pre></main></body></html>
    `;
    const result = htmlToMarkdown(html);
    expect(result.content).toContain("```");
    expect(result.content).toContain("const x = 42;");
  });

  it("preserves headings as ATX-style markdown", () => {
    const html = `
      <html><head><title>Docs</title></head>
      <body><main>
        <h2>Section One</h2>
        <p>Content under section one.</p>
        <h3>Subsection</h3>
        <p>Content under subsection.</p>
      </main></body></html>
    `;
    const result = htmlToMarkdown(html);
    expect(result.content).toContain("## Section One");
    expect(result.content).toContain("### Subsection");
  });

  it("truncates long content and sets truncated flag", () => {
    const paragraph = `<p>${"A".repeat(1000)}</p>\n`;
    const html = `
      <html><head><title>Long</title></head>
      <body><main>${paragraph.repeat(30)}</main></body></html>
    `;
    const result = htmlToMarkdown(html);
    expect(result.truncated).toBe(true);
    expect(result.content.length).toBeLessThanOrEqual(MAX_OUTPUT_CHARS);
  });

  it("does not truncate content within limit", () => {
    const html = `
      <html><head><title>Short</title></head>
      <body><main><p>Short content.</p></main></body></html>
    `;
    const result = htmlToMarkdown(html);
    expect(result.truncated).toBe(false);
  });

  it("handles empty body gracefully", () => {
    const html = "<html><head><title>Empty</title></head><body></body></html>";
    const result = htmlToMarkdown(html);
    expect(result.title).toBe("Empty");
    expect(result.truncated).toBe(false);
  });

  it("removes SVG elements", () => {
    const html = `
      <html><head><title>Icons</title></head>
      <body><main>
        <svg xmlns="http://www.w3.org/2000/svg"><circle r="50"/></svg>
        <p>Actual content.</p>
      </main></body></html>
    `;
    const result = htmlToMarkdown(html);
    expect(result.content).toContain("Actual content");
    expect(result.content).not.toContain("circle");
    expect(result.content).not.toContain("svg");
  });

  it("prefers <main> over full body in fallback", () => {
    const html = `
      <html><head><title>Test</title></head>
      <body>
        <div>Outside main</div>
        <main><p>Inside main.</p></main>
      </body></html>
    `;
    const result = htmlToMarkdown(html);
    expect(result.content).toContain("Inside main");
    expect(result.content).not.toContain("Outside main");
  });

  it("prefers <article> when no <main> exists in fallback", () => {
    const html = `
      <html><head><title>Test</title></head>
      <body>
        <div>Outside article</div>
        <article><p>Inside article.</p></article>
      </body></html>
    `;
    const result = htmlToMarkdown(html);
    expect(result.content).toContain("Inside article");
    expect(result.content).not.toContain("Outside article");
  });
});
