import { useEffect, useState } from "react";
import { fetchPressPodsEpisode } from "../api";
import type {
  PressPodsChunkStat,
  PressPodsEpisodeDetail,
  PressPodsRetrieverAttempt,
} from "../api";
import { ImageWithFallback } from "../components/ImageWithFallback";
import { formatAudioDuration } from "./PodsPage";
import { Link } from "../router";
import { formatAbsoluteWithYear, formatCents } from "../utils/format";

// Higgs length-verify bounds (see synthesize.ts): chunks outside this range
// were catastrophically truncated or ran away and got re-synthesized.
const SEC_PER_CHAR_MIN = 0.03;
const SEC_PER_CHAR_MAX = 0.15;

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
        {problematic && (
          <span className="pods-chunk-warn-badge">Needs Review</span>
        )}
      </div>
      <div className="meta-row pods-chunk-meta">
        <span>{chunk.charCount.toLocaleString()} chars</span>
        <span>{chunk.durationSeconds.toFixed(1)}s</span>
        <span>{chunk.secPerChar.toFixed(3)} s/char</span>
        <span>
          {chunk.attempts} attempt{chunk.attempts === 1 ? "" : "s"}
        </span>
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
        <span>Total {formatCents(episode.costCents) ?? "—"}</span>
      </div>
      {keys.size > 0 && (
        <div className="pods-cost-table">
          <div className="pods-cost-table-header">
            <span>Model / Function</span>
            <span>Cost</span>
            <span>Tokens (in/out)</span>
            <span>Chars</span>
          </div>
          {[...keys].sort().map((key) => {
            const tokens = costs.detailTokens[key];
            return (
              <div key={key} className="pods-cost-table-row">
                <span className="pods-cost-table-key">{key}</span>
                <span>{formatCents(costs.detailCents[key] ?? null) ?? "—"}</span>
                <span>
                  {tokens ? `${tokens.input.toLocaleString()} / ${tokens.output.toLocaleString()}` : "—"}
                </span>
                <span>
                  {costs.detailChars[key] !== undefined
                    ? costs.detailChars[key].toLocaleString()
                    : "—"}
                </span>
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
      <Link to="/pods" className="detail-back">
        <span className="detail-back-arrow" aria-hidden="true">
          ←
        </span>
        All episodes
      </Link>

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
            {episode.content}
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
    </>
  );
}
