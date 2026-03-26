import { useEffect, useState } from "react";
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

interface Pet {
  petId: string;
  name: string;
  currentWeight: number;
  weightHistory: WeightEntry[];
}

type Range = "7d" | "30d" | "90d" | "all";

const COLORS = ["#4fc3f7", "#81c784", "#ffb74d", "#e57373", "#ba68c8", "#4db6ac"];

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

interface ChartDataPoint {
  timestamp: string;
  epoch: number;
  weight: number;
  smoothed?: number;
  trend?: number;
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

function PetCard({ pet, colorIndex }: { pet: Pet; colorIndex: number }) {
  const [range, setRange] = useState<Range>("30d");
  const color = COLORS[colorIndex % COLORS.length];

  const sorted = [...pet.weightHistory].sort(
    (a, b) => new Date(a.timestamp).getTime() - new Date(b.timestamp).getTime()
  );
  const filtered = filterByRange(sorted, range);

  const baseData = filtered.map((e) => ({
    timestamp: e.timestamp,
    epoch: new Date(e.timestamp).getTime(),
    weight: e.weight,
  }));

  const t0 = baseData.length > 0 ? baseData[0].epoch : 0;
  const MS_PER_DAY = 86_400_000;
  const regressionPoints = baseData.map((d) => ({
    x: (d.epoch - t0) / MS_PER_DAY,
    y: d.weight,
  }));
  const { slope, intercept, r2 } = linearRegression(regressionPoints);
  const slopePerWeek = slope * 7;

  const smoothed = ewma(
    baseData.map((d) => d.weight),
    0.15,
  );

  const data: ChartDataPoint[] = baseData.map((d, i) => {
    const xDays = (d.epoch - t0) / MS_PER_DAY;
    return {
      ...d,
      smoothed: Math.round(smoothed[i] * 100) / 100,
      trend: Math.round((intercept + slope * xDays) * 100) / 100,
    };
  });

  const weights = data.map((d) => d.weight);
  const minW = weights.length > 0 ? Math.min(...weights) : 0;
  const maxW = weights.length > 0 ? Math.max(...weights) : 10;
  const padding = Math.max((maxW - minW) * 0.15, 0.2);
  const yMin = Math.floor((minW - padding) * 10) / 10;
  const yMax = Math.ceil((maxW + padding) * 10) / 10;

  const showBrush = range === "all" && data.length > 60;

  const ranges: Range[] = ["7d", "30d", "90d", "all"];

  const exportUrl =
    `/api/pets/${pet.petId}/export.csv` +
    (range !== "all" ? `?days=${RANGE_DAYS[range]}` : "");

  return (
    <div className="pet-card">
      <div className="pet-header">
        <div>
          <span className="pet-name">{pet.name}</span>
          <span className="pet-weight"> — {pet.currentWeight} lbs</span>
          {baseData.length >= 2 && (
            <span
              className={`pet-trend ${slopePerWeek >= 0 ? "up" : "down"}`}
              title={`R² = ${r2.toFixed(3)} (${r2 >= 0.7 ? "strong" : r2 >= 0.3 ? "moderate" : "weak"} fit)`}
            >
              {slopePerWeek >= 0 ? "+" : ""}
              {slopePerWeek.toFixed(2)} lbs/wk
            </span>
          )}
        </div>
        <div className="range-controls">
          <a href={exportUrl} className="export-btn" title="Export CSV">
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
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
      {data.length === 0 ? (
        <div className="no-data">No weight data for this range</div>
      ) : (
        <div className="chart-container">
          <ResponsiveContainer width="100%" height="100%">
            <LineChart data={data} margin={{ top: 8, right: 16, left: 4, bottom: showBrush ? 28 : 8 }}>
              <XAxis
                dataKey="epoch"
                type="number"
                scale="time"
                domain={["dataMin", "dataMax"]}
                tick={{ fill: "#8888a8", fontSize: 12 }}
                tickLine={{ stroke: "#3a3a5a" }}
                axisLine={{ stroke: "#3a3a5a" }}
                tickFormatter={(v: number) => formatDate(new Date(v).toISOString())}
                minTickGap={50}
              />
              <YAxis
                domain={[yMin, yMax]}
                tick={{ fill: "#8888a8", fontSize: 12 }}
                tickLine={{ stroke: "#3a3a5a" }}
                axisLine={{ stroke: "#3a3a5a" }}
                tickFormatter={(v: number) => `${v}`}
                width={40}
              />
              <Tooltip
                contentStyle={{
                  background: "#1a1a2e",
                  border: "1px solid #3a3a5a",
                  borderRadius: 8,
                  color: "#e0e0e0",
                  fontSize: 13,
                }}
                labelFormatter={(value: number) =>
                  formatTooltipDate(new Date(value).toISOString())
                }
                formatter={(value: number, name: string) => [
                  `${value} lbs`,
                  name === "trend" ? "Trend" : "Weight",
                ]}
              />
              <Line
                type="monotoneX"
                dataKey="weight"
                stroke={color}
                strokeWidth={2}
                dot={{ r: data.length <= 60 ? 3 : 0, fill: color, strokeWidth: 0 }}
                activeDot={{ r: 5, fill: color, stroke: "#1a1a2e", strokeWidth: 2 }}
              />
              <Line
                type="monotoneX"
                dataKey="smoothed"
                stroke="#ffffff"
                strokeWidth={2}
                strokeOpacity={0.6}
                dot={false}
                activeDot={false}
                name="Smoothed"
              />
              <Line
                type="linear"
                dataKey="trend"
                stroke={color}
                strokeWidth={1.5}
                strokeDasharray="6 4"
                strokeOpacity={0.5}
                dot={false}
                activeDot={false}
                name="Trend"
              />
              {showBrush && (
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
          setError(err instanceof Error ? err.message : "Failed to fetch pets");
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
