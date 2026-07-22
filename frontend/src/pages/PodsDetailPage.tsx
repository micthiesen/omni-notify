import { useEffect, useState } from "react";
import {
  deletePressPodsEpisode,
  fetchPressPodsEpisode,
  retryPressPodsEpisode,
} from "../api";
import type {
  PressPodsChunkStat,
  PressPodsEpisodeDetail,
  PressPodsRetrieverAttempt,
} from "../api";
import { ImageWithFallback } from "../components/ImageWithFallback";
import { Toast, useToast } from "../components/Toast";
import { formatAudioDuration } from "./PodsPage";
import { Link, navigate } from "../router";
import { formatAbsoluteWithYear, formatCents } from "../utils/format";

// Higgs duration-band bounds (the fallback verifier in synthesize.ts), scaled
// for the +10% playback speed-up applied in prepareChunk. Chunks outside this
// range were truncated or ran away. STT `coverage` (below) is the authoritative
// signal when present; duration is only meaningful without it.
const SEC_PER_CHAR_MIN = 0.03 / 1.1;
const SEC_PER_CHAR_MAX = 0.15 / 1.1;
// Below this coverage a chunk is missing too much of its text, above this word
// ratio it ran away (a loop). Mirror DEFAULT_CONTENT_BOUNDS in coverage.ts —
// the backend's accept test rejects on either, so the UI flag must too.
const MIN_COVERAGE = 0.75;
const MIN_COVERAGE_WITH_HEALTHY_RATIO = 0.68;
const MIN_HEALTHY_WORD_RATIO = 0.78;
const MAX_HEALTHY_RATIO_EXPECTED_WORDS = 60;
const MAX_WORD_RATIO = 1.8;

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex++;
  }
  return `${value.toFixed(1)} ${units[unitIndex]}`;
}

function DetailField({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="detail-field">
      <dt>{label}</dt>
      <dd>{children}</dd>
    </div>
  );
}

function isChunkProblematic(chunk: PressPodsChunkStat): boolean {
  // Coverage is authoritative when recorded; a chunk that shipped its best but
  // still-failing take is flagged on either bound the backend rejects on —
  // low coverage (truncation) or high word ratio (runaway loop).
  if (chunk.coverage != null) {
    if (chunk.wordRatio != null && chunk.wordRatio > MAX_WORD_RATIO) return true;
    const secondaryPass =
      chunk.coverage >= MIN_COVERAGE_WITH_HEALTHY_RATIO &&
      chunk.wordRatio != null &&
      chunk.wordRatio >= MIN_HEALTHY_WORD_RATIO &&
      // Older rows do not store expectedWords. charCount is not a safe proxy,
      // so only use the relaxed UI verdict when the persisted stat proves it.
      chunk.expectedWords != null &&
      chunk.expectedWords <= MAX_HEALTHY_RATIO_EXPECTED_WORDS;
    return chunk.coverage < MIN_COVERAGE && !secondaryPass;
  }
  return (
    chunk.attempts > 1 ||
    chunk.secPerChar < SEC_PER_CHAR_MIN ||
    chunk.secPerChar > SEC_PER_CHAR_MAX
  );
}

function RetrieverRow({
  attempt,
  isWinner,
}: {
  attempt: PressPodsRetrieverAttempt;
  isWinner: boolean;
}) {
  return (
    <div
      className={`pods-retriever-row ${attempt.success ? "" : "pods-retriever-row-failed"} ${
        isWinner ? "pods-retriever-row-winner" : ""
      }`}
    >
      <span className="pods-retriever-name">
        {attempt.name}
        {isWinner && <span className="pods-retriever-winner-badge">Winner</span>}
      </span>
      {attempt.success ? (
        <span className="meta-row pods-retriever-meta">
          <span>Rating {attempt.contentRating}/10</span>
          <span>{attempt.textChars.toLocaleString()} chars</span>
        </span>
      ) : (
        <span className="pods-retriever-error">{attempt.error}</span>
      )}
    </div>
  );
}

function ChunkCard({ chunk }: { chunk: PressPodsChunkStat }) {
  const [expanded, setExpanded] = useState(false);
  const problematic = isChunkProblematic(chunk);
  const preview =
    chunk.text.length > 240 && !expanded ? `${chunk.text.slice(0, 240)}…` : chunk.text;

  return (
    <div className={`pods-chunk-card ${problematic ? "pods-chunk-warn" : ""}`}>
      <div className="pods-chunk-header">
        <span className="pods-chunk-index">#{chunk.index}</span>
        {chunk.sectionTitle && (
          <span className="pods-chunk-section">{chunk.sectionTitle}</span>
        )}
        {chunk.resplit && (
          <span
            className="pods-chunk-resplit-badge"
            title="A larger chunk kept failing verification and was re-split into smaller pieces to recover"
          >
            Re-split{chunk.resplitDepth && chunk.resplitDepth > 1
              ? ` ×${chunk.resplitDepth}`
              : ""}
          </span>
        )}
        {problematic && <span className="pods-chunk-warn-badge">Needs Review</span>}
      </div>
      <div className="meta-row pods-chunk-meta">
        <span>{chunk.charCount.toLocaleString()} chars</span>
        <span>{chunk.durationSeconds.toFixed(1)}s</span>
        <span>{chunk.secPerChar.toFixed(3)} s/char</span>
        <span>
          {chunk.attempts} attempt{chunk.attempts === 1 ? "" : "s"}
        </span>
        {chunk.coverage != null && (
          <span>{Math.round(chunk.coverage * 100)}% coverage</span>
        )}
        <span>starts at {formatAudioDuration(chunk.startTimeSeconds)}</span>
      </div>
      <p className="pods-chunk-text">{preview}</p>
      {chunk.text.length > 240 && (
        <button
          type="button"
          className="pods-chunk-text-toggle"
          onClick={() => setExpanded((v) => !v)}
        >
          {expanded ? "Show Less" : "Show Full Text"}
        </button>
      )}
    </div>
  );
}

/** Light inline emphasis: *italic* / _italic_ → <em>. The narration cleaner
 * emits these (often for titles); raw asterisks/underscores read as broken. */
function renderInline(text: string): React.ReactNode[] {
  const parts: React.ReactNode[] = [];
  const re = /([*_])(?=\S)(.+?)(?<=\S)\1/g;
  let last = 0;
  let key = 0;
  let match: RegExpExecArray | null = re.exec(text);
  while (match !== null) {
    if (match.index > last) parts.push(text.slice(last, match.index));
    parts.push(<em key={key++}>{match[2]}</em>);
    last = match.index + match[0].length;
    match = re.exec(text);
  }
  if (last < text.length) parts.push(text.slice(last));
  return parts;
}

/** Render narration content: `## ` section markers become headings, blank-line
 * separated blocks become paragraphs, with inline emphasis. */
function TranscriptBody({ content }: { content: string }) {
  const blocks: React.ReactNode[] = [];
  let para: string[] = [];
  let key = 0;
  const flush = () => {
    const text = para.join(" ").trim();
    if (text) {
      blocks.push(
        <p key={key++} className="pods-transcript-p">
          {renderInline(text)}
        </p>,
      );
    }
    para = [];
  };
  for (const line of content.split("\n")) {
    const heading = line.match(/^##\s+(.+?)\s*$/);
    if (heading) {
      flush();
      blocks.push(
        <h3 key={key++} className="pods-transcript-heading">
          {renderInline(heading[1])}
        </h3>,
      );
    } else if (line.trim() === "") {
      flush();
    } else {
      para.push(line);
    }
  }
  flush();
  return <>{blocks}</>;
}

function CostTable({ episode }: { episode: PressPodsEpisodeDetail }) {
  const costs = episode.costs;
  if (!costs) return <div className="muted">No cost data recorded.</div>;

  const keys = new Set<string>([
    ...Object.keys(costs.detailCents),
    ...Object.keys(costs.detailTokens),
    ...Object.keys(costs.detailChars),
  ]);

  return (
    <>
      <div className="meta-row pods-cost-summary">
        <span>LLM {formatCents(costs.llmCents) ?? "—"}</span>
        <span>TTS {formatCents(costs.ttsCents) ?? "—"}</span>
        <span className="pods-cost-total">
          Total {formatCents(episode.costCents) ?? "—"}
        </span>
      </div>
      {keys.size > 0 && (
        <div className="pods-cost-list">
          {[...keys].sort().map((key) => {
            const tokens = costs.detailTokens[key];
            const chars = costs.detailChars[key];
            return (
              <div key={key} className="pods-cost-row">
                <div className="pods-cost-row-head">
                  <span className="pods-cost-key">{key}</span>
                  <span className="pods-cost-amount">
                    {formatCents(costs.detailCents[key] ?? null) ?? "—"}
                  </span>
                </div>
                {(tokens || chars !== undefined) && (
                  <div className="meta-row pods-cost-usage">
                    {tokens && (
                      <span>
                        {tokens.input.toLocaleString()} / {tokens.output.toLocaleString()}{" "}
                        tokens
                      </span>
                    )}
                    {chars !== undefined && <span>{chars.toLocaleString()} chars</span>}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </>
  );
}

export default function PodsDetailPage({ id }: { id: string }) {
  const [episode, setEpisode] = useState<PressPodsEpisodeDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [transcriptExpanded, setTranscriptExpanded] = useState(false);
  const [retrying, setRetrying] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const { toast, showToast } = useToast();

  const onRetry = async () => {
    if (!episode || retrying) return;
    setRetrying(true);
    try {
      await retryPressPodsEpisode(episode.episodeId);
      // A successful regeneration replaces (deletes) this episode row once the
      // new one lands, so this detail page would 404 on reload — send the user
      // back to the list where the in-progress job is visible.
      showToast("Re-queued for regeneration");
      navigate("/pods");
    } catch (err) {
      showToast(
        err instanceof Error ? err.message : "Failed to retry episode",
        "error",
      );
    } finally {
      setRetrying(false);
    }
  };

  const onDelete = async () => {
    if (!episode || deleting) return;
    const confirmed = window.confirm(
      `Delete "${episode.title}"? This removes the episode and its audio permanently.`,
    );
    if (!confirmed) return;
    setDeleting(true);
    try {
      await deletePressPodsEpisode(episode.episodeId);
      showToast("Episode deleted");
      navigate("/pods");
    } catch (err) {
      showToast(
        err instanceof Error ? err.message : "Failed to delete episode",
        "error",
      );
      setDeleting(false);
    }
  };

  useEffect(() => {
    let cancelled = false;
    fetchPressPodsEpisode(id)
      .then((res) => {
        if (!cancelled) setEpisode(res.episode);
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : "Failed to load episode");
        }
      });
    return () => {
      cancelled = true;
    };
  }, [id]);

  if (error) {
    return (
      <>
        <Link to="/pods" className="detail-back">
          <span className="detail-back-arrow" aria-hidden="true">
            ←
          </span>
          All Episodes
        </Link>
        <div className="error">
          <div>Failed to load this episode</div>
          <div className="error-detail">{error}</div>
        </div>
      </>
    );
  }
  if (!episode) return <div className="loading">Loading…</div>;

  const problemChunkCount = episode.chunks?.filter(isChunkProblematic).length ?? 0;

  return (
    <>
      <div className="pods-detail-back-row">
        <Link to="/pods" className="detail-back">
          <span className="detail-back-arrow" aria-hidden="true">
            ←
          </span>
          All episodes
        </Link>
        <div className="pods-detail-actions">
          <button
            type="button"
            className="pods-card-logs"
            onClick={onRetry}
            disabled={retrying}
          >
            {retrying ? "Retrying…" : "Retry"}
          </button>
          <button
            type="button"
            className="pods-card-logs pods-card-delete"
            onClick={onDelete}
            disabled={deleting}
          >
            {deleting ? "Deleting…" : "Delete"}
          </button>
        </div>
      </div>

      <div className="detail-head">
        <ImageWithFallback
          src={episode.leadImageUrl}
          alt={`${episode.title} lead image`}
          className="detail-art"
          placeholderClassName="detail-art-placeholder"
          placeholder="🎙️"
        />
        <div className="detail-head-body">
          <h1 className="detail-title">{episode.title}</h1>
          {(episode.author || episode.publication) && (
            <div className="pods-detail-byline">
              {episode.author}
              {episode.author && episode.authorGender && ` (${episode.authorGender})`}
              {episode.author && episode.publication && " · "}
              {episode.publication}
            </div>
          )}
          <div className="detail-badges">
            {episode.voiceName && (
              <span className="pods-detail-voice-badge">
                {episode.voiceName}
                {episode.voiceProvider && ` · ${episode.voiceProvider}`}
              </span>
            )}
            {formatAudioDuration(episode.durationSeconds) && (
              <span className="pods-detail-duration-badge">
                {formatAudioDuration(episode.durationSeconds)}
              </span>
            )}
            {formatCents(episode.costCents) && (
              <span className="pods-detail-cost-badge">
                {formatCents(episode.costCents)}
              </span>
            )}
          </div>
          <div className="rec-links">
            <a href={episode.articleUrl} target="_blank" rel="noreferrer">
              {episode.domain ?? "Article"} ↗
            </a>
            <a href={episode.audioUrl} target="_blank" rel="noreferrer">
              Audio File ↗
            </a>
          </div>
        </div>
      </div>

      <div className="detail-sections">
        <div className="pods-overview">
        <section className="page-section">
          <h2 className="section-title">Details</h2>
          <dl className="detail-grid">
            <DetailField label="Created">
              {formatAbsoluteWithYear(episode.createdAt)}
            </DetailField>
            {episode.publishedAt !== null && (
              <DetailField label="Published">
                {formatAbsoluteWithYear(episode.publishedAt)}
              </DetailField>
            )}
            <DetailField label="File Size">{formatBytes(episode.fileBytes)}</DetailField>
            {episode.synthesizedSeconds !== null && (
              <DetailField label="Synthesized Audio">
                {formatAudioDuration(episode.synthesizedSeconds)}
              </DetailField>
            )}
            {episode.retrieverSeconds !== null && (
              <DetailField label="Retrieval Time">
                {episode.retrieverSeconds.toFixed(1)}s
              </DetailField>
            )}
          </dl>
        </section>

        <section className="page-section">
          <h2 className="section-title">Cost Breakdown</h2>
          <CostTable episode={episode} />
        </section>

        {episode.retrieverAttempts && episode.retrieverAttempts.length > 0 && (
          <section className="page-section">
            <h2 className="section-title">Retriever Attempts</h2>
            <div className="pods-retriever-list">
              {episode.retrieverAttempts.map((attempt) => (
                <RetrieverRow
                  key={attempt.name}
                  attempt={attempt}
                  isWinner={attempt.name === episode.retrieverName}
                />
              ))}
            </div>
          </section>
        )}

        {episode.chapters && episode.chapters.length > 0 && (
          <section className="page-section">
            <h2 className="section-title">Chapters</h2>
            <ul className="pods-chapter-list">
              {episode.chapters.map((chapter) => (
                <li
                  key={`${chapter.startTimeSeconds}-${chapter.title}`}
                  className="pods-chapter-row"
                >
                  <span className="pods-chapter-time">
                    {formatAudioDuration(chapter.startTimeSeconds)}
                  </span>
                  <span className="pods-chapter-title">{chapter.title}</span>
                </li>
              ))}
            </ul>
          </section>
        )}
        </div>

        <section className="page-section">
          <h2 className="section-title">
            Audio Chunks
            {episode.chunks && <span className="section-count">{episode.chunks.length}</span>}
            {problemChunkCount > 0 && (
              <span className="pods-chunk-warn-count">{problemChunkCount} flagged</span>
            )}
          </h2>
          {episode.chunks === null && (
            <div className="muted">
              This episode was synthesized before per-chunk stats were recorded.
            </div>
          )}
          {episode.chunks && episode.chunks.length === 0 && (
            <div className="muted">No chunk data recorded.</div>
          )}
          {episode.chunks && episode.chunks.length > 0 && (
            <div className="pods-chunk-list">
              {episode.chunks.map((chunk) => (
                <ChunkCard key={chunk.index} chunk={chunk} />
              ))}
            </div>
          )}
        </section>

        <section className="page-section">
          <h2 className="section-title">Transcript</h2>
          <div
            className={`pods-transcript ${transcriptExpanded ? "pods-transcript-expanded" : ""}`}
          >
            <TranscriptBody content={episode.content} />
          </div>
          <button
            type="button"
            className="pods-transcript-toggle"
            onClick={() => setTranscriptExpanded((v) => !v)}
          >
            {transcriptExpanded ? "Collapse Transcript" : "Show Full Transcript"}
          </button>
        </section>
      </div>
      <Toast toast={toast} />
    </>
  );
}
