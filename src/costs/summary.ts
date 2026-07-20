import type { CostEventData, CostUsage } from "./persistence.js";

export interface CostSummaryOptions {
  days: number | null;
  now?: number;
  timeZone: string;
}

function sumUsage(target: Required<CostUsage>, usage: CostUsage): void {
  for (const key of Object.keys(target) as (keyof CostUsage)[]) {
    target[key] += usage[key] ?? 0;
  }
}

function emptyUsage(): Required<CostUsage> {
  return {
    inputTokens: 0,
    inputNoCacheTokens: 0,
    cacheReadTokens: 0,
    cacheWriteTokens: 0,
    outputTokens: 0,
    reasoningTokens: 0,
    characters: 0,
    requests: 0,
    credits: 0,
  };
}

function dayKey(timestamp: number, timeZone: string): string {
  const parts = new Intl.DateTimeFormat("en-CA", {
    timeZone,
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  }).formatToParts(timestamp);
  const get = (type: Intl.DateTimeFormatPartTypes) =>
    parts.find((part) => part.type === type)?.value ?? "";
  return `${get("year")}-${get("month")}-${get("day")}`;
}

export function summarizeCosts(events: CostEventData[], options: CostSummaryOptions) {
  const now = options.now ?? Date.now();
  const from = options.days === null ? null : now - options.days * 86_400_000;
  const selected = events.filter(
    (event) => event.incurredAt <= now && (from === null || event.incurredAt >= from),
  );
  const allTimeCostCents = events
    .filter((event) => event.incurredAt <= now)
    .reduce((sum, event) => sum + (event.costCents ?? 0), 0);
  const allTimeUnknownEventCount = events.filter(
    (event) => event.incurredAt <= now && event.costCents === null,
  ).length;
  const usage = emptyUsage();
  let selectedCostCents = 0;
  let unknownEventCount = 0;
  const daily = new Map<
    string,
    {
      date: string;
      costCents: number;
      byFeature: Record<string, number>;
      pricedEventCount: number;
      unknownEventCount: number;
    }
  >();
  const feature = new Map<
    string,
    {
      feature: string;
      costCents: number;
      eventCount: number;
      unknownEventCount: number;
    }
  >();
  const service = new Map<
    string,
    {
      service: string;
      model: string | null;
      category: string;
      costCents: number;
      eventCount: number;
      unknownEventCount: number;
    } & Required<CostUsage>
  >();

  for (const event of selected) {
    selectedCostCents += event.costCents ?? 0;
    sumUsage(usage, event.usage);
    const unknown = event.costCents === null ? 1 : 0;
    unknownEventCount += unknown;

    const date = dayKey(event.incurredAt, options.timeZone);
    const day = daily.get(date) ?? {
      date,
      costCents: 0,
      byFeature: {},
      pricedEventCount: 0,
      unknownEventCount: 0,
    };
    day.costCents += event.costCents ?? 0;
    day.byFeature[event.feature] =
      (day.byFeature[event.feature] ?? 0) + (event.costCents ?? 0);
    if (event.costCents !== null) day.pricedEventCount++;
    day.unknownEventCount += unknown;
    daily.set(date, day);

    const featureRow = feature.get(event.feature) ?? {
      feature: event.feature,
      costCents: 0,
      eventCount: 0,
      unknownEventCount: 0,
    };
    featureRow.costCents += event.costCents ?? 0;
    featureRow.eventCount++;
    featureRow.unknownEventCount += unknown;
    feature.set(event.feature, featureRow);

    const serviceKey = `${event.category}\0${event.service}\0${event.model ?? ""}`;
    const serviceRow = service.get(serviceKey) ?? {
      service: event.service,
      model: event.model ?? null,
      category: event.category,
      costCents: 0,
      eventCount: 0,
      unknownEventCount: 0,
      ...emptyUsage(),
    };
    serviceRow.costCents += event.costCents ?? 0;
    serviceRow.eventCount++;
    serviceRow.unknownEventCount += unknown;
    sumUsage(serviceRow, event.usage);
    service.set(serviceKey, serviceRow);
  }

  const dailyRows = [...daily.values()].sort((a, b) => a.date.localeCompare(b.date));
  const highestDay = dailyRows
    .filter((day) => day.pricedEventCount > 0)
    .reduce<(typeof dailyRows)[number] | null>(
      (highest, day) => (!highest || day.costCents > highest.costCents ? day : highest),
      null,
    );
  const elapsedDays =
    options.days ??
    (selected.length === 0
      ? 0
      : Math.max(
          1,
          Math.ceil(
            (now - Math.min(...selected.map((event) => event.incurredAt))) / 86_400_000,
          ),
        ));

  return {
    range: { days: options.days, from, to: now },
    summary: {
      selectedCostCents,
      allTimeCostCents,
      allTimeUnknownEventCount,
      averageDailyCostCents: elapsedDays === 0 ? 0 : selectedCostCents / elapsedDays,
      highestDay: highestDay
        ? { date: highestDay.date, costCents: highestDay.costCents }
        : null,
      eventCount: selected.length,
      unknownEventCount,
      inputTokens: usage.inputTokens,
      outputTokens: usage.outputTokens,
      characters: usage.characters,
      requests: usage.requests,
      credits: usage.credits,
    },
    daily: dailyRows,
    byFeature: [...feature.values()].sort((a, b) => b.costCents - a.costCents),
    byService: [...service.values()].sort((a, b) => b.costCents - a.costCents),
    recent: selected
      .slice()
      .sort((a, b) => b.incurredAt - a.incurredAt)
      .slice(0, 50)
      .map((event) => ({
        ...event,
        model: event.model ?? null,
        runId: event.runId ?? null,
      })),
  };
}
