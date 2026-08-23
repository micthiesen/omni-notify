export type LivestreamAlertType =
  | "destiny_guest"
  | "breaking_news"
  | "debate"
  | "guest_joined"
  | "major_announcement"
  | "viewer_surge"
  | "cross_stream_topic";

export type LivestreamFeedbackVerdict = "useful" | "not_useful" | "false_positive";

export interface SemanticMetadata {
  headline: string;
  topics: string[];
  contentKind: "politics" | "debate" | "news" | "gaming" | "conversation" | "other";
  importance: number;
  reason: string;
  updatedAt: number;
}

export interface ViewerTrend {
  percentChange: number;
  viewersPerMinute: number;
  dggPercentChange: number | null;
  anomalous: boolean;
  reason: string | null;
  updatedAt: number;
}

export interface LivestreamChapter {
  chapterId: string;
  startedAt: number;
  title: string;
  summary: string;
}

export interface RollingSummary {
  text: string;
  topic: string;
  confidence: number;
  transcriptExcerpt: string;
  updatedAt: number;
  windowSeconds: number;
}

export interface DestinyPresence {
  state: "possible" | "confirmed";
  confidence: number;
  detectedAt: number;
  reason: string;
}

export interface LivestreamAlertRecord {
  alertId: string;
  type: LivestreamAlertType;
  title: string;
  message: string;
  reason: string;
  confidence: number;
  createdAt: number;
}

export interface LivestreamIntelligenceData {
  streamerId: string;
  sessionStartedAt: number;
  semantic?: SemanticMetadata;
  trend?: ViewerTrend;
  relevanceScore: number;
  relevanceReasons: string[];
  summary?: RollingSummary;
  chapters: LivestreamChapter[];
  destinyPresence?: DestinyPresence;
  latestAlert?: LivestreamAlertRecord;
  updatedAt: number;
}

export interface LivestreamFeedbackData {
  feedbackId: string;
  streamerId: string;
  alertId: string;
  alertType: LivestreamAlertType;
  verdict: LivestreamFeedbackVerdict;
  note?: string;
  createdAt: number;
}
