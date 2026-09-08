#!/usr/bin/env node

// Searches Iconify for icons and downloads them as 128x128 PNGs.
// Usage: node search-icons.mjs <query> [count]
//
// Requires: rsvg-convert (brew install librsvg)

import { writeFileSync, mkdirSync, readdirSync, unlinkSync } from "fs";
import { join } from "path";
import { execSync } from "child_process";

const [, , ...args] = process.argv;
if (args.length === 0) {
  console.error("Usage: search-icons.mjs <query> [count]");
  process.exit(1);
}

const count = parseInt(args[args.length - 1]);
const hasCount = !isNaN(count);
const query = hasCount ? args.slice(0, -1).join(" ") : args.join(" ");
const maxResults = hasCount ? count : 6;

const ICONS_DIR = join(process.cwd(), ".pushover-icons");
mkdirSync(ICONS_DIR, { recursive: true });

// Clear old candidates
for (const f of readdirSync(ICONS_DIR)) {
  unlinkSync(join(ICONS_DIR, f));
}

console.log(`Searching Iconify for: "${query}" (up to ${maxResults} results)\n`);

const res = await fetch(
  `https://api.iconify.design/search?query=${encodeURIComponent(query)}&limit=30`
);
const data = await res.json();

if (!data.icons || data.icons.length === 0) {
  console.log("No icons found on Iconify.");
  console.log("Try different search terms, or provide an image URL/path directly.");
  process.exit(0);
}

let downloaded = 0;
for (const icon of data.icons) {
  if (downloaded >= maxResults) break;
  const [prefix, name] = icon.split(":");
  const svgUrl = `https://api.iconify.design/${prefix}/${name}.svg?width=128&height=128`;

  try {
    const svgRes = await fetch(svgUrl);
    if (!svgRes.ok) continue;
    const svg = await svgRes.text();

    const svgPath = join(ICONS_DIR, `${downloaded + 1}_${prefix}_${name}.svg`);
    const pngPath = join(ICONS_DIR, `${downloaded + 1}_${prefix}_${name}.png`);
    writeFileSync(svgPath, svg);

    // Convert SVG to 128x128 PNG
    execSync(
      `rsvg-convert -w 128 -h 128 "${svgPath}" -o "${pngPath}" 2>&1`
    );
    // Remove SVG, keep only PNG
    unlinkSync(svgPath);

    console.log(`  ${downloaded + 1}. ${prefix}:${name} → ${pngPath}`);
    downloaded++;
  } catch (e) {
    // Skip failed conversions
  }
}

if (downloaded === 0) {
  console.log("Failed to download/convert any icons.");
  console.log("Try different search terms, or provide an image URL/path directly.");
} else {
  console.log(`\n${downloaded} icon(s) saved to ${ICONS_DIR}`);
}
