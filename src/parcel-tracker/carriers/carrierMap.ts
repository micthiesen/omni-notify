import type { Logger } from "@micthiesen/mitools/logging";
import got from "got";

type CarrierEntry = { code: string; name: string };
type CarrierResolution = { resolved: true; carrierCode: string } | { resolved: false };

// In-memory cache for Parcel's carrier list
let cachedCarriers: CarrierEntry[] | undefined;
let cachedAt = 0;
const CACHE_TTL_MS = 24 * 60 * 60 * 1000; // 24 hours

export async function resolveCarrierCode(
  carrierName: string,
  logger: Logger,
): Promise<CarrierResolution> {
  const carriers = await fetchCarrierList(logger);
  if (!carriers) return { resolved: false };

  const normalized = carrierName.toLowerCase().trim();

  // Tier 1: Exact code match (LLM says "ups", Parcel has code "ups")
  const codeMatch = carriers.find((c) => c.code === normalized);
  if (codeMatch) return { resolved: true, carrierCode: codeMatch.code };

  // Tier 2: Exact name match, case-insensitive
  const nameMatch = carriers.find((c) => c.name.toLowerCase() === normalized);
  if (nameMatch) return { resolved: true, carrierCode: nameMatch.code };

  // Tier 3: Substring containment — pick the shortest name match to prefer
  // specific entries (e.g. "DHL Express" over "DHL Global Forwarding" for "DHL")
  const substringMatches = carriers
    .filter(
      (c) =>
        c.name.toLowerCase().includes(normalized) ||
        normalized.includes(c.name.toLowerCase()),
    )
    .sort((a, b) => a.name.length - b.name.length);

  if (substringMatches.length > 0) {
    return { resolved: true, carrierCode: substringMatches[0].code };
  }

  // Tier 4: Token overlap — handles "Amazon Logistics" matching "Amazon US",
  // or "DHL eCommerce" matching "DHL Global Mail"
  const inputTokens = tokenize(normalized);
  if (inputTokens.length > 0) {
    let bestMatch: CarrierEntry | undefined;
    let bestScore = 0;

    for (const carrier of carriers) {
      const carrierTokens = tokenize(carrier.name.toLowerCase());
      const shared = inputTokens.filter((t) => carrierTokens.includes(t));
      const score = shared.length / Math.max(inputTokens.length, carrierTokens.length);
      if (score > bestScore) {
        bestScore = score;
        bestMatch = carrier;
      }
    }

    if (bestMatch && bestScore >= 0.5) {
      return { resolved: true, carrierCode: bestMatch.code };
    }
  }

  return { resolved: false };
}

const STOP_WORDS = new Set(["the", "of", "and", "&", "-"]);

function tokenize(input: string): string[] {
  return input.split(/[\s\-&]+/).filter((t) => t.length > 0 && !STOP_WORDS.has(t));
}

async function fetchCarrierList(logger: Logger): Promise<CarrierEntry[] | undefined> {
  if (cachedCarriers && Date.now() - cachedAt < CACHE_TTL_MS) {
    return cachedCarriers;
  }

  try {
    const response = await got(
      "https://api.parcel.app/external/supported_carriers.json",
    ).json<Record<string, string>>();
    cachedCarriers = Object.entries(response).map(([code, name]) => ({
      code,
      name,
    }));
    cachedAt = Date.now();
    return cachedCarriers;
  } catch (error) {
    logger.warn(`Failed to fetch Parcel carrier list: ${(error as Error).message}`);
    return cachedCarriers; // Return stale cache if available
  }
}

// Exported for testing
export { fetchCarrierList };
