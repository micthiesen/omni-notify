import type { OnDeckItem } from "../api";
import { Link } from "../router";
import { ImageWithFallback } from "./ImageWithFallback";

function OnDeckPoster({ item }: { item: OnDeckItem }) {
  return (
    <ImageWithFallback
      src={item.posterPath ? `https://image.tmdb.org/t/p/w185${item.posterPath}` : null}
      alt={`${item.title} poster`}
      className="ondeck-poster"
      placeholderClassName="ondeck-poster-placeholder"
      loading="lazy"
      placeholder={
        <svg
          width="20"
          height="20"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.5"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <rect x="3" y="4" width="18" height="16" rx="2" />
          <path d="M3 9h18M7 4v5M12 4v5M17 4v5" />
        </svg>
      }
    />
  );
}

function OnDeckCard({ item }: { item: OnDeckItem }) {
  return (
    <Link
      className="ondeck-card"
      to={`/media/${encodeURIComponent(item.recommendationId)}`}
      title={item.title}
    >
      <OnDeckPoster item={item} />
      <div className="ondeck-body">
        <div className="ondeck-title-row">
          <span className="ondeck-title">{item.title}</span>
          {item.year !== null && <span className="ondeck-year"> ({item.year})</span>}
        </div>
        <span className={`media-badge media-${item.mediaType}`}>
          {item.mediaType === "tv" ? "TV" : "Movie"}
        </span>
        {item.whyForUser && <p className="ondeck-why">{item.whyForUser}</p>}
      </div>
    </Link>
  );
}

/** "What else could I watch" strip: newest delivered-but-unwatched picks.
 * Renders nothing when empty — it's a secondary answer, never noise. */
export function OnDeck({ items }: { items: OnDeckItem[] }) {
  if (items.length === 0) return null;

  return (
    <section className="page-section ondeck-section">
      <h2 className="section-title">On Deck</h2>
      <div className="ondeck-row">
        {items.map((item) => (
          <OnDeckCard key={item.recommendationId} item={item} />
        ))}
      </div>
    </section>
  );
}
