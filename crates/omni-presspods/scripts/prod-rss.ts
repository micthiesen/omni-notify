// Builds the TypeScript feed from a docstore copy, for comparison with the Rust
// feed over the same rows (`tests/prod_copy.rs`, OMNI_PROD_RSS):
//
//   bun crates/omni-presspods/scripts/prod-rss.ts <copy.db> <out.xml>
//
// Opens the copy, decodes every live `press-pods-episode` row
// with mitools, orders them like `getAllEpisodes` (newest first) and runs the
// builder extracted from src/press-pods/rss.ts (see golden-rss.ts).
import { Database } from "bun:sqlite";
import fs from "node:fs";
import path from "node:path";
import { decodeDoc } from "@micthiesen/mitools/docstore";
import { truncate } from "@micthiesen/mitools/strings";
import { escapeXml } from "@micthiesen/mitools/xml";
import { Podcast } from "podcast";
import { prepareTextForRss } from "../../../src/press-pods/rssText.ts";

const [dbPath, outPath] = process.argv.slice(2);
if (!dbPath || !outPath) throw new Error("usage: prod-rss.ts <copy.db> <out.xml>");

const root = path.resolve(import.meta.dir, "../../..");
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

// A private copy: WAL databases need write access to their -shm file even to read.
const db = new Database(dbPath);
const rows = db
  .query(
    "SELECT data FROM blobs WHERE entity = 'press-pods-episode' AND (expires_at IS NULL OR expires_at > ?)",
  )
  .all(Date.now()) as { data: Uint8Array }[];
const episodes = rows
  .map((row) => decodeDoc(Buffer.from(row.data)) as { createdAt: number })
  .sort((a, b) => b.createdAt - a.createdAt);
const xml: string = build("https://pods.example.test", episodes).replace(
  /<lastBuildDate>[^<]*<\/lastBuildDate>/,
  "<lastBuildDate>LAST_BUILD_DATE</lastBuildDate>",
);
fs.writeFileSync(outPath, xml);
console.log(`wrote ${episodes.length} episodes, ${xml.length} chars`);
