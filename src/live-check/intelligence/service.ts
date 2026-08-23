import { randomUUID } from "node:crypto";
import type { Logger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import PQueue from "p-queue";
import { recordCostEventSafely } from "../../costs/persistence.js";
import config from "../../utils/config.js";
import type { StreamerStatusLive } from "../persistence.js";
import { getNotificationUrlFields } from "../platforms/index.js";
import type { Streamer } from "../streamers.js";
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
  getAllLivestreamIntelligence,
  getLivestreamDiagnostics,
  getLivestreamIntelligence,
  recordLivestreamEvent,
  saveLivestreamIntelligence,
  updateLivestreamStage,
} from "./persistence.js";
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

const DESTINY_ID = "destiny";
const PRESENCE_EXPIRY_MS = 10 * 60_000;
const ALERT_COOLDOWN_MS = 30 * 60_000;
const CROSS_STREAM_COOLDOWN_MS = 2 * 60 * 60_000;
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

export interface LivestreamIntelligenceObserver {
  observeLive(observation: LiveObservation): void;
  observeOffline(streamerId: string): void;
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

function sameTopic(a: string, b: string): boolean {
  const normalize = (value: string) =>
    value
      .toLowerCase()
      .replace(/[^a-z0-9 ]/g, "")
      .replace(/\s+/g, " ")
      .trim();
  const left = normalize(a);
  const right = normalize(b);
  return (
    left === right ||
    (Math.min(left.length, right.length) >= 12 &&
      (left.includes(right) || right.includes(left)))
  );
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
  private readonly crossTopicAlertTimes = new Map<string, number>();

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
    let state =
      previous?.sessionStartedAt === sessionStartedAt
        ? previous
        : newState(observation, now);
    const trend = this.anomaly.observe({
      streamerId: observation.streamer.id,
      viewers: observation.status.viewerCount ?? null,
      dggViewers: observation.streamer.dgg?.viewers ?? null,
      now,
    });
    const presenceFresh =
      state.destinyPresence &&
      now - state.destinyPresence.detectedAt <= PRESENCE_EXPIRY_MS;
    if (state.destinyPresence && !presenceFresh) {
      state = { ...state, destinyPresence: undefined };
    }
    const relevance = computeRelevance({
      streamer: observation.streamer,
      semantic: state.semantic,
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
        status: state.semantic ? "success" : "idle",
        eligible: true,
        finishedAt: state.semantic?.updatedAt,
        detail: state.semantic?.headline ?? "Waiting for semantic classification",
        metrics: state.semantic
          ? {
              importance: state.semantic.importance,
              topicCount: state.semantic.topics.length,
            }
          : undefined,
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

    if (observation.wentLive || observation.titleChanged || !state.semantic) {
      this.scheduleMetadata(observation);
    }
    if (
      this.isVoiceTarget(observation) &&
      this.voiceDue(observation.streamer.id, now)
    ) {
      this.scheduleVoiceSample(observation);
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
      (state.semantic?.importance ?? 0) >= 65 ||
      state.relevanceScore >= 80
    );
  }

  private voiceDue(streamerId: string, now: number): boolean {
    return (
      !this.pendingVoice.has(streamerId) &&
      now - (this.lastVoiceSample.get(streamerId) ?? 0) >=
        config.LIVESTREAM_VOICE_SAMPLE_INTERVAL_SECONDS * 1000 &&
      this.pendingVoice.size < config.LIVESTREAM_MAX_VOICE_TARGETS
    );
  }

  private summaryDue(streamerId: string, now: number): boolean {
    return (
      !this.pendingSummary.has(streamerId) &&
      now - (this.lastSummary.get(streamerId) ?? 0) >=
        config.LIVESTREAM_SUMMARY_INTERVAL_SECONDS * 1000
    );
  }

  private scheduleMetadata(observation: LiveObservation): void {
    const id = observation.streamer.id;
    const sessionStartedAt = epoch(observation.status.startedAt);
    const startedAt = Date.now();
    const costBefore = livestreamSpendCents(startedAt);
    this.updateStage(id, sessionStartedAt, "metadata", {
      status: "running",
      eligible: true,
      startedAt,
      detail: observation.titleChanged
        ? "Classifying changed title"
        : "Classifying stream title",
    });
    void this.llmQueue
      .add(async () => {
        const semantic = await this.classifier.classifyMetadata({
          displayName: observation.streamer.displayName,
          title: observation.status.primaryTitle,
          category: observation.status.category,
          dggViewers: observation.streamer.dgg?.viewers,
        });
        if (!semantic) {
          const finishedAt = Date.now();
          this.updateStage(id, sessionStartedAt, "metadata", {
            status: "skipped",
            eligible: true,
            startedAt,
            finishedAt,
            durationMs: finishedAt - startedAt,
            detail: "Monthly budget could not cover this classification",
          });
          this.recordEvent({
            streamerId: id,
            sessionStartedAt,
            kind: "metadata",
            status: "warning",
            title: "Metadata classification skipped",
            detail: "Monthly intelligence budget gate",
          });
          return;
        }
        if (!this.active.has(id)) return;
        const current = getLivestreamIntelligence(observation.streamer.id);
        if (
          !current ||
          current.sessionStartedAt !== epoch(observation.status.startedAt)
        )
          return;
        const relevance = computeRelevance({
          streamer: observation.streamer,
          semantic,
          trend: current.trend,
          destinyConfirmed: current.destinyPresence?.state === "confirmed",
        });
        saveLivestreamIntelligence({
          ...current,
          semantic,
          relevanceScore: relevance.score,
          relevanceReasons: relevance.reasons,
          updatedAt: Date.now(),
        });
        const finishedAt = Date.now();
        const costCents = Math.max(0, livestreamSpendCents(finishedAt) - costBefore);
        this.updateStage(id, sessionStartedAt, "metadata", {
          status: "success",
          eligible: true,
          startedAt,
          finishedAt,
          durationMs: finishedAt - startedAt,
          detail: semantic.headline,
          metrics: {
            importance: semantic.importance,
            topicCount: semantic.topics.length,
            costCents,
          },
        });
        this.recordEvent({
          streamerId: id,
          sessionStartedAt,
          kind: "metadata",
          status: "success",
          title: "Semantic metadata updated",
          detail: semantic.headline,
          durationMs: finishedAt - startedAt,
          costCents,
          metrics: {
            importance: semantic.importance,
            contentKind: semantic.contentKind,
          },
        });
        await this.maybeNotifyCrossStreamTopic(semantic.topics);
      })
      .catch((error) => {
        const finishedAt = Date.now();
        const detail = (error as Error).message.slice(0, 500);
        this.updateStage(id, sessionStartedAt, "metadata", {
          status: "error",
          eligible: true,
          startedAt,
          finishedAt,
          durationMs: finishedAt - startedAt,
          detail,
        });
        this.recordEvent({
          streamerId: id,
          sessionStartedAt,
          kind: "metadata",
          status: "error",
          title: "Metadata classification failed",
          detail,
          durationMs: finishedAt - startedAt,
        });
        this.logger.warn(
          `Livestream metadata failed for ${observation.streamer.displayName}: ${detail}`,
        );
      });
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
    const evidence = this.voiceEvidence.observe(id, match.matchedWindows, now);
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
        evidence,
      },
    });
    if (evidence === "none") return;
    if (evidence === "possible") {
      saveLivestreamIntelligence({
        ...current,
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
      title: "Destiny confirmed as a live participant",
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
      lastChapter && sameTopic(lastChapter.title, assessment.topic)
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
    const relevant = buildLivestreamFeedbackDigest(100)
      .split("\n")
      .filter((line) => line.startsWith(`${type}:`));
    if (relevant.length < 2) return 0.75;
    const negative = relevant.filter(
      (line) => line.includes("not_useful") || line.includes("false_positive"),
    ).length;
    return negative / relevant.length >= 0.6 ? 0.9 : 0.75;
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
    const key = `${input.observation.streamer.id}:${input.type}`;
    const previous = this.alertTimes.get(key) ?? 0;
    if (Date.now() - previous < ALERT_COOLDOWN_MS) {
      markSkipped("A matching alert was already sent within the 30-minute cooldown");
      return;
    }
    if (
      input.type === "destiny_guest" &&
      current.latestAlert?.type === "destiny_guest" &&
      current.latestAlert.createdAt >= current.sessionStartedAt
    ) {
      markSkipped("Destiny was already reported during this live session");
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

  private async maybeNotifyCrossStreamTopic(topics: string[]): Promise<void> {
    const now = Date.now();
    for (const topic of topics) {
      const peers = getAllLivestreamIntelligence().filter(
        (item) =>
          this.active.has(item.streamerId) &&
          item.semantic?.importance !== undefined &&
          item.semantic.importance >= 60 &&
          item.semantic.topics.some((candidate) => sameTopic(candidate, topic)),
      );
      if (peers.length < 2) continue;
      const key = topic.toLowerCase();
      if (now - (this.crossTopicAlertTimes.get(key) ?? 0) < CROSS_STREAM_COOLDOWN_MS) {
        continue;
      }
      const first = this.active.get(peers[0]?.streamerId ?? "");
      if (!first) continue;
      this.crossTopicAlertTimes.set(key, now);
      await this.maybeNotify({
        observation: first,
        type: "cross_stream_topic",
        title: `Multiple streams are covering ${topic}`,
        message: peers
          .map((item) => this.active.get(item.streamerId)?.streamer.displayName)
          .filter(Boolean)
          .join(", "),
        reason: `${peers.length} active streams independently converged on this topic`,
        confidence: 0.85,
      });
    }
  }
}

export function createLivestreamIntelligenceService(
  logger: Logger,
): LivestreamIntelligenceService | undefined {
  if (!config.LIVESTREAM_INTELLIGENCE_ENABLED) return undefined;
  return new LivestreamIntelligenceService(logger.extend("LivestreamIntelligence"));
}
