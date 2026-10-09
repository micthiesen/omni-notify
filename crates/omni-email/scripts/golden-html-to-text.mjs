// Generates tests/golden/html_to_text.json from the TS `htmlToText`
// configuration (html-to-text 10) so the Rust port can be checked against it:
//   node crates/omni-email/scripts/golden-html-to-text.mjs
import { createRequire } from "node:module";
import { writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(join(here, "../../../package.json"));
const { convert } = require("html-to-text");

// Exactly the options of src/email/htmlToText.ts.
const htmlToText = (html) =>
  convert(html, {
    wordwrap: false,
    decodeEntities: true,
    selectors: [
      { selector: "a", options: { ignoreHref: true } },
      { selector: "img", format: "skip" },
      { selector: "h1", options: { uppercase: false } },
      { selector: "h2", options: { uppercase: false } },
      { selector: "h3", options: { uppercase: false } },
      { selector: "hr", format: "skip" },
      { selector: "table", options: { colSpacing: 2 } },
    ],
  });

const cases = [
  "<p>Hello   <b>big</b>\n world</p><p>Second</p>",
  "<div>a</div><div>b</div>",
  "line<br>next",
  "<a href=\"https://x.test/track\">Track</a> <img src=\"p.png\" alt=\"logo\"><hr>now",
  "<h1>Order</h1><h4>Note</h4>",
  "<ul><li>one</li><li>two</li></ul>",
  "<ol start=\"3\"><li>c</li><li>d</li></ol>",
  "<ol type=\"i\"><li>a</li><li>b</li><li>c</li><li>d</li></ol>",
  "<ol type=\"A\"><li>a</li><li>b</li></ol>",
  "<blockquote>q<br>r</blockquote>",
  "<html><head><title>T</title><style>p{}</style></head><body><script>x()</script>Hi</body></html>",
  "<title>T</title>Hi",
  "<ul><li>outer<ul><li>inner</li></ul></li><li>next</li></ul>",
  "<table><tr><td>Item</td><td>Qty</td></tr><tr><td>Shoes</td><td>1</td></tr></table>",
  "<pre>  keep\n   spaces </pre><p>after</p>",
  "Tracking&nbsp;number: <strong>1Z999</strong> &amp; more &lt;ok&gt;",
  "<div><p>Nested <span>inline</span></p><p>two</p></div><div>three</div>",
  "<section><header>Head</header><main>Body</main><footer>Foot</footer></section>",
  "text before<div>block</div>text after",
  "<p></p><p>  </p><p>x</p>",
  "<h2>Your order has shipped!</h2><p>Track it <a href=\"https://t.test/1\">here</a>.</p><h5>fine print</h5>",
  "a​b  c\td\r\ne",
  "<!-- comment --><p>visible</p><!-- trailing -->",
  "<ul>\n  <li>x</li>\n  <!-- c -->\n  <li>y</li>\n</ul>",
  "<div>Unicode: 日本語 — ünïcödé ✓</div>",
  "<br><br>start<br><br><br>end<br>",
  "<table><thead><tr><th>Name</th></tr></thead><tbody><tr><td>Value</td></tr></tbody></table>",
  "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>Shipped</title></head><body style=\"margin:0\"><table width=\"100%\" cellpadding=\"0\"><tr><td align=\"center\"><table><tr><td><img src=\"logo.png\" alt=\"Shop\"></td></tr><tr><td><h1 style=\"font-size:20px\">Your order is on its way!</h1><p>Hi Michael,</p><p>Good news &mdash; your package has shipped.</p><p><strong>Tracking number:</strong> <a href=\"https://www.ups.com/track?tracknum=1Z999AA10123456784\">1Z999AA10123456784</a></p></td></tr><tr><td><a href=\"https://shop.test/orders/1\" style=\"display:inline-block\">View order</a></td></tr></table></td></tr></table><div style=\"font-size:11px\">You received this email because&nbsp;you shopped with us.<br/>Unsubscribe</div></body></html>",
  "<p>Line one<br/>\n   Line two</p>\n\n<p>  Spaced   out  </p>",
  "<div><div><div>deep</div></div></div><span>tail</span>",
  "<p>Unclosed <b>bold <i>italic</p><p>next",
  "<ul><li><p>para in li</p></li><li>plain</li></ul>",
  "<center>Centered</center><font color=red>Font</font>",
  "<dl><dt>Term</dt><dd>Definition</dd></dl>",
  "<h3>Title</h3>\n<table><tr><td>A</td><td>B</td></tr></table>\n<h6>small</h6>",
];

const out = cases.map((html) => ({ html, text: htmlToText(html) }));
writeFileSync(join(here, "../tests/golden/html_to_text.json"), JSON.stringify(out, null, 2) + "\n");
console.log(`wrote ${out.length} cases`);
