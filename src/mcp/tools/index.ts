import type { McpRuntime } from "../runtime.js";
import type { McpToolDefinition } from "../tool.js";
import { createCoreWorkspaceTools } from "./core-workspaces.js";
import { createEmailCalendarTools } from "./email-calendar.js";
import { createEmailComposeTools } from "./email-compose.js";
import { createEmailAttachmentTools } from "./email-attachments.js";
import { createMediaPersonalTools } from "./media-personal.js";
import { createPrinterTools } from "./printer.js";
import { createBrowserHistoryTools } from "./browser-history.js";

export function createToolDefinitions(runtime: McpRuntime): McpToolDefinition[] {
  return [
    ...createCoreWorkspaceTools(runtime),
    ...createEmailCalendarTools(runtime),
    ...createEmailComposeTools(runtime),
    ...createEmailAttachmentTools(runtime),
    ...createMediaPersonalTools(runtime),
    ...createPrinterTools(runtime),
    ...createBrowserHistoryTools(runtime),
  ];
}
