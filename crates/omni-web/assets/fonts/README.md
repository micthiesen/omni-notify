# Self-hosted fonts

The UI uses Geist and Geist Mono (SIL Open Font License 1.1). Add these files
here; nothing loads them from a CDN:

- `Geist-Variable.woff2`
- `GeistMono-Variable.woff2`
- `OFL.txt` (the license that ships with the fonts)

Source: the official Geist release (github.com/vercel/geist-font), `fonts/Geist/webfonts`
and `fonts/GeistMono/webfonts`, variable `.woff2` files.

`index.html` copies this directory to `dist/assets/fonts/` with Trunk's
`copy-dir` and preloads both files. `style/tokens.css` declares the
`@font-face` rules. Copied files are not content-hashed, so a font update
keeps its URL; rename the file (and update both references) to bust caches.
The files here are the official variable webfonts (Latin), committed with their license.
