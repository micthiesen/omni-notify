import { randomUUID } from "node:crypto";
import type { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import PQueue from "p-queue";
import { recordCostEventSafely } from "../../costs/persistence.js";
import config from "../../utils/config.js";
import type { StreamerStatusLive } from "../persistence.js";
import { getNotificationUrlFields } from "../platforms/index.js";
import type { Streamer } from "../streamers.js";
import { alertSentInSession, livestreamAlertConfidenceFloor } from "./alertPolicy.js";
import { computeRelevance, ViewerAnomalyTracker } from "./anomaly.js";
import { LivestreamAudioCapture } from "./audio.js";
import {
  isTranscriptAlertType,
  LivestreamClassifier,
  livestreamSpendCents,
  type TranscriptAssessment,
} from "./classifier.js";
import { LocalSpeechRuntime } from "./localSpeech.js";
import {
  buildLivestreamFeedbackDigest,
  DESTINY_CONFIRMED_EVENT_TITLE,
  getLatestDestinyConfirmation,
  getLivestreamDiagnostics,
  getLivestreamIntelligence,
  recordLivestreamEvent,
  saveLivestreamIntelligence,
  updateLivestreamStage,
} from "./persistence.js";
import { decideVoiceMatchAction } from "./presencePolicy.js";
import { areSameLivestreamTopic } from "./summaryText.js";
import type {
  LivestreamAlertRecord,
  LivestreamAlertType,
  LivestreamEventKind,
  LivestreamIntelligenceData,
  LivestreamPipelineStage,
  LivestreamStageDiagnostic,
  RollingSummary,
} from "./types.js";
import { VoiceEvidenceTracker } from "./voiceEvidence.js";
import { selectVoiceTargets } from "./voiceTargets.js";

const DESTINY_ID = "destiny";
const PRESENCE_EXPIRY_MS = 10 * 60_000;
const ALERT_COOLDOWN_MS = 30 * 60_000;
const MAX_CHAPTERS = 40;
const TRANSCRIPT_EXCERPT_CHARS = 800;

export function isDestinyOwnedStream(streamer: Streamer): boolean {
  const identifiers = [
    streamer.id,
    streamer.displayName,
    ...streamer.bindings.map((binding) => binding.username),
  ];
  return identifiers.some(
    (value) => value.trim().toLowerCase().replace(/^@/, "") === DESTINY_ID,
  );
}

export interface LiveObservation {
  streamer: Streamer;
  status: StreamerStatusLive;
  wentLive: boolean;
  titleChanged: boolean;
}

export function viewerCountForAnomaly(status: StreamerStatusLive): number | null {
  if (!status.sources) return status.viewerCount ?? null;
  const primarySource = status.sources.find(
    (source) =>
      source.platform === status.primary.platform &&
      source.username === status.primary.username,
  );
  return primarySource?.viewerCount ?? null;
}

export interface LivestreamIntelligenceObserver {
  observeLive(observation: LiveObservation): void;
  observeOffline(streamerId: string): void;
  afterTick(): void;
  close(): Promise<void>;
}

export interface LivestreamRuntimeDiagnostics {
  enabled: true;
  voiceprintLoaded: boolean;
  model: string;
  queues: {
    capture: { running: number; queued: number };
    speech: { running: number; queued: number };
    llm: { running: number; queued: number };
  };
  activeStreamCount: number;
  activeVoiceTargetCount: number;
  budget: { spentCents: number; limitCents: number; remainingCents: number };
  intervals: { voiceSeconds: number; summarySeconds: number };
}

export interface LivestreamIntelligenceDiagnosticsProvider {
  getRuntimeDiagnostics(): LivestreamRuntimeDiagnostics;
}

function epoch(value: Date | string): number {
  return new Date(value).getTime();
}

function streamUrl(status: StreamerStatusLive): string {
  return (
    status.primary.urlOverride ??
    getNotificationUrlFields(status.primary.platform, status.primary.username).url
  );
}

function newState(
  observation: LiveObservation,
  now: number,
): LivestreamIntelligenceData {
  return {
    streamerId: observation.streamer.id,
    sessionStartedAt: epoch(observation.status.startedAt),
    relevanceScore: observation.streamer.tier === "primary" ? 40 : 15,
    relevanceReasons:
      observation.streamer.tier === "primary" ? ["primary channel"] : [],
    chapters: [],
    updatedAt: now,
  };
}

function concatSamples(...parts: Float32Array[]): Float32Array {
  const total = parts.reduce((sum, part) => sum + part.length, 0);
  const joined = new Float32Array(total);
  let offset = 0;
  for (const part of parts) {
    joined.set(part, offset);
    offset += part.length;
  }
  return joined;
}

export class LivestreamIntelligenceService implements LivestreamIntelligenceObserver {
  private readonly anomaly = new ViewerAnomalyTracker();
  private readonly capture = new LivestreamAudioCapture();
  private readonly speech: LocalSpeechRuntime;
  private readonly classifier: LivestreamClassifier;
  private readonly captureQueue = new PQueue({ concurrency: 2 });
  private readonly speechQueue = new PQueue({ concurrency: 1 });
  private readonly llmQueue = new PQueue({ concurrency: 1 });
  private readonly pendingVoice = new Map<string, number>();
  private readonly pendingSummary = new Map<string, number>();
  private readonly lastVoiceSample = new Map<string, number>();
  private readonly lastSummary = new Map<string, number>();
  private readonly voiceEvidence = new VoiceEvidenceTracker();
  private readonly active = new Map<string, LiveObservation>();
  private readonly alertTimes = new Map<string, number>();

  public constructor(private readonly logger: Logger) {
    this.classifier = new LivestreamClassifier(logger);
    this.speech = new LocalSpeechRuntime(
      config.LIVESTREAM_MODEL_DIR,
      config.LIVESTREAM_DESTINY_VOICEPRINT_PATH,
      config.LIVESTREAM_DESTINY_SPEAKER_THRESHOLD,
    );
    if (!this.speech.hasVoiceprint) {
      this.logger.warn(
        "Livestream intelligence started without a Destiny voiceprint; summaries are enabled but guest detection is disabled",
      );
    }
  }

  public getRuntimeDiagnostics(): LivestreamRuntimeDiagnostics {
    const spentCents = livestreamSpendCents();
    const limitCents = config.LIVESTREAM_MONTHLY_BUDGET_USD * 100;
    return {
      enabled: true,
      voiceprintLoaded: this.speech.hasVoiceprint,
      model: "parakeet-tdt-0.6b-v3-int8",
      queues: {
        capture: { running: this.captureQueue.pending, queued: this.captureQueue.size },
        speech: { running: this.speechQueue.pending, queued: this.speechQueue.size },
        llm: { running: this.llmQueue.pending, queued: this.llmQueue.size },
      },
      activeStreamCount: this.active.size,
      activeVoiceTargetCount: [...this.active.values()].filter((item) =>
        this.isVoiceTarget(item),
      ).length,
      budget: {
        spentCents,
        limitCents,
        remainingCents: Math.max(0, limitCents - spentCents),
      },
      intervals: {
        voiceSeconds: config.LIVESTREAM_VOICE_SAMPLE_INTERVAL_SECONDS,
        summarySeconds: config.LIVESTREAM_SUMMARY_INTERVAL_SECONDS,
      },
    };
  }

  private recordEvent(input: {
    streamerId: string;
    sessionStartedAt?: number;
    kind: LivestreamEventKind;
    status: "info" | "success" | "warning" | "error";
    title: string;
    detail?: string;
    durationMs?: number;
    costCents?: number;
    metrics?: Record<string, number | string | boolean | null>;
  }): void {
    try {
      recordLivestreamEvent(input);
    } catch (error) {
      this.logger.warn(
        `Failed to persist intelligence event: ${(error as Error).message}`,
      );
    }
  }

  private updateStage(
    streamerId: string,
    sessionStartedAt: number,
    stage: LivestreamPipelineStage,
    value: LivestreamStageDiagnostic,
  ): void {
    try {
      updateLivestreamStage(streamerId, sessionStartedAt, stage, value);
    } catch (error) {
      this.logger.warn(
        `Failed to persist ${stage} diagnostics: ${(error as Error).message}`,
      );
    }
  }

  public observeLive(observation: LiveObservation): void {
    const now = Date.now();
    this.active.set(observation.streamer.id, observation);
    const previous = getLivestreamIntelligence(observation.streamer.id);
    const sessionStartedAt = epoch(observation.status.startedAt);
    const isNewSession = previous?.sessionStartedAt !== sessionStartedAt;
    if (isNewSession) this.anomaly.clear(observation.streamer.id);
    let state =
      previous?.sessionStartedAt === sessionStartedAt
        ? previous
        : newState(observation, now);
    const trend = this.anomaly.observe({
      streamerId: observation.streamer.id,
      viewers: viewerCountForAnomaly(observation.status),
      dggViewers: observation.streamer.dgg?.viewers ?? null,
      sessionStartedAt,
      now,
    });
    const presenceFresh =
      state.destinyPresence &&
      now - state.destinyPresence.detectedAt <= PRESENCE_EXPIRY_MS;
    if (state.destinyPresence && !presenceFresh) {
      state = { ...state, destinyPresence: undefined };
    }
    // Title-only model classification mostly paraphrased the visible title.
    // Keep intelligence grounded in measured audience data and captured audio.
    state = { ...state, semantic: undefined };
    const relevance = computeRelevance({
      streamer: observation.streamer,
      trend,
      destinyConfirmed: state.destinyPresence?.state === "confirmed",
    });
    state = {
      ...state,
      trend,
      relevanceScore: relevance.score,
      relevanceReasons: relevance.reasons,
      updatedAt: now,
    };
    saveLivestreamIntelligence(state);

    const previousDiagnostics = getLivestreamDiagnostics(observation.streamer.id);
    if (isNewSession) {
      this.recordEvent({
        streamerId: observation.streamer.id,
        sessionStartedAt,
        kind: "session",
        status: "info",
        title: "Live session started",
        detail: observation.status.primaryTitle,
      });
    }
    if (isNewSession || previousDiagnostics?.sessionStartedAt !== sessionStartedAt) {
      this.updateStage(observation.streamer.id, sessionStartedAt, "metadata", {
        status: "skipped",
        eligible: false,
        finishedAt: now,
        detail: "Title-only LLM classification is disabled",
      });
      this.updateStage(observation.streamer.id, sessionStartedAt, "voice", {
        status: "idle",
        eligible: this.isVoiceTarget(observation),
        detail: this.isVoiceTarget(observation)
          ? "Eligible DGG third-party stream"
          : isDestinyOwnedStream(observation.streamer)
            ? "Destiny's own stream is always excluded"
            : "Not a DGG third-party voice target",
      });
      this.updateStage(observation.streamer.id, sessionStartedAt, "summary", {
        status: state.summary ? "success" : "idle",
        eligible: this.isSummaryTarget(observation, state),
        finishedAt: state.summary?.updatedAt,
        detail:
          state.summary?.topic ??
          (this.isSummaryTarget(observation, state)
            ? "Eligible for rolling summaries"
            : "Waiting for relevance, importance, or anomaly gate"),
        metrics: state.summary
          ? {
              confidence: state.summary.confidence,
              audioSeconds: state.summary.windowSeconds,
            }
          : undefined,
      });
      if (state.latestAlert) {
        this.updateStage(observation.streamer.id, sessionStartedAt, "alert", {
          status: "success",
          eligible: true,
          finishedAt: state.latestAlert.createdAt,
          detail: state.latestAlert.title,
          metrics: {
            type: state.latestAlert.type,
            confidence: state.latestAlert.confidence,
          },
        });
      }
    }

    if (trend.anomalous && relevance.score >= 70) {
      if (!previous?.trend?.anomalous) {
        this.recordEvent({
          streamerId: observation.streamer.id,
          sessionStartedAt,
          kind: "anomaly",
          status: "warning",
          title: "Viewer surge detected",
          detail: trend.reason ?? undefined,
          metrics: {
            percentChange: trend.percentChange,
            viewersPerMinute: trend.viewersPerMinute,
            dggPercentChange: trend.dggPercentChange,
          },
        });
      }
      void this.maybeNotify({
        observation,
        type: "viewer_surge",
        title: `${observation.streamer.displayName} is surging`,
        message: trend.reason ?? "Viewer activity rose unusually quickly.",
        reason: trend.reason ?? "Unusual viewer acceleration",
        confidence: 0.85,
      }).catch((error) =>
        this.logger.warn(
          `Viewer surge notification failed for ${observation.streamer.displayName}: ${(error as Error).message}`,
        ),
      );
    }

    if (
      this.isSummaryTarget(observation, state) &&
      this.summaryDue(observation.streamer.id, now)
    ) {
      this.scheduleSummary(observation);
    }
  }

  public observeOffline(streamerId: string): void {
    const current = getLivestreamIntelligence(streamerId);
    const diagnostics = getLivestreamDiagnostics(streamerId);
    const finishedAt = Date.now();
    if (this.active.has(streamerId) && current) {
      this.recordEvent({
        streamerId,
        sessionStartedAt: current.sessionStartedAt,
        kind: "session",
        status: "info",
        title: "Live session ended",
      });
      for (const [stage, value] of Object.entries(diagnostics?.stages ?? {})) {
        if (value?.status !== "running") continue;
        this.updateStage(
          streamerId,
          current.sessionStartedAt,
          stage as LivestreamPipelineStage,
          {
            ...value,
            status: "skipped",
            finishedAt,
            durationMs: value.startedAt ? finishedAt - value.startedAt : undefined,
            detail: "Stream ended before this operation completed",
          },
        );
      }
    }
    this.active.delete(streamerId);
    this.anomaly.clear(streamerId);
    this.voiceEvidence.clear(streamerId);
    this.pendingVoice.delete(streamerId);
    this.pendingSummary.delete(streamerId);
    this.lastVoiceSample.delete(streamerId);
    this.lastSummary.delete(streamerId);
  }

  public afterTick(): void {
    const now = Date.now();
    const targets = selectVoiceTargets(
      [...this.active.values()].filter((observation) =>
        this.isVoiceTarget(observation),
      ),
      config.LIVESTREAM_MAX_VOICE_TARGETS,
    );
    for (const observation of targets) {
      if (this.voiceDue(observation.streamer.id, now)) {
        this.scheduleVoiceSample(observation);
      }
    }
  }

  public async close(): Promise<void> {
    await Promise.all([
      this.captureQueue.onIdle(),
      this.speechQueue.onIdle(),
      this.llmQueue.onIdle(),
    ]);
  }

  private isVoiceTarget(observation: LiveObservation): boolean {
    return (
      this.speech.hasVoiceprint &&
      !isDestinyOwnedStream(observation.streamer) &&
      observation.streamer.dgg !== undefined
    );
  }

  private currentSession(
    observation: LiveObservation,
  ): LivestreamIntelligenceData | undefined {
    if (!this.active.has(observation.streamer.id)) return undefined;
    const current = getLivestreamIntelligence(observation.streamer.id);
    return current?.sessionStartedAt === epoch(observation.status.startedAt)
      ? current
      : undefined;
  }

  private isSummaryTarget(
    observation: LiveObservation,
    state: LivestreamIntelligenceData,
  ): boolean {
    return (
      observation.streamer.tier === "primary" ||
      state.destinyPresence?.state === "confirmed" ||
      state.trend?.anomalous === true ||
      state.relevanceScore >= 80
    );
  }

  private voiceDue(streamerId: string, now: number): boolean {
    return (
      !this.pendingVoice.has(streamerId) &&
      now - (this.lastVoiceSample.get(streamerId) ?? 0) >=
        config.LIVESTREAM_VOICE_SAMPLE_INTERVAL_SECONDS * 1000
    );
  }

  private summaryDue(streamerId: string, now: number): boolean {
    return (
      !this.pendingSummary.has(streamerId) &&
      now - (this.lastSummary.get(streamerId) ?? 0) >=
        config.LIVESTREAM_SUMMARY_INTERVAL_SECONDS * 1000
    );
  }

  private scheduleVoiceSample(observation: LiveObservation): void {
    const id = observation.streamer.id;
    const sessionStartedAt = epoch(observation.status.startedAt);
    const startedAt = Date.now();
    this.pendingVoice.set(id, sessionStartedAt);
    this.lastVoiceSample.set(id, startedAt);
    this.updateStage(id, sessionStartedAt, "voice", {
      status: "running",
      eligible: true,
      startedAt,
      detail: "Capturing a bounded voice sample",
    });
    void this.captureQueue
      .add(async () => {
        const audio = await this.capture.capture(
          streamUrl(observation.status),
          config.LIVESTREAM_VOICE_SAMPLE_SECONDS,
        );
        await this.speechQueue.add(async () => {
          const match = this.speech.detectDestiny(audio.samples);
          await this.handleVoiceMatch(observation, audio.samples, match, startedAt);
        });
      })
      .catch((error) => {
        const finishedAt = Date.now();
        const detail = (error as Error).message.slice(0, 500);
        this.updateStage(id, sessionStartedAt, "voice", {
          status: "error",
          eligible: true,
          startedAt,
          finishedAt,
          nextAt: startedAt + config.LIVESTREAM_VOICE_SAMPLE_INTERVAL_SECONDS * 1000,
          durationMs: finishedAt - startedAt,
          detail,
        });
        this.recordEvent({
          streamerId: id,
          sessionStartedAt,
          kind: "voice",
          status: "error",
          title: "Voice scan failed",
          detail,
          durationMs: finishedAt - startedAt,
        });
        this.logger.warn(
          `Voice sampling failed for ${observation.streamer.displayName}: ${detail}`,
        );
      })
      .finally(() => {
        if (this.pendingVoice.get(id) === sessionStartedAt) {
          this.pendingVoice.delete(id);
        }
      });
  }

  private async handleVoiceMatch(
    observation: LiveObservation,
    samples: Float32Array,
    match: { confidence: number; matchedWindows: number; checkedWindows: number },
    startedAt: number,
  ): Promise<void> {
    const id = observation.streamer.id;
    const now = Date.now();
    const current = this.currentSession(observation);
    if (!current) return;
    const evidence = this.voiceEvidence.observe(
      id,
      match.matchedWindows,
      match.checkedWindows,
      now,
    );
    const priorConfirmation = getLatestDestinyConfirmation(
      id,
      current.sessionStartedAt,
    );
    const confirmedPresence =
      current.destinyPresence?.state === "confirmed"
        ? current.destinyPresence
        : priorConfirmation
          ? {
              state: "confirmed" as const,
              confidence: Math.min(
                Number(priorConfirmation.metrics?.speakerConfidence ?? 0),
                Number(priorConfirmation.metrics?.assessmentConfidence ?? 0),
              ),
              detectedAt: priorConfirmation.createdAt,
              reason:
                priorConfirmation.detail ?? "Live conversation previously confirmed",
            }
          : current.destinyPresence;
    const action = decideVoiceMatchAction(evidence, confirmedPresence);
    const voiceDetail = `${match.matchedWindows}/${match.checkedWindows} windows matched at ${Math.round(match.confidence * 100)}% confidence`;
    this.updateStage(id, current.sessionStartedAt, "voice", {
      status: "success",
      eligible: true,
      startedAt,
      finishedAt: now,
      nextAt: startedAt + config.LIVESTREAM_VOICE_SAMPLE_INTERVAL_SECONDS * 1000,
      durationMs: now - startedAt,
      detail: evidence === "none" ? `No Destiny evidence; ${voiceDetail}` : voiceDetail,
      metrics: {
        confidence: match.confidence,
        matchedWindows: match.matchedWindows,
        checkedWindows: match.checkedWindows,
        evidence: action === "retain_confirmed" ? "confirmed" : evidence,
      },
    });
    if (action === "ignore") return;
    if (action === "retain_confirmed" && confirmedPresence) {
      const presence = {
        ...confirmedPresence,
        detectedAt: now,
      };
      const latest = this.currentSession(observation);
      if (!latest) return;
      saveLivestreamIntelligence({
        ...latest,
        destinyPresence: presence,
        updatedAt: now,
      });
      if (!alertSentInSession(latest, "destiny_guest")) {
        await this.maybeNotify({
          observation,
          type: "destiny_guest",
          title: `Destiny is on ${observation.streamer.displayName}`,
          message: presence.reason,
          reason: "Previously confirmed live participation plus fresh voice evidence",
          confidence: presence.confidence,
        });
      }
      return;
    }
    if (action === "record_possible") {
      const latest = this.currentSession(observation);
      if (!latest) return;
      saveLivestreamIntelligence({
        ...latest,
        destinyPresence: {
          state: "possible",
          confidence: match.confidence,
          detectedAt: now,
          reason: `${match.matchedWindows}/${match.checkedWindows} voice windows matched; awaiting confirmation`,
        },
        updatedAt: now,
      });
      this.recordEvent({
        streamerId: id,
        sessionStartedAt: current.sessionStartedAt,
        kind: "voice",
        status: "warning",
        title: "Possible Destiny voice match",
        detail: `${voiceDetail}; awaiting another independent sample`,
        durationMs: now - startedAt,
        metrics: {
          confidence: match.confidence,
          matchedWindows: match.matchedWindows,
          checkedWindows: match.checkedWindows,
        },
      });
      return;
    }

    const context = await this.capture.capture(streamUrl(observation.status), 45);
    const combined = concatSamples(samples, context.samples);
    const transcript = await this.speech.transcribe(combined);
    this.recordLocalTranscription("verify-destiny", combined.length / 16_000);
    if (transcript.length < 30) {
      this.recordEvent({
        streamerId: id,
        sessionStartedAt: current.sessionStartedAt,
        kind: "voice",
        status: "warning",
        title: "Voice confirmation skipped",
        detail: "The confirmation window did not contain enough transcribed speech",
      });
      return;
    }
    const assessment = await this.llmQueue.add(() =>
      this.classifier.assessTranscript({
        displayName: observation.streamer.displayName,
        title: observation.status.primaryTitle,
        transcript,
        previousSummary: current.summary?.text,
        previousTopic: current.summary?.topic,
        viewerAnomaly: current.trend?.reason,
        speakerMatchConfidence: match.confidence,
        testingDestinyPresence: true,
      }),
    );
    if (!assessment?.destinyIsLiveParticipant || assessment.confidence < 0.65) {
      this.recordEvent({
        streamerId: id,
        sessionStartedAt: current.sessionStartedAt,
        kind: "voice",
        status: "info",
        title: "Destiny live participation not confirmed",
        detail:
          assessment?.summary ?? "Transcript assessment skipped by the budget gate",
        metrics: { assessmentConfidence: assessment?.confidence ?? null },
      });
      return;
    }
    const latest = this.currentSession(observation);
    if (!latest) return;
    saveLivestreamIntelligence({
      ...latest,
      destinyPresence: {
        state: "confirmed",
        confidence: Math.min(match.confidence, assessment.confidence),
        detectedAt: Date.now(),
        reason: assessment.summary,
      },
      updatedAt: Date.now(),
    });
    this.recordEvent({
      streamerId: id,
      sessionStartedAt: latest.sessionStartedAt,
      kind: "voice",
      status: "success",
      title: DESTINY_CONFIRMED_EVENT_TITLE,
      detail: assessment.summary,
      metrics: {
        speakerConfidence: match.confidence,
        assessmentConfidence: assessment.confidence,
      },
    });
    await this.maybeNotify({
      observation,
      type: "destiny_guest",
      title: `Destiny is on ${observation.streamer.displayName}`,
      message: assessment.summary,
      reason: `${match.matchedWindows} repeated voice matches plus live-conversation context`,
      confidence: Math.min(match.confidence, assessment.confidence),
    });
  }

  private scheduleSummary(observation: LiveObservation): void {
    const id = observation.streamer.id;
    const sessionStartedAt = epoch(observation.status.startedAt);
    const startedAt = Date.now();
    const costBefore = livestreamSpendCents(startedAt);
    this.pendingSummary.set(id, sessionStartedAt);
    this.lastSummary.set(id, startedAt);
    this.updateStage(id, sessionStartedAt, "summary", {
      status: "running",
      eligible: true,
      startedAt,
      detail: "Capturing and transcribing the current window",
    });
    void this.captureQueue
      .add(async () => {
        const audio = await this.capture.capture(
          streamUrl(observation.status),
          config.LIVESTREAM_SUMMARY_SAMPLE_SECONDS,
        );
        const transcript = await this.speechQueue.add(() =>
          this.speech.transcribe(audio.samples),
        );
        this.recordLocalTranscription("rolling-summary", audio.durationSeconds);
        if (!transcript || transcript.length < 30) {
          const finishedAt = Date.now();
          this.updateStage(id, sessionStartedAt, "summary", {
            status: "skipped",
            eligible: true,
            startedAt,
            finishedAt,
            nextAt: startedAt + config.LIVESTREAM_SUMMARY_INTERVAL_SECONDS * 1000,
            durationMs: finishedAt - startedAt,
            detail: "Not enough speech in the captured window",
          });
          this.recordEvent({
            streamerId: id,
            sessionStartedAt,
            kind: "summary",
            status: "info",
            title: "Summary window skipped",
            detail: "Not enough transcribed speech",
            durationMs: finishedAt - startedAt,
          });
          return;
        }
        const current = this.currentSession(observation);
        if (!current) return;
        const assessment = await this.llmQueue.add(() =>
          this.classifier.assessTranscript({
            displayName: observation.streamer.displayName,
            title: observation.status.primaryTitle,
            transcript,
            previousSummary: current.summary?.text,
            previousTopic: current.summary?.topic,
            viewerAnomaly: current.trend?.reason,
            testingDestinyPresence: false,
          }),
        );
        if (!assessment) {
          const finishedAt = Date.now();
          this.updateStage(id, sessionStartedAt, "summary", {
            status: "skipped",
            eligible: true,
            startedAt,
            finishedAt,
            nextAt: startedAt + config.LIVESTREAM_SUMMARY_INTERVAL_SECONDS * 1000,
            durationMs: finishedAt - startedAt,
            detail: "Monthly budget could not cover transcript assessment",
          });
          this.recordEvent({
            streamerId: id,
            sessionStartedAt,
            kind: "summary",
            status: "warning",
            title: "Summary assessment skipped",
            detail: "Monthly intelligence budget gate",
          });
          return;
        }
        await this.saveAssessment(
          observation,
          transcript,
          audio.durationSeconds,
          assessment,
        );
        const finishedAt = Date.now();
        const costCents = Math.max(0, livestreamSpendCents(finishedAt) - costBefore);
        this.updateStage(id, sessionStartedAt, "summary", {
          status: "success",
          eligible: true,
          startedAt,
          finishedAt,
          nextAt: startedAt + config.LIVESTREAM_SUMMARY_INTERVAL_SECONDS * 1000,
          durationMs: finishedAt - startedAt,
          detail: assessment.topic,
          metrics: {
            confidence: assessment.confidence,
            audioSeconds: Math.round(audio.durationSeconds),
            transcriptCharacters: transcript.length,
            costCents,
          },
        });
        this.recordEvent({
          streamerId: id,
          sessionStartedAt,
          kind: "summary",
          status: "success",
          title: "Now summary updated",
          detail: assessment.topic,
          durationMs: finishedAt - startedAt,
          costCents,
          metrics: {
            confidence: assessment.confidence,
            audioSeconds: Math.round(audio.durationSeconds),
          },
        });
      })
      .catch((error) => {
        const finishedAt = Date.now();
        const detail = (error as Error).message.slice(0, 500);
        this.updateStage(id, sessionStartedAt, "summary", {
          status: "error",
          eligible: true,
          startedAt,
          finishedAt,
          nextAt: startedAt + config.LIVESTREAM_SUMMARY_INTERVAL_SECONDS * 1000,
          durationMs: finishedAt - startedAt,
          detail,
        });
        this.recordEvent({
          streamerId: id,
          sessionStartedAt,
          kind: "summary",
          status: "error",
          title: "Rolling summary failed",
          detail,
          durationMs: finishedAt - startedAt,
        });
        this.logger.warn(
          `Rolling summary failed for ${observation.streamer.displayName}: ${detail}`,
        );
      })
      .finally(() => {
        if (this.pendingSummary.get(id) === sessionStartedAt) {
          this.pendingSummary.delete(id);
        }
      });
  }

  private async saveAssessment(
    observation: LiveObservation,
    transcript: string,
    durationSeconds: number,
    assessment: TranscriptAssessment,
  ): Promise<void> {
    const current = this.currentSession(observation);
    if (!current) return;
    const now = Date.now();
    const summary: RollingSummary = {
      text: assessment.summary,
      topic: assessment.topic,
      confidence: assessment.confidence,
      transcriptExcerpt: transcript.slice(-TRANSCRIPT_EXCERPT_CHARS),
      updatedAt: now,
      windowSeconds: Math.round(durationSeconds),
    };
    const lastChapter = current.chapters.at(-1);
    const chapters =
      lastChapter && areSameLivestreamTopic(lastChapter.title, assessment.topic)
        ? [
            ...current.chapters.slice(0, -1),
            { ...lastChapter, summary: assessment.summary },
          ]
        : [
            ...current.chapters,
            {
              chapterId: randomUUID(),
              startedAt: now - durationSeconds * 1000,
              title: assessment.topic,
              summary: assessment.summary,
            },
          ].slice(-MAX_CHAPTERS);
    saveLivestreamIntelligence({ ...current, summary, chapters, updatedAt: now });
    if (
      isTranscriptAlertType(assessment.alertType) &&
      assessment.alertReason &&
      assessment.importance >= 80 &&
      assessment.confidence >= 0.75
    ) {
      await this.maybeNotify({
        observation,
        type: assessment.alertType,
        title: `${observation.streamer.displayName}: ${assessment.topic}`,
        message: assessment.summary,
        reason: assessment.alertReason,
        confidence: assessment.confidence,
      }).catch((error) =>
        this.logger.warn(
          `Semantic alert failed for ${observation.streamer.displayName}: ${(error as Error).message}`,
        ),
      );
    }
  }

  private recordLocalTranscription(operation: string, seconds: number): void {
    recordCostEventSafely({
      category: "transcription",
      feature: "livestream-intelligence",
      operation,
      service: "self-hosted",
      model: "parakeet-tdt-0.6b-v3-int8",
      costCents: 0,
      priceStatus: "free",
      usage: { requests: 1, characters: Math.round(seconds) },
    });
  }

  private feedbackConfidenceFloor(type: LivestreamAlertType): number {
    return livestreamAlertConfidenceFloor({
      type,
      feedbackDigest: buildLivestreamFeedbackDigest(100),
      destinySpeakerThreshold: config.LIVESTREAM_DESTINY_SPEAKER_THRESHOLD,
    });
  }

  private async maybeNotify(input: {
    observation: LiveObservation;
    type: LivestreamAlertType;
    title: string;
    message: string;
    reason: string;
    confidence: number;
  }): Promise<void> {
    const current = this.currentSession(input.observation);
    if (!current) return;
    const markSkipped = (detail: string) =>
      this.updateStage(
        input.observation.streamer.id,
        current.sessionStartedAt,
        "alert",
        {
          status: "skipped",
          eligible: true,
          finishedAt: Date.now(),
          detail,
          metrics: { type: input.type, confidence: input.confidence },
        },
      );
    const confidenceFloor = this.feedbackConfidenceFloor(input.type);
    if (input.confidence < confidenceFloor) {
      markSkipped(
        `${Math.round(input.confidence * 100)}% confidence was below the ${Math.round(confidenceFloor * 100)}% alert threshold`,
      );
      return;
    }
    const key = `${input.observation.streamer.id}:${current.sessionStartedAt}:${input.type}`;
    const previous = this.alertTimes.get(key) ?? 0;
    if (Date.now() - previous < ALERT_COOLDOWN_MS) {
      markSkipped("A matching alert was already sent within the 30-minute cooldown");
      return;
    }
    if (
      (input.type === "destiny_guest" || input.type === "viewer_surge") &&
      alertSentInSession(current, input.type)
    ) {
      markSkipped(
        input.type === "destiny_guest"
          ? "Destiny was already reported during this live session"
          : "A viewer surge was already reported during this live session",
      );
      return;
    }
    const alert: LivestreamAlertRecord = {
      alertId: randomUUID(),
      type: input.type,
      title: input.title,
      message: input.message,
      reason: input.reason,
      confidence: input.confidence,
      createdAt: Date.now(),
    };
    this.alertTimes.set(key, alert.createdAt);
    const alertStartedAt = Date.now();
    this.updateStage(input.observation.streamer.id, current.sessionStartedAt, "alert", {
      status: "running",
      eligible: true,
      startedAt: alertStartedAt,
      detail: alert.title,
    });
    try {
      await notify({
        title: alert.title,
        message: `${alert.message}\n\nWhy: ${alert.reason}`,
        token: config.PUSHOVER_LIVE_TOKEN,
        ...getNotificationUrlFields(
          input.observation.status.primary.platform,
          input.observation.status.primary.username,
          input.observation.status.primary.urlOverride,
        ),
      });
    } catch (error) {
      if (this.alertTimes.get(key) === alert.createdAt) this.alertTimes.delete(key);
      const finishedAt = Date.now();
      const detail = (error as Error).message.slice(0, 500);
      this.updateStage(
        input.observation.streamer.id,
        current.sessionStartedAt,
        "alert",
        {
          status: "error",
          eligible: true,
          startedAt: alertStartedAt,
          finishedAt,
          durationMs: finishedAt - alertStartedAt,
          detail,
        },
      );
      this.recordEvent({
        streamerId: input.observation.streamer.id,
        sessionStartedAt: current.sessionStartedAt,
        kind: "alert",
        status: "error",
        title: "Alert delivery failed",
        detail,
      });
      throw error;
    }
    const latest = this.currentSession(input.observation);
    if (!latest) return;
    saveLivestreamIntelligence({
      ...latest,
      latestAlert: alert,
      alertedAtByType: {
        ...latest.alertedAtByType,
        [alert.type]: alert.createdAt,
      },
      updatedAt: Date.now(),
    });
    const finishedAt = Date.now();
    this.updateStage(input.observation.streamer.id, latest.sessionStartedAt, "alert", {
      status: "success",
      eligible: true,
      startedAt: alertStartedAt,
      finishedAt,
      durationMs: finishedAt - alertStartedAt,
      detail: alert.title,
      metrics: { type: alert.type, confidence: alert.confidence },
    });
    this.recordEvent({
      streamerId: input.observation.streamer.id,
      sessionStartedAt: latest.sessionStartedAt,
      kind: "alert",
      status: "success",
      title: `Alert sent: ${alert.title}`,
      detail: alert.reason,
      durationMs: finishedAt - alertStartedAt,
      metrics: { type: alert.type, confidence: alert.confidence },
    });
  }
}

export function createLivestreamIntelligenceService(
  logger: Logger,
): LivestreamIntelligenceService | undefined {
  if (!config.LIVESTREAM_INTELLIGENCE_ENABLED) return undefined;
  return new LivestreamIntelligenceService(logger.extend("LivestreamIntelligence"));
}
