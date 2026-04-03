---
name: pushover
description: >-
  Create a new Pushover application with a custom icon. Searches for icons,
  lets the user pick one, then creates the app via Pushover's web interface
  using 1Password credentials.
user_invocable: true
argument-hint: <description of what the new app/token is for> [image:<path>]
---

# Create Pushover Application

Create a new Pushover application (API token) with a custom icon.

## Scripts

Located at `.claude/skills/pushover/scripts/`:

- **pushover-session.mjs** - Logs into Pushover via 1Password (email + password + TOTP). Prints session cookie to stdout.
- **pushover-create-app.mjs** - Creates a Pushover app. Args: `<cookie> <name> [icon-path] [description]`. Prints JSON `{ "token": "...", "url": "..." }`.
- **search-icons.mjs** - Searches Iconify for icons, downloads as 128x128 PNGs to `.pushover-icons/`. Args: `<query> [count]`.
- **download-icon.mjs** - Downloads a single image URL to `.pushover-icons/`, converts to 128x128 PNG. Args: `<url> [filename]`. Prints output path.
- **clear-icons.sh** - Clears `.pushover-icons/`.

## Steps

### 1. Parse arguments

From `$ARGUMENTS`, extract:
- The **description/purpose** of the new app (e.g. "live stream notifications")
- Optional explicit image path if the user provided one (e.g. `image:/path/to/icon.png` or just a file path)

### 2. Choose an app name

Derive a short name (20 char max) from the description. This appears as the notification source when no title is set. Confirm with the user before proceeding.

### 3. Find an icon

**If the user provided an image path or URL:** use it directly. If it's a URL, download with:
```bash
node .claude/skills/pushover/scripts/download-icon.mjs "<url>" "chosen"
```
If it's a local file that isn't 128x128 PNG, resize with `sips --resampleWidth 128 --resampleHeight 128`.

**Otherwise:** Search Iconify for candidates:
```bash
node .claude/skills/pushover/scripts/search-icons.mjs "<search terms>" 6
```

This downloads 128x128 PNGs to `.pushover-icons/`. Show each image to the user using the Read tool (it can display images).

The user will either:
- **Pick one** - proceed with that icon
- **Reject all** - search again with different terms (script clears old results)
- **Provide a URL** - download it with `download-icon.mjs`
- **Skip** - create the app without an icon

### 4. Create the application

```bash
# Get session cookie (requires 1Password CLI + biometric/auth)
COOKIE="$(node .claude/skills/pushover/scripts/pushover-session.mjs)"

# Create app
node .claude/skills/pushover/scripts/pushover-create-app.mjs "$COOKIE" "AppName" ".pushover-icons/chosen.png" "Description of the app"
```

The create script prints JSON: `{ "token": "abc123...", "url": "https://pushover.net/apps/..." }`

### 5. Report results

Show the user:
- The new **API token**
- The **Pushover app URL** (for managing the icon/settings later)
- Suggest which **env var** to update if relevant to this project

Clean up: `bash .claude/skills/pushover/scripts/clear-icons.sh`

## User Instructions

$ARGUMENTS
