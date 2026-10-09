// Regenerates crates/omni-presspods/tests/golden/rss.xml from the TypeScript feed
// builder, run from the repository root with `bun crates/omni-presspods/scripts/golden-rss.ts`.
//
// `buildPressPodsFeedFromEpisodes` is not exported from src/press-pods/rss.ts, so
// its source is extracted from that file and evaluated against the real
// `podcast`, mitools and `rssText` modules. The golden therefore always reflects
// the shipped TS implementation rather than a copy of it. `lastBuildDate` is the
// only time-dependent field; it is replaced by a fixed placeholder that the Rust
// test substitutes the same way.
import fs from "node:fs";
import path from "node:path";
import { truncate } from "@micthiesen/mitools/strings";
import { escapeXml } from "@micthiesen/mitools/xml";
import { Podcast } from "podcast";
import { prepareTextForRss } from "../../../src/press-pods/rssText.ts";

const root = path.resolve(import.meta.dir, "../../..");
const golden = path.join(root, "crates/omni-presspods/tests/golden");
const source = fs.readFileSync(path.join(root, "src/press-pods/rss.ts"), "utf8");

const fn = source.match(/function buildPressPodsFeedFromEpisodes\([\s\S]*?\n}\n/);
const limit = source.match(/const FEED_EPISODE_LIMIT = (\d+);/);
if (!fn || !limit) throw new Error("rss.ts no longer has the expected shape");

const js = new Bun.Transpiler({ loader: "ts" }).transformSync(fn[0]);
const build = new Function(
  "Podcast",
  "truncate",
  "escapeXml",
  "prepareTextForRss",
  "FEED_EPISODE_LIMIT",
  `${js}\nreturn buildPressPodsFeedFromEpisodes;`,
)(Podcast, truncate, escapeXml, prepareTextForRss, Number(limit[1]));

const episodes = JSON.parse(
  fs.readFileSync(path.join(golden, "rss-episodes.json"), "utf8"),
);
const xml: string = build("https://pods.example.test", episodes).replace(
  /<lastBuildDate>[^<]*<\/lastBuildDate>/,
  "<lastBuildDate>LAST_BUILD_DATE</lastBuildDate>",
);
fs.writeFileSync(path.join(golden, "rss.xml"), xml);
console.log(`wrote ${xml.length} bytes`);
