//! Port of `src/ai/tools/fetchUrl.spec.ts` (`htmlToMarkdown`).

use omni_ai::tools::{MAX_OUTPUT_CHARS, html_to_markdown};

#[test]
fn converts_a_simple_article_page() {
    let html = r#"
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
    "#;
    let result = html_to_markdown(html);
    assert!(
        result.content.contains("first paragraph"),
        "{}",
        result.content
    );
    assert!(result.content.contains("**bold**"), "{}", result.content);
    assert!(
        result.content.contains("[link](https://example.com)"),
        "{}",
        result.content
    );
    assert!(!result.truncated);
}

#[test]
fn strips_scripts_styles_and_nav_elements_in_fallback() {
    let html = r#"
      <html><head><title>Simple Page</title></head>
      <body>
        <script>alert("xss")</script>
        <style>.red { color: red }</style>
        <nav><a href="/">Home</a><a href="/about">About</a></nav>
        <main><p>Main content here.</p></main>
        <footer><p>Footer stuff</p></footer>
        <aside><p>Sidebar</p></aside>
      </body></html>
    "#;
    let result = html_to_markdown(html);
    assert!(result.content.contains("Main content here"));
    assert!(!result.content.contains("alert"));
    assert!(!result.content.contains(".red"));
    assert!(!result.content.contains("Footer stuff"));
    assert!(!result.content.contains("Sidebar"));
}

#[test]
fn extracts_title_from_title_tag_in_fallback_mode() {
    let html = r#"
      <html><head><title>My Page Title</title></head>
      <body><main><p>Short content.</p></main></body></html>
    "#;
    let result = html_to_markdown(html);
    assert_eq!(result.title.as_deref(), Some("My Page Title"));
    assert!(result.content.contains("# My Page Title"));
}

#[test]
fn converts_html_tables_to_markdown() {
    let html = r#"
      <html><head><title>Data</title></head>
      <body><main>
        <table>
          <thead><tr><th>Name</th><th>Value</th></tr></thead>
          <tbody><tr><td>Alpha</td><td>100</td></tr></tbody>
        </table>
      </main></body></html>
    "#;
    let result = html_to_markdown(html);
    assert!(result.content.contains("Name"));
    assert!(result.content.contains("Alpha"));
    assert!(result.content.contains("100"));
}

#[test]
fn converts_code_blocks() {
    let html = r#"
      <html><head><title>Code</title></head>
      <body><main><pre><code>const x = 42;</code></pre></main></body></html>
    "#;
    let result = html_to_markdown(html);
    assert!(result.content.contains("```"), "{}", result.content);
    assert!(result.content.contains("const x = 42;"));
}

#[test]
fn preserves_headings_as_atx_style_markdown() {
    let html = r#"
      <html><head><title>Docs</title></head>
      <body><main>
        <h2>Section One</h2>
        <p>Content under section one.</p>
        <h3>Subsection</h3>
        <p>Content under subsection.</p>
      </main></body></html>
    "#;
    let result = html_to_markdown(html);
    assert!(
        result.content.contains("## Section One"),
        "{}",
        result.content
    );
    assert!(result.content.contains("### Subsection"));
}

#[test]
fn truncates_long_content_and_sets_truncated_flag() {
    let paragraph = format!("<p>{}</p>\n", "A".repeat(1000));
    let html = format!(
        "<html><head><title>Long</title></head><body><main>{}</main></body></html>",
        paragraph.repeat(30)
    );
    let result = html_to_markdown(&html);
    assert!(result.truncated);
    assert!(omni_core::js::utf16_len(&result.content) <= MAX_OUTPUT_CHARS);
}

#[test]
fn does_not_truncate_content_within_limit() {
    let html = r#"
      <html><head><title>Short</title></head>
      <body><main><p>Short content.</p></main></body></html>
    "#;
    assert!(!html_to_markdown(html).truncated);
}

#[test]
fn handles_empty_body_gracefully() {
    let html = "<html><head><title>Empty</title></head><body></body></html>";
    let result = html_to_markdown(html);
    assert_eq!(result.title.as_deref(), Some("Empty"));
    assert!(!result.truncated);
}

#[test]
fn removes_svg_elements() {
    let html = r#"
      <html><head><title>Icons</title></head>
      <body><main>
        <svg xmlns="http://www.w3.org/2000/svg"><circle r="50"/></svg>
        <p>Actual content.</p>
      </main></body></html>
    "#;
    let result = html_to_markdown(html);
    assert!(result.content.contains("Actual content"));
    assert!(!result.content.contains("circle"));
    assert!(!result.content.contains("svg"));
}

#[test]
fn prefers_main_over_full_body_in_fallback() {
    let html = r#"
      <html><head><title>Test</title></head>
      <body>
        <div>Outside main</div>
        <main><p>Inside main.</p></main>
      </body></html>
    "#;
    let result = html_to_markdown(html);
    assert!(result.content.contains("Inside main"));
    assert!(!result.content.contains("Outside main"));
}

#[test]
fn prefers_article_when_no_main_exists_in_fallback() {
    let html = r#"
      <html><head><title>Test</title></head>
      <body>
        <div>Outside article</div>
        <article><p>Inside article.</p></article>
      </body></html>
    "#;
    let result = html_to_markdown(html);
    assert!(result.content.contains("Inside article"));
    assert!(!result.content.contains("Outside article"));
}
