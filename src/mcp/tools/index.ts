import type { McpRuntime } from "../runtime.js";
import type { McpToolDefinition } from "../tool.js";
import { createCoreWorkspaceTools } from "./core-workspaces.js";
import { createEmailCalendarTools } from "./email-calendar.js";
import { createEmailComposeTools } from "./email-compose.js";
import { createEmailArchiveTools } from "./email-archive.js";
import { createEmailAttachmentTools } from "./email-attachments.js";
import { createMediaPersonalTools } from "./media-personal.js";
import { createPrinterTools } from "./printer.js";
import { createBrowserHistoryTools } from "./browser-history.js";
import { createClaudeSessionTools } from "./claude-sessions.js";
import { createRemindersTools } from "./reminders.js";

export function createToolDefinitions(runtime: McpRuntime): McpToolDefinition[] {
  return [
    ...createRemindersTools(runtime),
    ...createCoreWorkspaceTools(runtime),
    ...createEmailCalendarTools(runtime),
    ...createEmailComposeTools(runtime),
    ...createEmailArchiveTools(runtime),
    ...createEmailAttachmentTools(runtime),
    ...createMediaPersonalTools(runtime),
    ...createPrinterTools(runtime),
    ...createBrowserHistoryTools(runtime),
    ...createClaudeSessionTools(runtime),
  ];
}
