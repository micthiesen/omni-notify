import type { McpRuntime } from "../runtime.js";
import type { McpToolDefinition } from "../tool.js";
import { createCalendarTools } from "./calendar.js";
import { createEmailTools } from "./email.js";
import { createEmailComposeTools } from "./email-compose.js";
import { createEmailArchiveTools } from "./email-archive.js";
import { createEventTools } from "./events.js";
import { createEmailAttachmentTools } from "./email-attachments.js";
import { createMediaTools } from "./media.js";
import { createPersonalTools } from "./personal.js";
import { createPodcastTools } from "./podcasts.js";
import { createPressPodsTools } from "./press-pods.js";
import { createSystemTools } from "./system.js";
import { createWorkspaceTools } from "./workspaces.js";
import { createPrinterTools } from "./printer.js";
import { createBrowserHistoryTools } from "./browser-history.js";
import { createClaudeSessionTools } from "./claude-sessions.js";
import { createRemindersTools } from "./reminders.js";

export function createToolDefinitions(runtime: McpRuntime): McpToolDefinition[] {
  return [
    ...createRemindersTools(runtime),
    ...createSystemTools(runtime),
    ...createWorkspaceTools(runtime),
    ...createEmailTools(runtime),
    ...createCalendarTools(runtime),
    ...createEmailComposeTools(runtime),
    ...createEmailArchiveTools(runtime),
    ...createEventTools(runtime),
    ...createEmailAttachmentTools(runtime),
    ...createMediaTools(),
    ...createPodcastTools(runtime),
    ...createPressPodsTools(runtime),
    ...createPersonalTools(),
    ...createPrinterTools(runtime),
    ...createBrowserHistoryTools(runtime),
    ...createClaudeSessionTools(runtime),
  ];
}
