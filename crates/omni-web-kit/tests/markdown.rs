//! Markdown rendering (`wasm-bindgen-test`; also runs natively).

use omni_web_kit::markdown::render_markdown_html;
use wasm_bindgen_test::wasm_bindgen_test;

#[wasm_bindgen_test]
fn renders_headings_lists_and_semantic_gfm_tables() {
    let html = render_markdown_html(
        "# Shortlist\n\nA **durable** choice for daily use.\n\n- Replaceable battery\n- Two-year warranty\n\n| Model | Price |\n| --- | --- |\n| Example | $20 |\n\n[Manufacturer](https://example.com/product)\n",
    );
    assert!(html.contains("<h4>Shortlist</h4>"));
    assert!(html.contains("<strong>durable</strong>"));
    assert!(html.contains("<li>Replaceable battery</li>"));
    assert!(html.contains("<table>"));
    assert!(html.contains("<th>Model</th>"));
    assert!(html.contains("<td>$20</td>"));
    assert!(html.contains("href=\"https://example.com/product\""));
    assert!(html.contains("rel=\"noopener noreferrer\""));
}

#[wasm_bindgen_test]
fn does_not_execute_raw_html_embedded_in_generated_text() {
    let html = render_markdown_html(
        "Useful text.\n\n<script>alert(document.cookie)</script>\n\n<img src=x onerror=\"alert(1)\">\n\n<iframe src=\"https://example.com\"></iframe>\n\nStill useful text.",
    );
    assert!(html.contains("Useful text."));
    assert!(html.contains("Still useful text."));
    assert!(!html.contains("<script"));
    assert!(!html.contains("<img"));
    assert!(!html.contains("<iframe"));
    assert!(!html.contains("onerror"));
}

#[wasm_bindgen_test]
fn keeps_link_text_but_removes_unsafe_destination() {
    for destination in [
        "javascript:alert%281%29",
        "JaVaScRiPt:alert%281%29",
        "data:text/html;base64,PHNjcmlwdD4=",
        "vbscript:msgbox%281%29",
    ] {
        let html = render_markdown_html(&format!("[Source]({destination})"));
        assert!(html.contains("Source"), "{destination}");
        assert!(!html.contains("href="), "{destination}");
        assert!(!html.contains(destination), "{destination}");
    }
}
