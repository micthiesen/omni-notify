/**
 * Status filter chip bar shared by the Recommendations and Podcasts lists.
 * Statuses with zero items are hidden; clicking the active chip clears it.
 */
export function StatusFilterChips<S extends string>({
  order,
  labels,
  counts,
  total,
  active,
  onChange,
}: {
  order: readonly S[];
  labels: Record<S, string>;
  counts: ReadonlyMap<S, number>;
  total: number;
  active: S | "";
  onChange: (status: S | "") => void;
}) {
  return (
    <div className="rec-filters" role="group" aria-label="Filter by Status">
      <button
        type="button"
        className={`chip-btn ${active === "" ? "active" : ""}`}
        aria-pressed={active === ""}
        onClick={() => onChange("")}
      >
        All <span className="chip-btn-count">{total}</span>
      </button>
      {order
        .filter((status) => counts.has(status))
        .map((status) => (
          <button
            key={status}
            type="button"
            className={`chip-btn ${active === status ? "active" : ""}`}
            aria-pressed={active === status}
            onClick={() => onChange(active === status ? "" : status)}
          >
            {labels[status]}{" "}
            <span className="chip-btn-count">{counts.get(status)}</span>
          </button>
        ))}
    </div>
  );
}
