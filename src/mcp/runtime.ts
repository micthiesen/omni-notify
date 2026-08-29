import type { Logger } from "@micthiesen/mitools/logging";
import type { IOSControlService } from "../ios-controls/service.js";
import type { LivestreamIntelligenceDiagnosticsProvider } from "../live-check/intelligence/service.js";
import type { Streamer } from "../live-check/streamers.js";
import type { PodcastAccountClient } from "../podcast-recs/account.js";
import type { PrinterService } from "../printer/service.js";
import type { EmailControls } from "../server.js";
import type { TaskRegistry } from "../task-runs/registry.js";

export interface McpRuntime {
  logger: Logger;
  registry: TaskRegistry;
  streamers: Streamer[];
  emailControls: EmailControls;
  podcastAccount?: PodcastAccountClient;
  iosControls?: IOSControlService;
  livestreamDiagnostics?: LivestreamIntelligenceDiagnosticsProvider;
  printer?: PrinterService;
}
