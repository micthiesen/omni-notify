import type { ReactNode } from "react";
import type { TasteClaim } from "../api";
import { formatAbsolute, formatRelative } from "../utils/format";

/** The claim-based fields shared by the media and podcast taste profiles. */
export interface TasteBrainProfile {
  version: number;
  generatedAt: number;
  summary: string;
  stablePreferences: TasteClaim[];
  conditionalPreferences: TasteClaim[];
  aversions: TasteClaim[];
  currentSaturation: TasteClaim[];
  explorationTargets: TasteClaim[];
  uncertainties: TasteClaim[];
}

function ClaimList({ claims }: { claims: TasteClaim[] }) {
  return (
    <ul className="taste-claim-list">
      {claims.map((item) => (
        <li key={item.claim}>
          <span>{item.claim}</span>
          <span
            className="taste-confidence"
            title={`${item.evidenceIds.length} supporting evidence item${item.evidenceIds.length === 1 ? "" : "s"}`}
          >
            {Math.round(item.confidence * 100)}%
          </span>
        </li>
      ))}
    </ul>
  );
}

function TagGroup({ label, claims, explore }: {
  label: string;
  claims: TasteClaim[];
  explore?: boolean;
}) {
  return (
    <div>
      <span className="taste-tags-label">{label}</span>
      <div className="taste-tags">
        {claims.map((target) => (
          <span
            className={`taste-tag ${explore ? "taste-tag-explore" : ""}`}
            key={target.claim}
            title={`${target.evidenceIds.length} supporting evidence item(s)`}
          >
            {target.claim}
          </span>
        ))}
      </div>
    </div>
  );
}

/**
 * Shared "Taste brain" section for the Recommendations and Podcasts pages.
 * Callers provide the domain-specific stats rows, subtitle, empty-state text,
 * and an optional footer (e.g. commitment preferences for media).
 */
export function TasteBrain({
  profile,
  loading,
  error,
  subtitle,
  emptyText,
  stats,
  footer,
}: {
  profile: TasteBrainProfile | null;
  loading: boolean;
  error: string | null;
  subtitle: string;
  emptyText: string;
  stats: [string, ReactNode][];
  footer?: ReactNode;
}) {
  const columns: [string, TasteClaim[]][] = profile
    ? [
        ["Reliable preferences", profile.stablePreferences],
        ["Depends on context", profile.conditionalPreferences],
        ["Avoid", profile.aversions],
        ["Still learning", profile.uncertainties],
      ]
    : [];

  return (
    <section className="page-section taste-brain">
      <div className="taste-heading">
        <div>
          <h2 className="section-title">Taste brain</h2>
          <div className="muted taste-subtitle">{subtitle}</div>
        </div>
        {profile && (
          <span
            className="taste-version meta-row"
            title={formatAbsolute(profile.generatedAt)}
          >
            <span>v{profile.version}</span>
            <span>{formatRelative(profile.generatedAt)}</span>
          </span>
        )}
      </div>

      {loading && <div className="loading-inline">Loading taste profile…</div>}
      {!loading && error && (
        <div className="error-inline">Taste profile unavailable: {error}</div>
      )}
      {!loading && !error && profile === null && (
        <div className="taste-empty">{emptyText}</div>
      )}
      {profile && (
        <div className="taste-card">
          <p className="taste-summary">{profile.summary}</p>
          {stats.length > 0 && (
            <div className="taste-stats">
              {stats.map(([name, value]) => (
                <div className="taste-stat" key={name}>
                  <span>{name}</span>
                  <strong>{value}</strong>
                </div>
              ))}
            </div>
          )}
          <div className="taste-columns">
            {columns
              .filter(([, claims]) => claims.length > 0)
              .map(([title, claims]) => (
                <div key={title}>
                  <h3>{title}</h3>
                  <ClaimList claims={claims} />
                </div>
              ))}
          </div>
          {(profile.explorationTargets.length > 0 ||
            profile.currentSaturation.length > 0) && (
            <div className="taste-tags-row">
              {profile.explorationTargets.length > 0 && (
                <TagGroup
                  label="Explore"
                  claims={profile.explorationTargets}
                  explore
                />
              )}
              {profile.currentSaturation.length > 0 && (
                <TagGroup
                  label="Currently saturated"
                  claims={profile.currentSaturation}
                />
              )}
            </div>
          )}
          {footer}
        </div>
      )}
    </section>
  );
}
