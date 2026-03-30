import { useEffect, useRef, useState } from "react";
import {
  Brush,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";

interface WeightEntry {
  timestamp: string;
  weight: number;
}

interface DailyVisit {
  date: string;
  count: number;
}

interface Pet {
  petId: string;
  name: string;
  currentWeight: number;
  weightHistory: WeightEntry[];
  dailyVisits: DailyVisit[];
}

type Range = "7d" | "30d" | "90d" | "all";
type ChartMode = "weight" | "visits";

const COLORS = [
  "#4fc3f7",
  "#81c784",
  "#ffb74d",
  "#e57373",
  "#ba68c8",
  "#4db6ac",
];

const RANGE_DAYS: Record<Range, number | null> = {
  "7d": 7,
  "30d": 30,
  "90d": 90,
  all: null,
};

function filterByRange(history: WeightEntry[], range: Range): WeightEntry[] {
  const days = RANGE_DAYS[range];
  if (days === null) return history;
  const cutoff = new Date();
  cutoff.setDate(cutoff.getDate() - days);
  return history.filter((e) => new Date(e.timestamp) >= cutoff);
}

function filterVisitsByRange(
  visits: DailyVisit[],
  range: Range,
): DailyVisit[] {
  const days = RANGE_DAYS[range];
  if (days === null) return visits;
  const cutoff = new Date();
  cutoff.setDate(cutoff.getDate() - days);
  const cutoffStr = cutoff.toISOString().slice(0, 10);
  return visits.filter((v) => v.date >= cutoffStr);
}

function formatDate(timestamp: string): string {
  const d = new Date(timestamp);
  return d.toLocaleDateString("en-US", { month: "short", day: "numeric" });
}

function formatTooltipDate(timestamp: string): string {
  const d = new Date(timestamp);
  return d.toLocaleDateString("en-US", {
    month: "short",
    day: "numeric",
    year: "numeric",
  });
}

interface DataPoint {
  timestamp: string;
  epoch: number;
  [key: string]: string | number | undefined;
}

function ewma(values: number[], alpha: number): number[] {
  if (values.length === 0) return [];
  const result = [values[0]];
  for (let i = 1; i < values.length; i++) {
    result.push(alpha * values[i] + (1 - alpha) * result[i - 1]);
  }
  return result;
}

function linearRegression(points: { x: number; y: number }[]): {
  slope: number;
  intercept: number;
  r2: number;
} {
  const n = points.length;
  if (n < 2) return { slope: 0, intercept: points[0]?.y ?? 0, r2: 0 };

  let sumX = 0;
  let sumY = 0;
  let sumXY = 0;
  let sumXX = 0;
  for (const p of points) {
    sumX += p.x;
    sumY += p.y;
    sumXY += p.x * p.y;
    sumXX += p.x * p.x;
  }

  const denom = n * sumXX - sumX * sumX;
  if (denom === 0) return { slope: 0, intercept: sumY / n, r2: 0 };

  const slope = (n * sumXY - sumX * sumY) / denom;
  const intercept = (sumY - slope * sumX) / n;

  const meanY = sumY / n;
  let ssRes = 0;
  let ssTot = 0;
  for (const p of points) {
    const predicted = intercept + slope * p.x;
    ssRes += (p.y - predicted) ** 2;
    ssTot += (p.y - meanY) ** 2;
  }
  const r2 = ssTot === 0 ? 1 : 1 - ssRes / ssTot;

  return { slope, intercept, r2 };
}

interface LineConfig {
  dataKey: string;
  stroke: string;
  strokeWidth: number;
  type: "monotoneX" | "linear";
  dash?: string;
  opacity?: number;
  dot: boolean;
}

interface ChartConfig {
  data: DataPoint[];
  yDomain: [number, number];
  yFormatter: (v: number) => string;
  lines: LineConfig[];
  tooltipLines: {
    dataKey: string;
    stroke: string;
    width: number;
    dash?: string;
    opacity?: number;
    label: string;
  }[];
  tooltipUnit: string;
  showBrush: boolean;
}

interface SeriesInput {
  dataKey: string;
  label: string;
  unit: string;
  points: { timestamp: string; value: number }[];
  yMinZero?: boolean;
}

interface ChartBuildResult {
  config: ChartConfig;
  slopePerWeek: number;
  r2: number;
}

function buildChartConfig(
  input: SeriesInput,
  range: Range,
  color: string,
): ChartBuildResult {
  const baseData = input.points.map((p) => ({
    timestamp: p.timestamp,
    epoch: new Date(p.timestamp).getTime(),
    [input.dataKey]: p.value,
  }));

  const t0 = baseData.length > 0 ? baseData[0].epoch : 0;
  const MS_PER_DAY = 86_400_000;
  const values = input.points.map((p) => p.value);
  const regressionPoints = baseData.map((d, i) => ({
    x: (d.epoch - t0) / MS_PER_DAY,
    y: values[i],
  }));
  const { slope, intercept, r2 } = linearRegression(regressionPoints);

  const smoothed = ewma(values, 0.15);

  const data: DataPoint[] = baseData.map((d, i) => {
    const xDays = (d.epoch - t0) / MS_PER_DAY;
    return {
      ...d,
      smoothed: Math.round(smoothed[i] * 100) / 100,
      trend: Math.round((intercept + slope * xDays) * 100) / 100,
    };
  });

  let yDomain: [number, number];
  if (input.yMinZero) {
    const maxVal = values.length > 0 ? Math.max(...values) : 1;
    yDomain = [0, maxVal + 1];
  } else {
    const minVal = values.length > 0 ? Math.min(...values) : 0;
    const maxVal = values.length > 0 ? Math.max(...values) : 10;
    const padding = Math.max((maxVal - minVal) * 0.15, 0.2);
    yDomain = [
      Math.floor((minVal - padding) * 10) / 10,
      Math.ceil((maxVal + padding) * 10) / 10,
    ];
  }

  return {
    config: {
      data,
      yDomain,
      yFormatter: input.yMinZero
        ? (v: number) => `${Math.round(v)}`
        : (v: number) => `${v}`,
      lines: [
        {
          dataKey: input.dataKey,
          stroke: color,
          strokeWidth: 2,
          type: "monotoneX",
          dot: true,
        },
        {
          dataKey: "smoothed",
          stroke: "#ffffff",
          strokeWidth: 2,
          type: "monotoneX",
          opacity: 0.6,
          dot: false,
        },
        {
          dataKey: "trend",
          stroke: color,
          strokeWidth: 1.5,
          type: "linear",
          dash: "6 4",
          opacity: 0.5,
          dot: false,
        },
      ],
      tooltipLines: [
        { dataKey: input.dataKey, stroke: color, width: 2, label: input.label },
        {
          dataKey: "smoothed",
          stroke: "#ffffff",
          width: 2,
          opacity: 0.6,
          label: "Smoothed",
        },
        {
          dataKey: "trend",
          stroke: color,
          width: 1.5,
          dash: "4 3",
          opacity: 0.5,
          label: "Trend",
        },
      ],
      tooltipUnit: input.unit,
      showBrush: range === "all" && data.length > 60,
    },
    slopePerWeek: slope * 7,
    r2,
  };
}

function PetCard({ pet, colorIndex }: { pet: Pet; colorIndex: number }) {
  const [range, setRange] = useState<Range>("30d");
  const [mode, setMode] = useState<ChartMode>("weight");
  const [tooltipActive, setTooltipActive] = useState(false);
  const chartRef = useRef<HTMLDivElement>(null);
  const color = COLORS[colorIndex % COLORS.length];

  useEffect(() => {
    function handlePointerDown(e: PointerEvent) {
      if (chartRef.current && !chartRef.current.contains(e.target as Node)) {
        setTooltipActive(false);
      }
    }
    document.addEventListener("pointerdown", handlePointerDown);
    return () => document.removeEventListener("pointerdown", handlePointerDown);
  }, []);

  const sorted = [...pet.weightHistory].sort(
    (a, b) =>
      new Date(a.timestamp).getTime() - new Date(b.timestamp).getTime(),
  );
  const filteredWeight = filterByRange(sorted, range);
  const filteredVisits = filterVisitsByRange(pet.dailyVisits, range);

  const weightResult = buildChartConfig(
    {
      dataKey: "weight",
      label: "Weight",
      unit: "lbs",
      points: filteredWeight.map((e) => ({
        timestamp: e.timestamp,
        value: e.weight,
      })),
    },
    range,
    color,
  );
  const visitResult = buildChartConfig(
    {
      dataKey: "count",
      label: "Visits",
      unit: "visits",
      yMinZero: true,
      points: filteredVisits.map((v) => ({ timestamp: v.date, value: v.count })),
    },
    range,
    color,
  );

  const { slopePerWeek, r2 } = weightResult;
  const chartConfig =
    mode === "weight" ? weightResult.config : visitResult.config;

  const ranges: Range[] = ["7d", "30d", "90d", "all"];

  const exportUrl =
    `/api/pets/${pet.petId}/export.csv` +
    (range !== "all" ? `?days=${RANGE_DAYS[range]}` : "");

  const todayStr = new Date().toISOString().slice(0, 10);
  const visitsToday = pet.dailyVisits.find((d) => d.date === todayStr)?.count ?? 0;
  const completeDays = filteredVisits.filter((d) => d.date !== todayStr);
  const avgPerDay =
    completeDays.length > 0
      ? completeDays.reduce((s, d) => s + d.count, 0) / completeDays.length
      : 0;

  return (
    <div className="pet-card">
      <div className="pet-header">
        <div>
          <span className="pet-name">{pet.name}</span>
          <span className="pet-weight"> &mdash; {pet.currentWeight} lbs</span>
          {filteredWeight.length >= 2 && (
            <span
              className={`pet-trend ${slopePerWeek >= 0 ? "up" : "down"}`}
              title={`R² = ${r2.toFixed(3)} (${r2 >= 0.7 ? "strong" : r2 >= 0.3 ? "moderate" : "weak"} fit)`}
            >
              {slopePerWeek >= 0 ? "+" : ""}
              {slopePerWeek.toFixed(2)} lbs/wk
            </span>
          )}
          <span className="pet-visits">
            {visitsToday} today &middot; avg {avgPerDay.toFixed(1)}/day
          </span>
        </div>
        <div className="range-controls">
          <div className="mode-toggle">
            <button
              className={`range-btn ${mode === "weight" ? "active" : ""}`}
              onClick={() => setMode("weight")}
            >
              Weight
            </button>
            <button
              className={`range-btn ${mode === "visits" ? "active" : ""}`}
              onClick={() => setMode("visits")}
            >
              Visits
            </button>
          </div>
          <a href={exportUrl} className="export-btn" title="Export CSV">
            <svg
              width="16"
              height="16"
              viewBox="0 0 16 16"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.5"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <path d="M8 2v8M5 7l3 3 3-3M3 12h10M3 14h10" />
            </svg>
          </a>
          <div className="range-buttons">
            {ranges.map((r) => (
              <button
                key={r}
                className={`range-btn ${range === r ? "active" : ""}`}
                onClick={() => setRange(r)}
              >
                {r === "all" ? "All" : r}
              </button>
            ))}
          </div>
        </div>
      </div>
      {chartConfig.data.length === 0 ? (
        <div className="no-data">
          No {mode === "weight" ? "weight" : "visit"} data for this range
        </div>
      ) : (
        <div className="chart-container" ref={chartRef}>
          <ResponsiveContainer width="100%" height="100%">
            <LineChart
              data={chartConfig.data}
              margin={{
                top: 8,
                right: 16,
                left: 4,
                bottom: chartConfig.showBrush ? 28 : 8,
              }}
              onMouseMove={() => setTooltipActive(true)}
              onMouseLeave={() => setTooltipActive(false)}
            >
              <XAxis
                dataKey="epoch"
                type="number"
                scale="time"
                domain={["dataMin", "dataMax"]}
                tick={{ fill: "#8888a8", fontSize: 12 }}
                tickLine={{ stroke: "#3a3a5a" }}
                axisLine={{ stroke: "#3a3a5a" }}
                tickFormatter={(v: number) =>
                  formatDate(new Date(v).toISOString())
                }
                minTickGap={50}
              />
              <YAxis
                domain={chartConfig.yDomain}
                tick={{ fill: "#8888a8", fontSize: 12 }}
                tickLine={{ stroke: "#3a3a5a" }}
                axisLine={{ stroke: "#3a3a5a" }}
                tickFormatter={chartConfig.yFormatter}
                width={40}
              />
              <Tooltip
                content={({ active, payload, label }) => {
                  if (!active || !tooltipActive || !payload?.length)
                    return null;
                  return (
                    <div className="custom-tooltip">
                      <div className="tooltip-label">
                        {formatTooltipDate(
                          new Date(label as number).toISOString(),
                        )}
                      </div>
                      {payload.map((entry) => {
                        const style = chartConfig.tooltipLines.find(
                          (l) => l.dataKey === entry.dataKey,
                        );
                        if (!style) return null;
                        return (
                          <div key={entry.dataKey} className="tooltip-row">
                            <svg
                              width="20"
                              height="12"
                              className="tooltip-swatch"
                            >
                              <line
                                x1="0"
                                y1="6"
                                x2="20"
                                y2="6"
                                stroke={style.stroke}
                                strokeWidth={style.width}
                                strokeDasharray={style.dash}
                                strokeOpacity={style.opacity ?? 1}
                              />
                            </svg>
                            <span>
                              {style.label}: {entry.value}{" "}
                              {chartConfig.tooltipUnit}
                            </span>
                          </div>
                        );
                      })}
                    </div>
                  );
                }}
              />
              {chartConfig.lines.map((line) => (
                <Line
                  key={line.dataKey}
                  type={line.type}
                  dataKey={line.dataKey}
                  stroke={line.stroke}
                  strokeWidth={line.strokeWidth}
                  strokeDasharray={line.dash}
                  strokeOpacity={line.opacity}
                  dot={
                    line.dot
                      ? {
                          r: chartConfig.data.length <= 60 ? 3 : 0,
                          fill: line.stroke,
                          strokeWidth: 0,
                        }
                      : false
                  }
                  activeDot={
                    line.dot
                      ? {
                          r: 5,
                          fill: line.stroke,
                          stroke: "#1a1a2e",
                          strokeWidth: 2,
                        }
                      : false
                  }
                />
              ))}
              {chartConfig.showBrush && (
                <Brush
                  dataKey="epoch"
                  height={24}
                  stroke="#4a6fa5"
                  fill="#16213e"
                  travellerWidth={8}
                  tickFormatter={() => ""}
                />
              )}
            </LineChart>
          </ResponsiveContainer>
        </div>
      )}
    </div>
  );
}

export default function App() {
  const [pets, setPets] = useState<Pet[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    async function fetchPets() {
      try {
        const res = await fetch("/api/pets");
        if (!res.ok) {
          throw new Error(`HTTP ${res.status}: ${res.statusText}`);
        }
        const data: Pet[] = await res.json();
        if (!cancelled) {
          setPets(data);
          setLoading(false);
        }
      } catch (err) {
        if (!cancelled) {
          setError(
            err instanceof Error ? err.message : "Failed to fetch pets",
          );
          setLoading(false);
        }
      }
    }

    fetchPets();
    return () => {
      cancelled = true;
    };
  }, []);

  if (loading) {
    return <div className="loading">Loading...</div>;
  }

  if (error) {
    return (
      <div className="error">
        <div>Failed to load pet data</div>
        <div className="error-detail">{error}</div>
      </div>
    );
  }

  return (
    <>
      <h1>Pet Weight Tracker</h1>
      {pets.map((pet, i) => (
        <PetCard key={pet.petId} pet={pet} colorIndex={i} />
      ))}
    </>
  );
}
