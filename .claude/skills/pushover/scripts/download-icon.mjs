#!/usr/bin/env node

// Downloads an image URL to .pushover-icons/ and converts to 128x128 PNG.
// Usage: node download-icon.mjs <url> [filename]
//
// Requires: rsvg-convert (brew install librsvg) for SVG input
// Prints the output PNG path to stdout.

import { writeFileSync, mkdirSync, unlinkSync } from "fs";
import { join, extname } from "path";
import { execSync } from "child_process";

const [, , url, filename] = process.argv;
if (!url) {
  console.error("Usage: download-icon.mjs <url> [filename]");
  process.exit(1);
}

const ICONS_DIR = join(process.cwd(), ".pushover-icons");
mkdirSync(ICONS_DIR, { recursive: true });

const res = await fetch(url, { signal: AbortSignal.timeout(10000) });
if (!res.ok) {
  console.error(`Failed to download: HTTP ${res.status}`);
  process.exit(1);
}

const buf = Buffer.from(await res.arrayBuffer());
const contentType = res.headers.get("content-type") || "";

// Detect format
let ext = extname(new URL(url).pathname).toLowerCase() || ".png";
if (contentType.includes("svg")) ext = ".svg";
else if (contentType.includes("png")) ext = ".png";
else if (contentType.includes("jpeg") || contentType.includes("jpg")) ext = ".jpg";
else if (contentType.includes("webp")) ext = ".webp";

const baseName = (filename || "icon").replace(/\.[^.]+$/, "");
const srcPath = join(ICONS_DIR, `${baseName}${ext}`);
const pngPath = join(ICONS_DIR, `${baseName}.png`);

writeFileSync(srcPath, buf);

if (ext === ".svg") {
  execSync(`rsvg-convert -w 128 -h 128 "${srcPath}" -o "${pngPath}"`);
  unlinkSync(srcPath);
} else if (ext !== ".png") {
  execSync(`sips -s format png "${srcPath}" --out "${pngPath}" 2>&1`);
  execSync(`sips --resampleWidth 128 --resampleHeight 128 "${pngPath}" 2>&1`);
  unlinkSync(srcPath);
} else {
  execSync(`sips --resampleWidth 128 --resampleHeight 128 "${pngPath}" 2>&1`);
}

console.log(pngPath);
