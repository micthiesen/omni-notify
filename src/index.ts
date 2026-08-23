import { Injector } from "@micthiesen/mitools/config";
import { Logger } from "@micthiesen/mitools/logging";
import type { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { Scheduler } from "@micthiesen/mitools/scheduling";
import { installAlertThrottle } from "./alerts/throttle.js";
import { BriefingAgentTask } from "./briefing-agent/BriefingAgentTask.js";
import { loadBriefingConfigs } from "./briefing-agent/configs.js";
import { createCalendarHandler } from "./calendar-events/index.js";
import { importHistoricalCosts } from "./costs/migrate.js";
import { EmailDispatcher } from "./email/dispatcher.js";
import { ImapTransport } from "./email/imap/transport.js";
import { JmapTransport } from "./email/jmap/transport.js";
import EmailRetryTask from "./email/retryTask.js";
import { EmailTriageService } from "./email/triage.js";
import type { EmailHandler, EmailTransport } from "./email/types.js";
import EmailWatchdogTask from "./email/watchdogTask.js";
import { ApnsControlClient } from "./ios-controls/apns.js";
import { IOSControlService } from "./ios-controls/service.js";
import { loadChannelsConfig } from "./live-check/channelsConfig.js";
import {
  createLivestreamIntelligenceService,
  type LivestreamIntelligenceObserver,
} from "./live-check/intelligence/service.js";
import { Platform } from "./live-check/platforms/index.js";
import {
  buildStreamers,
  dropPlatformBindings,
  type Streamer,
} from "./live-check/streamers.js";
import LiveCheckTask from "./live-check/task.js";
import { createParcelHandler } from "./parcel-tracker/index.js";
import PetTrackerTask from "./pet-tracker/task.js";
import { CastroInboxCleanupTask } from "./podcast-recs/castro/inboxCleanupTask.js";
import { PodcastTasteReflectionTask } from "./podcast-recs/reflection/index.js";
import { PodcastRecommendationTask } from "./podcast-recs/task.js";
import PressPodsTask from "./press-pods/task.js";
import { MediaRecommendationTask } from "./recommendations/task.js";
import { MediaTasteReflectionTask } from "./recommendations/taste/task.js";
import { type EmailControls, startServer } from "./server.js";
import { taskRunBus } from "./task-runs/events.js";
import { installLogCapture } from "./task-runs/logCapture.js";
import { TaskAlreadyRunningError, TaskRegistry } from "./task-runs/registry.js";
import config from "./utils/config.js";
import { workspaceDefinitions } from "./workspaces/definitions.js";
import { WorkspaceEmailHandler } from "./workspaces/email.js";
import { WorkspaceNotificationTask } from "./workspaces/notifications.js";
import { WorkspaceTask } from "./workspaces/task.js";

Injector.configure({ config });
installLogCapture();
installAlertThrottle();

const logger = new Logger("Main");
const livestreamIntelligence = createLivestreamIntelligenceService(logger);
const importedCostEvents = importHistoricalCosts();
if (importedCostEvents > 0) {
  logger.info(`Imported ${importedCostEvents} historical cost event(s)`);
}

// channels.json is the sole source of channel config now; these env vars are
// silently ignored by utils/config.ts (dropped from its schema), so warn
// instead of leaving a stale, no-longer-honored value unexplained.
const LEGACY_CHANNEL_ENV_VARS = [
  "YT_CHANNEL_NAMES",
  "TWITCH_CHANNEL_NAMES",
  "KICK_CHANNEL_NAMES",
] as const;

function warnOnLegacyChannelEnvVars(): void {
  for (const key of LEGACY_CHANNEL_ENV_VARS) {
    if (process.env[key]) {
      logger.warn(
        `${key} is no longer read — channels are configured in channels.json`,
      );
    }
  }
}

function loadStreamers(): {
  streamers: Streamer[];
  dggTopEmbeds: number;
  availablePlatforms: Set<Platform>;
} {
  warnOnLegacyChannelEnvVars();

  const liveCheckConfig = loadChannelsConfig();
  const streamers = buildStreamers(liveCheckConfig.channels);
  const kickConfigured = Boolean(config.KICK_CLIENT_ID && config.KICK_CLIENT_SECRET);
  const availablePlatforms = new Set(Object.values(Platform));
  if (kickConfigured) {
    return {
      streamers,
      dggTopEmbeds: liveCheckConfig.dggTopEmbeds,
      availablePlatforms,
    };
  }

  const { streamers: withoutKick, droppedAny } = dropPlatformBindings(
    streamers,
    Platform.Kick,
  );
  if (droppedAny) {
    logger.warn(
      "Kick channels configured but KICK_CLIENT_ID/KICK_CLIENT_SECRET missing; skipping Kick",
    );
    // A streamer whose ONLY binding was Kick vanishes from tracking entirely —
    // name those specifically rather than letting them disappear silently.
    const remaining = new Set(withoutKick.map((s) => s.id));
    const fullyDropped = streamers
      .filter((s) => !remaining.has(s.id))
      .map((s) => s.displayName);
    if (fullyDropped.length > 0) {
      logger.warn(`Not tracked at all (Kick-only): ${fullyDropped.join(", ")}`);
    }
  }
  return {
    streamers: withoutKick,
    dggTopEmbeds: liveCheckConfig.dggTopEmbeds,
    availablePlatforms,
  };
}

function buildTasks(
  streamers: Streamer[],
  iosControls?: IOSControlService,
  dggTopEmbeds = 0,
  availablePlatforms: ReadonlySet<Platform> = new Set(Object.values(Platform)),
  intelligence: LivestreamIntelligenceObserver | undefined = livestreamIntelligence,
): ScheduledTask[] {
  const tasks: ScheduledTask[] = [];

  if (streamers.length > 0 || dggTopEmbeds > 0) {
    tasks.push(
      new LiveCheckTask(
        streamers,
        logger,
        () => iosControls?.reconcile() ?? Promise.resolve(),
        dggTopEmbeds > 0 ? { topEmbeds: dggTopEmbeds, availablePlatforms } : undefined,
        intelligence,
      ),
    );
  }
  if (config.WHISKER_CREDENTIALS) {
    tasks.push(new PetTrackerTask(config.WHISKER_CREDENTIALS, logger));
  }

  for (const config of loadBriefingConfigs(logger)) {
    const task = BriefingAgentTask.create(config, logger);
    if (task) tasks.push(task);
  }

  const pressPods = PressPodsTask.create(logger);
  if (pressPods) tasks.push(pressPods);
  const recommendations = MediaRecommendationTask.create(logger);
  if (recommendations) tasks.push(recommendations);
  const podcastRecs = PodcastRecommendationTask.create(logger);
  if (podcastRecs) tasks.push(podcastRecs);
  const castroInboxCleanup = CastroInboxCleanupTask.create(logger);
  if (castroInboxCleanup) tasks.push(castroInboxCleanup);
  const tasteReflection = MediaTasteReflectionTask.create(logger);
  if (tasteReflection) tasks.push(tasteReflection);
  const podcastTasteReflection = PodcastTasteReflectionTask.create(logger);
  if (podcastTasteReflection) tasks.push(podcastTasteReflection);
  for (const definition of workspaceDefinitions) {
    tasks.push(new WorkspaceTask(definition, logger));
  }
  tasks.push(new WorkspaceNotificationTask(logger));

  return tasks;
}

function createIOSControlService(streamers: Streamer[]): IOSControlService {
  const apnsValues = [
    config.IOS_CONTROL_APNS_TEAM_ID,
    config.IOS_CONTROL_APNS_KEY_ID,
    config.IOS_CONTROL_APNS_KEY_PATH,
  ];
  const hasAnyApnsConfig = apnsValues.some(Boolean);
  const hasCompleteApnsConfig = apnsValues.every(Boolean);
  if (hasAnyApnsConfig && !hasCompleteApnsConfig) {
    logger.warn(
      "iOS control APNs pushes disabled: team ID, key ID, and key path must all be set",
    );
  }
  let apns: ApnsControlClient | undefined;
  if (hasCompleteApnsConfig && config.IOS_CONTROL_AUTH_TOKEN) {
    try {
      apns = new ApnsControlClient({
        teamId: config.IOS_CONTROL_APNS_TEAM_ID as string,
        keyId: config.IOS_CONTROL_APNS_KEY_ID as string,
        privateKeyPath: config.IOS_CONTROL_APNS_KEY_PATH as string,
        bundleId: config.IOS_CONTROL_BUNDLE_ID,
      });
    } catch (error) {
      logger.warn(
        `iOS control APNs pushes disabled: failed to load signing key (${error instanceof Error ? error.message : "unknown error"})`,
      );
    }
  }
  if (hasCompleteApnsConfig && !config.IOS_CONTROL_AUTH_TOKEN) {
    logger.warn("iOS control APNs pushes disabled: IOS_CONTROL_AUTH_TOKEN is not set");
  }
  return new IOSControlService(streamers, config.IOS_CONTROL_HOME_URL, logger, apns);
}

// --run-task <name>: run a single task once and exit
const runTaskIndex = process.argv.indexOf("--run-task");
if (runTaskIndex !== -1) {
  const taskName = process.argv[runTaskIndex + 1];
  if (!taskName) {
    logger.error("Usage: --run-task <TaskName>");
    process.exit(1);
  }

  const loadedLiveCheck = loadStreamers();
  const oneOffStreamers = loadedLiveCheck.streamers;
  const oneOffControls = createIOSControlService(oneOffStreamers);
  const tasks = buildTasks(
    oneOffStreamers,
    oneOffControls,
    loadedLiveCheck.dggTopEmbeds,
    loadedLiveCheck.availablePlatforms,
  );
  const task = tasks.find((t) => t.name.toLowerCase() === taskName.toLowerCase());
  if (!task) {
    const names = tasks.map((t) => t.name).join(", ");
    logger.error(`Unknown task "${taskName}". Available: ${names}`);
    process.exit(1);
  }

  logger.info(`Running task "${task.name}" once...`);
  try {
    await task.run();
  } finally {
    await livestreamIntelligence?.close();
    oneOffControls.close();
  }
  logger.info(`Task "${task.name}" complete`);
  process.exit(0);
}

// --server-only: just the HTTP server, no tasks
const serverOnly = process.argv.includes("--server-only");

const registry = new TaskRegistry(logger);
const loadedLiveCheck = loadStreamers();
const streamers = loadedLiveCheck.streamers;
const iosControls = createIOSControlService(streamers);
const pendingWorkspaceEmailRuns = new Map<
  string,
  { workspaceId: string; subjectId: string; message: string }
>();

function requestWorkspaceEmailRun(
  workspaceId: string,
  subjectId: string,
  message: string,
  trigger: "email" = "email",
): void {
  const definition = workspaceDefinitions.find((item) => item.id === workspaceId);
  if (!definition) return;
  const key = `${workspaceId}:${subjectId}`;
  try {
    registry.runNow(definition.taskName, {
      message,
      subjectId,
      trigger,
    });
    pendingWorkspaceEmailRuns.delete(key);
  } catch (error) {
    if (!(error instanceof TaskAlreadyRunningError)) throw error;
    pendingWorkspaceEmailRuns.set(key, { workspaceId, subjectId, message });
    logger.info(
      `Queued a follow-up workspace email run for ${workspaceId}/${subjectId}`,
    );
  }
}

const unsubscribeWorkspaceEmailRuns = taskRunBus.subscribe((event) => {
  if (event.type !== "run-finished") return;
  const definition = workspaceDefinitions.find(
    (item) => item.taskName === event.taskName,
  );
  if (!definition) return;
  setTimeout(() => {
    for (const pending of [...pendingWorkspaceEmailRuns.values()]) {
      if (pending.workspaceId !== definition.id) continue;
      requestWorkspaceEmailRun(pending.workspaceId, pending.subjectId, pending.message);
    }
  }, 50);
});

// Filled in once the email features start; powers the reprocess endpoint.
const emailControls: EmailControls = {};

// Start HTTP server
const closeServer = startServer(
  config.FRONTEND_PORT,
  logger,
  registry,
  streamers,
  emailControls,
  iosControls,
);

let cleanupEmailTransport: (() => void) | undefined;

if (!serverOnly) {
  const scheduler = new Scheduler(logger);
  for (const task of buildTasks(
    streamers,
    iosControls,
    loadedLiveCheck.dggTopEmbeds,
    loadedLiveCheck.availablePlatforms,
  )) {
    scheduler.register(registry.track(task));
  }

  // Email tasks register up-front (Scheduler requires pre-start registration)
  // so a failed transport connect at boot can't silently disable them — the
  // exact outage the watchdog exists to catch. The retry task no-ops until
  // the controls fill in; the connect itself retries in the background.
  if (config.EMAIL_TRANSPORT) {
    scheduler.register(registry.track(new EmailWatchdogTask(logger)));
    scheduler.register(registry.track(new EmailRetryTask(() => emailControls, logger)));
    void startEmailWithRetry(logger);
  } else {
    logger.info(
      "Email features disabled: no transport configured " +
        "(FASTMAIL_API_TOKEN or ICLOUD_USERNAME + ICLOUD_APP_PASSWORD)",
    );
  }

  // Start scheduler (runs opted-in tasks immediately, then all tasks on schedule)
  scheduler.start();
  const recoveryPromise = registry.recoverMissedTasks().catch((error) => {
    logger.error("Failed to recover missed task runs", error);
  });

  // Graceful shutdown handling
  let isShuttingDown = false;

  async function shutdown(signal: string): Promise<void> {
    if (isShuttingDown) return;
    isShuttingDown = true;

    logger.info(`Received ${signal}, shutting down gracefully...`);
    closeServer();
    cleanupEmailTransport?.();
    iosControls.close();
    unsubscribeWorkspaceEmailRuns();
    await scheduler.shutdown();
    await livestreamIntelligence?.close();
    await recoveryPromise;
    logger.info("Shutdown complete");
    process.exit(0);
  }

  process.on("SIGTERM", () => shutdown("SIGTERM"));
  process.on("SIGINT", () => shutdown("SIGINT"));
} else {
  logger.info("Running in server-only mode (tasks disabled)");
}

interface EmailFeatures {
  cleanup: () => void;
  transport: EmailTransport;
  handlers: Map<string, EmailHandler>;
}

/**
 * Containers restart often and mail servers can blip: a one-shot connect at
 * boot would silently disable the whole email system (including the retry
 * drain) until the next restart. Retry forever with capped backoff instead;
 * only the first failure alerts (errors reach Pushover), and the watchdog
 * covers the prolonged-outage case.
 */
async function startEmailWithRetry(parentLogger: Logger): Promise<void> {
  const maxDelayMs = 5 * 60_000;
  for (let attempt = 1; ; attempt++) {
    try {
      const email = await startEmailFeatures(parentLogger);
      if (email) {
        cleanupEmailTransport = email.cleanup;
        emailControls.transport = email.transport;
        emailControls.handlers = email.handlers;
      }
      return;
    } catch (error) {
      const delayMs = Math.min(30_000 * 2 ** (attempt - 1), maxDelayMs);
      const message = `Failed to start email features (attempt ${attempt}), retrying in ${Math.round(delayMs / 1000)}s`;
      if (attempt === 1) {
        parentLogger.error(message, (error as Error).message);
      } else {
        parentLogger.info(`${message}: ${(error as Error).message}`);
      }
      await new Promise((resolve) => setTimeout(resolve, delayMs));
    }
  }
}

async function createEmailTransport(
  logger: Logger,
): Promise<EmailTransport | undefined> {
  switch (config.EMAIL_TRANSPORT) {
    case "fastmail":
      if (!config.FASTMAIL_API_TOKEN) return undefined;
      return JmapTransport.create(config.FASTMAIL_API_TOKEN, logger.extend("JMAP"));
    case "icloud":
      if (!config.ICLOUD_USERNAME || !config.ICLOUD_APP_PASSWORD) return undefined;
      return new ImapTransport(
        { user: config.ICLOUD_USERNAME, pass: config.ICLOUD_APP_PASSWORD },
        logger.extend("IMAP"),
      );
    default:
      return undefined;
  }
}

async function startEmailFeatures(
  parentLogger: Logger,
): Promise<EmailFeatures | undefined> {
  const emailLogger = parentLogger.extend("Email");
  const transport = await createEmailTransport(parentLogger);
  if (!transport) return undefined;

  // Create dispatcher and register handlers; one shared triage service so
  // concurrent pipelines classify each email with a single model call.
  const dispatcher = new EmailDispatcher(transport, emailLogger);
  const triage = new EmailTriageService(emailLogger.extend("Triage"));

  const parcel = createParcelHandler(parentLogger, triage);
  if (parcel) dispatcher.register(parcel);

  const calendar = createCalendarHandler(transport, parentLogger, triage);
  if (calendar) dispatcher.register(calendar);

  const workspaces = new WorkspaceEmailHandler(
    (workspaceId, subjectId, message, trigger) =>
      requestWorkspaceEmailRun(workspaceId, subjectId, message, trigger),
    emailLogger.extend("Workspaces"),
  );
  dispatcher.register(workspaces);

  if (dispatcher.handlerCount === 0) {
    emailLogger.info("No email pipelines active");
    return undefined;
  }

  const handlers = new Map<string, EmailHandler>();
  if (parcel) handlers.set(parcel.name, parcel);
  if (calendar) handlers.set(calendar.name, calendar);
  handlers.set(workspaces.name, workspaces);

  await transport.start(() => dispatcher.onMailEvent());
  emailLogger.info(
    `Started ${transport.name} transport with ${dispatcher.handlerCount} pipeline(s)`,
  );
  return { cleanup: () => transport.stop(), transport, handlers };
}
