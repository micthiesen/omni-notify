import { useEffect, useMemo, useState } from "react";
import {
  Bar,
  BarChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import {
  type CostEvent,
  type CostRange,
  type CostsResponse,
  fetchCosts,
  type ServiceCost,
} from "../api";
import {
  formatAbsoluteWithYear,
  formatCalendarDate,
  formatCents,
  formatCompactNumber,
  toTitleCase,
} from "../utils/format";

const RANGES: CostRange[] = [7, 30, 90, "all"];
const FEATURE_COLORS = [
  "#38bdf8",
  "#a770ff",
  "#4ade80",
  "#fbbf24",
  "#fb7185",
  "#2dd4bf",
  "#f97316",
  "#818cf8",
];

function rangeLabel(range: CostRange): string {
  return range === "all" ? "All" : `${range}d`;
}

function usageParts(service: ServiceCost): string[] {
  const parts: string[] = [];
  if (service.inputTokens || service.outputTokens) {
    parts.push(
      `${formatCompactNumber(service.inputTokens)} in / ${formatCompactNumber(service.outputTokens)} out`,
    );
  }
  if (service.characters) {
    parts.push(`${formatCompactNumber(service.characters)} characters`);
  }
  if (service.requests) parts.push(`${service.requests.toLocaleString()} requests`);
  if (service.credits) parts.push(`${service.credits.toLocaleString()} credits`);
  return parts;
}

function eventCost(event: CostEvent): string {
  if (event.costCents !== null) return formatCents(event.costCents) ?? "Unknown";
  return toTitleCase(event.priceStatus);
}

export default function CostsPage() {
  const [range, setRange] = useState<CostRange>(30);
  const [data, setData] = useState<CostsResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setData(null);
    setError(null);
    fetchCosts(range)
      .then((result) => {
        if (!cancelled) setData(result);
      })
      .catch((err) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : "Failed to load costs");
        }
      });
    return () => {
      cancelled = true;
    };
  }, [range]);

  const featureNames = useMemo(
    () => data?.byFeature.map((item) => item.feature) ?? [],
    [data],
  );
  const chartData = useMemo(
    () =>
      data?.daily.map((day) => ({
        ...day.byFeature,
        date: day.date,
        total: day.costCents,
        unknown: day.unknownEventCount,
      })) ?? [],
    [data],
  );
  const hasPricedDailyData = data?.daily.some((day) => day.pricedEventCount > 0) ?? false;

  return (
    <>
      <div className="page-header costs-header">
        <div className="page-header-stack">
          <h1>Costs</h1>
          <p className="page-subtitle">
            Usage and estimated spend across Omni Notify services.
          </p>
        </div>
        <div className="range-buttons" aria-label="Cost date range">
          {RANGES.map((item) => (
            <button
              key={item}
              type="button"
              className={`range-btn ${range === item ? "active" : ""}`}
              aria-pressed={range === item}
              onClick={() => setRange(item)}
            >
              {rangeLabel(item)}
            </button>
          ))}
        </div>
      </div>

      {data === null && error === null && <div className="loading">Loading…</div>}
      {error && data === null && (
        <div className="error">
          <div>Failed to load costs</div>
          <div className="error-detail">{error}</div>
        </div>
      )}
      {error && data !== null && <div className="error-inline">{error}</div>}

      {data && (
        <>
          <div className="stat-strip costs-stat-strip">
            <div className="stat-tile accent">
              <span className="stat-label">Selected Total</span>
              <span className="stat-value">
                {formatCents(data.summary.selectedCostCents)}
              </span>
              <span className="stat-detail">
                {data.summary.eventCount.toLocaleString()} tracked events
              </span>
            </div>
            <div className="stat-tile">
              <span className="stat-label">Daily Average</span>
              <span className="stat-value">
                {formatCents(data.summary.averageDailyCostCents)}
              </span>
              <span className="stat-detail">in selected range</span>
            </div>
            <div className="stat-tile">
              <span className="stat-label">Highest Day</span>
              <span className="stat-value">
                {formatCents(data.summary.highestDay?.costCents) ?? "—"}
              </span>
              <span className="stat-detail">
                {data.summary.highestDay
                  ? formatCalendarDate(data.summary.highestDay.date)
                  : "No priced usage"}
              </span>
            </div>
            <div className="stat-tile">
              <span className="stat-label">All-Time Total</span>
              <span className="stat-value">
                {formatCents(data.summary.allTimeCostCents)}
              </span>
              <span className="stat-detail">
                {data.summary.allTimeUnknownEventCount > 0
                  ? `${data.summary.allTimeUnknownEventCount.toLocaleString()} unpriced excluded`
                  : "all recorded usage"}
              </span>
            </div>
            <div className={`stat-tile ${data.summary.unknownEventCount ? "danger" : ""}`}>
              <span className="stat-label">Unpriced Events</span>
              <span className="stat-value">
                {data.summary.unknownEventCount.toLocaleString()}
              </span>
              <span className="stat-detail">excluded from totals</span>
            </div>
          </div>

          <div className="meta-row costs-usage-summary">
            <span>{data.summary.requests.toLocaleString()} requests</span>
            {(data.summary.inputTokens > 0 || data.summary.outputTokens > 0) && (
              <>
                <span>{formatCompactNumber(data.summary.inputTokens)} input tokens</span>
                <span>{formatCompactNumber(data.summary.outputTokens)} output tokens</span>
              </>
            )}
            {data.summary.characters > 0 && (
              <span>{formatCompactNumber(data.summary.characters)} characters</span>
            )}
            {data.summary.credits > 0 && (
              <span>{data.summary.credits.toLocaleString()} credits</span>
            )}
          </div>

          <section className="page-section">
            <h2 className="section-title">Daily Spend by Feature</h2>
            {!hasPricedDailyData || chartData.length === 0 || featureNames.length === 0 ? (
              <div className="no-data">No priced cost data in this range</div>
            ) : (
              <>
                <div className="costs-chart-legend">
                  {featureNames.map((feature, index) => (
                    <span key={feature}>
                      <i style={{ background: FEATURE_COLORS[index % FEATURE_COLORS.length] }} />
                      {toTitleCase(feature)}
                    </span>
                  ))}
                </div>
                <div className="chart-container costs-chart">
                  <ResponsiveContainer width="100%" height="100%">
                    <BarChart
                      accessibilityLayer
                      data={chartData}
                      margin={{ top: 8, right: 16, left: 4, bottom: 8 }}
                    >
                      <XAxis
                        dataKey="date"
                        tick={{ fill: "#8888a8", fontSize: 12 }}
                        tickLine={{ stroke: "#3a3a5a" }}
                        axisLine={{ stroke: "#3a3a5a" }}
                        tickFormatter={(date: string) => formatCalendarDate(date, false)}
                        minTickGap={40}
                      />
                      <YAxis
                        tick={{ fill: "#8888a8", fontSize: 12 }}
                        tickLine={{ stroke: "#3a3a5a" }}
                        axisLine={{ stroke: "#3a3a5a" }}
                        tickFormatter={(value: number) => formatCents(value) ?? ""}
                        width={62}
                      />
                      <Tooltip
                        cursor={{ fill: "rgba(56, 189, 248, 0.08)" }}
                        content={({ active, payload, label }) => {
                          if (!active || !payload?.length) return null;
                          return (
                            <div className="custom-tooltip">
                              <div className="tooltip-label">
                                {formatCalendarDate(String(label))}
                              </div>
                              {payload.map((entry) => (
                                <div key={String(entry.dataKey)} className="tooltip-row">
                                  <i
                                    className="costs-tooltip-swatch"
                                    style={{ background: entry.color }}
                                  />
                                  <span>
                                    {toTitleCase(String(entry.name))}: {formatCents(Number(entry.value))}
                                  </span>
                                </div>
                              ))}
                            </div>
                          );
                        }}
                      />
                      {featureNames.map((feature, index) => (
                        <Bar
                          key={feature}
                          dataKey={feature}
                          name={feature}
                          stackId="cost"
                          fill={FEATURE_COLORS[index % FEATURE_COLORS.length]}
                          maxBarSize={36}
                        />
                      ))}
                    </BarChart>
                  </ResponsiveContainer>
                </div>
              </>
            )}
          </section>

          <div className="costs-breakdown-grid">
            <section className="page-section">
              <h2 className="section-title">By Feature</h2>
              <div className="costs-list">
                {data.byFeature.map((item) => (
                  <div key={item.feature} className="costs-row">
                    <div className="costs-row-main">
                      <strong>{toTitleCase(item.feature)}</strong>
                      <span>
                        {item.eventCount.toLocaleString()} events
                        {item.unknownEventCount > 0
                          ? `, ${item.unknownEventCount.toLocaleString()} unpriced`
                          : ""}
                      </span>
                    </div>
                    <span className="costs-row-value">
                      {formatCents(item.costCents)}
                    </span>
                  </div>
                ))}
                {data.byFeature.length === 0 && (
                  <div className="muted">No feature costs in this range.</div>
                )}
              </div>
            </section>

            <section className="page-section">
              <h2 className="section-title">By Service</h2>
              <div className="costs-list">
                {data.byService.map((item) => {
                  const usage = usageParts(item);
                  return (
                    <div
                      key={`${item.service}:${item.model ?? ""}:${item.category}`}
                      className="costs-row"
                    >
                      <div className="costs-row-main">
                        <strong>{toTitleCase(item.service)}</strong>
                        <span className="costs-model">
                          {item.model ?? toTitleCase(item.category)}
                        </span>
                        {usage.length > 0 && <span>{usage.join(" · ")}</span>}
                        {item.unknownEventCount > 0 && (
                          <span>{item.unknownEventCount} unpriced</span>
                        )}
                      </div>
                      <span className="costs-row-value">
                        {formatCents(item.costCents)}
                      </span>
                    </div>
                  );
                })}
                {data.byService.length === 0 && (
                  <div className="muted">No service costs in this range.</div>
                )}
              </div>
            </section>
          </div>

          <section className="page-section">
            <h2 className="section-title">Recent Cost Events</h2>
            <div className="costs-event-list">
              {data.recent.map((event) => (
                <div key={event.eventId} className="costs-event-row">
                  <div className="costs-event-when">
                    <strong>{formatAbsoluteWithYear(event.incurredAt)}</strong>
                    <span>{toTitleCase(event.feature)}</span>
                  </div>
                  <div className="costs-event-detail">
                    <strong>{toTitleCase(event.operation)}</strong>
                    <span className="meta-row">
                      <span>{toTitleCase(event.service)}</span>
                      {event.model && <span>{event.model}</span>}
                      <span>{toTitleCase(event.category)}</span>
                    </span>
                  </div>
                  <span
                    className={`costs-event-value ${event.costCents === null ? "unknown" : ""}`}
                  >
                    {eventCost(event)}
                  </span>
                </div>
              ))}
              {data.recent.length === 0 && (
                <div className="muted">No recent cost events in this range.</div>
              )}
            </div>
          </section>
        </>
      )}
    </>
  );
}
