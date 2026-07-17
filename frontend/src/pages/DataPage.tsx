import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  deleteDataRow,
  fetchDataEntities,
  fetchDataRows,
  type DataEntity,
  type DataRow,
  type DataStorageSummary,
  type DataValue,
} from "../api";
import { Toast, useToast } from "../components/Toast";
import { toTitleCase } from "../utils/format";

type SortDirection = "asc" | "desc";

const MALFORMED_ROW_KEY = "__dataManagerMalformed";

type MalformedRowMetadata = {
  rawKey: string;
  error: string;
};

function getMalformedMetadata(row: DataRow): MalformedRowMetadata | null {
  const value = row[MALFORMED_ROW_KEY];
  if (
    typeof value !== "object" ||
    value === null ||
    Array.isArray(value) ||
    typeof value.rawKey !== "string" ||
    typeof value.error !== "string"
  ) {
    return null;
  }
  return { rawKey: value.rawKey, error: value.error };
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let unit = units[0];
  for (const next of units.slice(1)) {
    if (value < 1024) break;
    value /= 1024;
    unit = next;
  }
  return `${value >= 10 ? value.toFixed(0) : value.toFixed(1)} ${unit}`;
}

function json(value: DataValue | DataRow): string {
  return JSON.stringify(value);
}

function keyFor(row: DataRow, primaryKey: string[]): DataRow {
  const malformed = getMalformedMetadata(row);
  if (malformed) return { [MALFORMED_ROW_KEY]: malformed };
  return Object.fromEntries(primaryKey.map((property) => [property, row[property]]));
}

function rowId(row: DataRow, primaryKey: string[]): string {
  return json(keyFor(row, primaryKey));
}

function displayValue(value: DataValue | undefined): { text: string; title: string } {
  if (value === undefined) return { text: "", title: "Missing" };
  if (value === null) return { text: "null", title: "null" };
  if (Array.isArray(value)) {
    return {
      text: `[${value.length} item${value.length === 1 ? "" : "s"}]`,
      title: json(value),
    };
  }
  if (typeof value === "object") {
    const count = Object.keys(value).length;
    return {
      text: `{${count} field${count === 1 ? "" : "s"}}`,
      title: json(value),
    };
  }
  return { text: String(value), title: String(value) };
}

function compareValues(a: DataValue | undefined, b: DataValue | undefined): number {
  if (typeof a === "number" && typeof b === "number") return a - b;
  return displayValue(a).text.localeCompare(displayValue(b).text, undefined, {
    numeric: true,
    sensitivity: "base",
  });
}

function RowDetail({
  entity,
  row,
  deleting,
  onClose,
  onDelete,
}: {
  entity: DataEntity;
  row: DataRow;
  deleting: boolean;
  onClose: () => void;
  onDelete: () => void;
}) {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  return (
    <div
      className="modal-root"
      role="dialog"
      aria-modal="true"
      aria-label="Row detail"
    >
      <button
        className="modal-backdrop"
        type="button"
        onClick={onClose}
        aria-label="Close"
      />
      <div className="data-detail-modal">
        <div className="data-detail-header">
          <div>
            <div className="data-detail-label">{toTitleCase(entity.label)}</div>
            <code>{json(keyFor(row, entity.primaryKey))}</code>
          </div>
          <button
            className="log-modal-close"
            type="button"
            onClick={onClose}
            aria-label="Close"
          >
            ✕
          </button>
        </div>
        <pre className="data-detail-json">{JSON.stringify(row, null, 2)}</pre>
        <div className="data-detail-footer">
          <button
            className="data-delete-btn"
            type="button"
            disabled={deleting}
            onClick={onDelete}
          >
            {deleting ? "Deleting…" : "Delete row"}
          </button>
        </div>
      </div>
    </div>
  );
}

export default function DataPage() {
  const [entities, setEntities] = useState<DataEntity[]>([]);
  const [storage, setStorage] = useState<DataStorageSummary | null>(null);
  const [selectedSlug, setSelectedSlug] = useState<string>("");
  const [rows, setRows] = useState<DataRow[]>([]);
  const [loadingEntities, setLoadingEntities] = useState(true);
  const [loadingRows, setLoadingRows] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [sortColumn, setSortColumn] = useState<string | null>(null);
  const [sortDirection, setSortDirection] = useState<SortDirection>("asc");
  const [detailRow, setDetailRow] = useState<DataRow | null>(null);
  const [deletingId, setDeletingId] = useState<string | null>(null);
  const rowsRequest = useRef(0);
  const { toast, showToast } = useToast();

  useEffect(() => {
    let cancelled = false;
    void fetchDataEntities()
      .then(({ entities: fetched, storage: fetchedStorage }) => {
        if (cancelled) return;
        setEntities(fetched);
        setStorage(fetchedStorage);
        setSelectedSlug((current) => current || fetched[0]?.slug || "");
        setError(null);
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : "Failed to load entities");
        }
      })
      .finally(() => {
        if (!cancelled) setLoadingEntities(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const selected = entities.find((entity) => entity.slug === selectedSlug) ?? null;

  const loadRows = useCallback(async () => {
    if (!selectedSlug) return;
    const request = ++rowsRequest.current;
    setLoadingRows(true);
    try {
      const { summary, rows: fetched } = await fetchDataRows(selectedSlug);
      if (request !== rowsRequest.current) return;
      setRows(fetched);
      setEntities((current) =>
        current.map((entity) => (entity.slug === summary.slug ? summary : entity)),
      );
      setError(null);
    } catch (err) {
      if (request !== rowsRequest.current) return;
      setError(err instanceof Error ? err.message : "Failed to load rows");
    } finally {
      if (request === rowsRequest.current) setLoadingRows(false);
    }
  }, [selectedSlug]);

  useEffect(() => {
    setRows([]);
    setQuery("");
    setSortColumn(null);
    setDetailRow(null);
    void loadRows();
  }, [loadRows]);

  const columns = useMemo(() => {
    if (!selected) return [];
    const frequency = new Map<string, number>();
    for (const row of rows) {
      for (const property of Object.keys(row)) {
        frequency.set(property, (frequency.get(property) ?? 0) + 1);
      }
    }
    const rest = [...frequency.keys()]
      .filter(
        (property) =>
          !selected.primaryKey.includes(property) &&
          property !== MALFORMED_ROW_KEY,
      )
      .sort(
        (a, b) =>
          (frequency.get(b) ?? 0) - (frequency.get(a) ?? 0) || a.localeCompare(b),
      );
    return [
      ...selected.primaryKey,
      ...(frequency.has(MALFORMED_ROW_KEY) ? [MALFORMED_ROW_KEY] : []),
      ...rest,
    ];
  }, [rows, selected]);

  const visibleRows = useMemo(() => {
    const needle = query.trim().toLocaleLowerCase();
    const filtered = needle
      ? rows.filter((row) => json(row).toLocaleLowerCase().includes(needle))
      : [...rows];
    if (sortColumn) {
      filtered.sort((a, b) => {
        const result = compareValues(a[sortColumn], b[sortColumn]);
        return sortDirection === "asc" ? result : -result;
      });
    }
    return filtered;
  }, [query, rows, sortColumn, sortDirection]);

  const selectSort = (column: string) => {
    if (sortColumn === column) {
      setSortDirection((direction) => (direction === "asc" ? "desc" : "asc"));
    } else {
      setSortColumn(column);
      setSortDirection("asc");
    }
  };

  const deleteRow = async (row: DataRow) => {
    if (!selected) return;
    const key = keyFor(row, selected.primaryKey);
    const label = getMalformedMetadata(row)?.rawKey ?? json(key);
    if (
      !window.confirm(
        `Delete ${selected.label} row ${label}? This cannot be undone.`,
      )
    ) {
      return;
    }
    const id = rowId(row, selected.primaryKey);
    setDeletingId(id);
    try {
      await deleteDataRow(selected.slug, key);
      setRows((current) =>
        current.filter(
          (candidate) => rowId(candidate, selected.primaryKey) !== id,
        ),
      );
      setEntities((current) =>
        current.map((entity) =>
          entity.slug === selected.slug
            ? { ...entity, count: Math.max(0, entity.count - 1) }
            : entity,
        ),
      );
      setDetailRow(null);
      showToast("Row deleted", "info");
      void fetchDataEntities()
        .then(({ entities: fetched, storage: fetchedStorage }) => {
          setEntities(fetched);
          setStorage(fetchedStorage);
        })
        .catch(() => {});
    } catch (err) {
      showToast(
        err instanceof Error ? err.message : "Failed to delete row",
        "error",
      );
    } finally {
      setDeletingId(null);
    }
  };

  if (loadingEntities) return <div className="loading">Loading data…</div>;
  if (error && entities.length === 0) return <div className="error">{error}</div>;

  return (
    <>
      <div className="page-header data-page-header">
        <div className="page-header-stack">
          <h1>Data</h1>
          <p className="page-subtitle">
            Browse and remove records stored by mitools Entities.
          </p>
        </div>
        <div className="data-header-actions">
          {storage && (
            <div className="data-storage-summary">
              <span title="SQLite allocated pages, including relational tables and indexes">
                <strong>{formatBytes(storage.databaseSizeBytes)}</strong> database
              </span>
              <span title="Encoded payload bytes across registered mitools Entities">
                <strong>{formatBytes(storage.entityStorageBytes)}</strong> entities
              </span>
            </div>
          )}
          <button
            className="run-btn"
            type="button"
            onClick={() => {
              void loadRows();
              void fetchDataEntities()
                .then(({ entities: fetched, storage: fetchedStorage }) => {
                  setEntities(fetched);
                  setStorage(fetchedStorage);
                })
                .catch(() => {});
            }}
            disabled={loadingRows}
          >
            {loadingRows ? "Refreshing…" : "Refresh"}
          </button>
        </div>
      </div>

      <div className="data-layout">
        <aside className="data-entity-panel" aria-label="Entities">
          <label
            className="data-mobile-select-label"
            htmlFor="data-entity-select"
          >
            Entity
          </label>
          <select
            id="data-entity-select"
            className="data-entity-select"
            value={selectedSlug}
            onChange={(event) => setSelectedSlug(event.target.value)}
          >
            {entities.map((entity) => (
              <option key={entity.slug} value={entity.slug}>
                {toTitleCase(entity.label)} ({entity.count})
              </option>
            ))}
          </select>
          <div className="data-entity-list">
            {entities.map((entity) => (
              <button
                key={entity.slug}
                className={`data-entity-item ${entity.slug === selectedSlug ? "active" : ""}`}
                type="button"
                onClick={() => setSelectedSlug(entity.slug)}
              >
                <span className="data-entity-name">{toTitleCase(entity.label)}</span>
                <span className="data-entity-meta">
                  <span>{formatBytes(entity.storageBytes)}</span>
                  <span className="data-entity-count">{entity.count}</span>
                </span>
              </button>
            ))}
          </div>
        </aside>

        <section className="data-browser">
          {selected && (
            <>
              <div className="data-browser-heading">
                <div>
                  <h2>{toTitleCase(selected.label)}</h2>
                  <p>{selected.description}</p>
                </div>
                <div className="data-browser-aside">
                  <code>{selected.slug}</code>
                  <span className="data-selected-size">
                    {formatBytes(selected.storageBytes)} payload
                  </span>
                </div>
              </div>
              {selected.warning && (
                <div className="data-warning">{selected.warning}</div>
              )}
              <div className="data-controls">
                <input
                  className="data-search"
                  type="search"
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                  placeholder="Search every field…"
                  aria-label="Search rows"
                />
                <span className="data-row-count">
                  {visibleRows.length === rows.length
                    ? rows.length
                    : `${visibleRows.length} of ${rows.length}`} {" "}
                  rows
                </span>
              </div>

              {error && <div className="error-inline">{error}</div>}
              <div className="data-table-wrap">
                {loadingRows && rows.length === 0 ? (
                  <div className="loading-inline">Loading rows…</div>
                ) : visibleRows.length === 0 ? (
                  <div className="data-empty">
                    {rows.length === 0
                      ? "No rows in this entity."
                      : "No rows match your search."}
                  </div>
                ) : (
                  <table className="data-table">
                    <thead>
                      <tr>
                        {columns.map((column) => (
                          <th
                            key={column}
                            className={
                              selected.primaryKey.includes(column)
                                ? "data-pk-column"
                                : ""
                            }
                          >
                            <button type="button" onClick={() => selectSort(column)}>
                              {column === MALFORMED_ROW_KEY
                                ? "Malformed record"
                                : column}
                              {sortColumn === column && (
                                <span>
                                  {sortDirection === "asc" ? " ↑" : " ↓"}
                                </span>
                              )}
                            </button>
                          </th>
                        ))}
                        <th className="data-actions-column">
                          <span className="sr-only">Actions</span>
                        </th>
                      </tr>
                    </thead>
                    <tbody>
                      {visibleRows.map((row) => {
                        const id = rowId(row, selected.primaryKey);
                        const malformed = getMalformedMetadata(row);
                        return (
                          <tr
                            key={id}
                            className={malformed ? "data-row-malformed" : ""}
                            onClick={() => setDetailRow(row)}
                          >
                            {columns.map((column) => {
                              if (column === MALFORMED_ROW_KEY && malformed) {
                                return (
                                  <td
                                    key={column}
                                    className="data-malformed-cell"
                                    title={malformed.error}
                                  >
                                    Malformed: {malformed.error}
                                  </td>
                                );
                              }
                              const value = displayValue(row[column]);
                              return (
                                <td key={column} title={value.title}>
                                  {value.text}
                                </td>
                              );
                            })}
                            <td className="data-actions-column">
                              <button
                                className="data-trash-btn"
                                type="button"
                                title="Delete row"
                                aria-label="Delete row"
                                disabled={deletingId === id}
                                onClick={(event) => {
                                  event.stopPropagation();
                                  void deleteRow(row);
                                }}
                              >
                                {deletingId === id ? "…" : "×"}
                              </button>
                            </td>
                          </tr>
                        );
                      })}
                    </tbody>
                  </table>
                )}
              </div>
            </>
          )}
        </section>
      </div>

      {detailRow && selected && (
        <RowDetail
          entity={selected}
          row={detailRow}
          deleting={deletingId === rowId(detailRow, selected.primaryKey)}
          onClose={() => setDetailRow(null)}
          onDelete={() => void deleteRow(detailRow)}
        />
      )}
      {toast && <Toast toast={toast} />}
    </>
  );
}
