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
    <div className="rec-filters">
      <button
        type="button"
        className={`chip-btn ${active === "" ? "active" : ""}`}
        onClick={() => onChange("")}
      >
        All ({total})
      </button>
      {order
        .filter((status) => counts.has(status))
        .map((status) => (
          <button
            key={status}
            type="button"
            className={`chip-btn ${active === status ? "active" : ""}`}
            onClick={() => onChange(active === status ? "" : status)}
          >
            {labels[status]} ({counts.get(status)})
          </button>
        ))}
    </div>
  );
}
