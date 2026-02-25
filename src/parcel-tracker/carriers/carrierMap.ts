import type { Logger } from "@micthiesen/mitools/logging";
import got from "got";

type CarrierEntry = { code: string; name: string };

// In-memory cache for Parcel's carrier list
let cachedCarriers: CarrierEntry[] | undefined;
let cachedAt = 0;
const CACHE_TTL_MS = 24 * 60 * 60 * 1000; // 24 hours

/** Returns "code: name" lines for inclusion in the LLM extraction prompt. */
export async function getCarrierCodesForPrompt(logger: Logger): Promise<string> {
  const carriers = await fetchCarrierList(logger);
  if (!carriers) return "";
  return carriers.map((c) => `${c.code}: ${c.name}`).join("\n");
}

/** Checks if a carrier code exists in the Parcel carrier list. */
export async function isValidCarrierCode(
  code: string,
  logger: Logger,
): Promise<boolean> {
  const carriers = await fetchCarrierList(logger);
  if (!carriers) return false;
  return carriers.some((c) => c.code === code);
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

const AMAZON_CODE_PREFIXES = ["amzl", "amship"];

export function isAmazonCarrier(code: string): boolean {
  return AMAZON_CODE_PREFIXES.some((prefix) => code.startsWith(prefix));
}

/** Fetches carrier names and returns word-boundary regexes, excluding Amazon carriers. */
export async function getCarrierNamePatterns(logger: Logger): Promise<RegExp[]> {
  const carriers = await fetchCarrierList(logger);
  if (!carriers) return [];
  return carriers
    .filter((c) => !isAmazonCarrier(c.code))
    .map((c) => new RegExp(`\\b${escapeRegExp(c.name)}\\b`, "i"));
}

function escapeRegExp(str: string): string {
  return str.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
