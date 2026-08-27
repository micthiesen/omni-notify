import type { McpRuntime } from "../runtime.js";
import type { McpToolDefinition } from "../tool.js";
import { createCoreWorkspaceTools } from "./core-workspaces.js";
import { createEmailCalendarTools } from "./email-calendar.js";
import { createMediaPersonalTools } from "./media-personal.js";

export function createToolDefinitions(runtime: McpRuntime): McpToolDefinition[] {
  return [
    ...createCoreWorkspaceTools(runtime),
    ...createEmailCalendarTools(runtime),
    ...createMediaPersonalTools(runtime),
  ];
}
