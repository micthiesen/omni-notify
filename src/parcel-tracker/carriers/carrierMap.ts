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
    cachedCarriers = Object.entries(response)
      .filter(([code]) => !isBlacklistedCarrier(code))
      .map(([code, name]) => ({ code, name }));
    cachedAt = Date.now();
    return cachedCarriers;
  } catch (error) {
    logger.warn(`Failed to fetch Parcel carrier list: ${(error as Error).message}`);
    return cachedCarriers; // Return stale cache if available
  }
}

// Maintenance: To prune this list, fetch https://api.parcel.app/external/supported_carriers.json
// and diff it against the entries below. Remove any blacklisted codes that Parcel has dropped,
// and consider blacklisting new codes that are clearly irrelevant for a Canadian recipient
// (regional last-mile carriers in distant countries, freight/B2B services, non-shipping platforms).
// Keep international postal services and cross-border shippers — they can carry inbound packages.
const BLACKLISTED_PREFIXES = [
  "amzl", // Amazon regional
  "amship", // Amazon Shipping
];

const BLACKLISTED_CARRIERS = new Set([
  // Food delivery / non-shipping
  "doordash",
  "pholder", // Placeholder Deliveries

  // Freight / B2B logistics (not consumer parcels)
  "abf", // ABF Freight
  "ceva", // Ceva Logistics
  "dachser", // Dachser
  "dsv", // DSV
  "geodis", // Geodis
  "mscgva", // MSC (shipping line)
  "pilot", // Pilot Freight
  "safmar", // Safmarine (shipping line)
  "sch", // DB Schenker
  "seabour", // Seabourne Logistics
  "straight", // Straightship
  "pfl", // Parcel Freight Logistics
  "syncreon", // Syncreon

  // Russia / CIS
  "rp", // Russian Post
  "ems", // EMS Russian Post
  "edos", // CDEK
  "boxb", // Boxberry
  "shiptor", // Shiptor
  "fivepost", // 5post
  "dellin", // Delovie Linii
  "pec", // PEC
  "energia", // TK Energia
  "major", // Major Express
  "blp", // Belpost (Belarus)
  "kz", // Kazpost
  "azer", // Azerpost
  "moldov", // Moldova Post
  "newp", // Nova Poshta (Ukraine)
  "ukr", // Ukrpost

  // Middle East / Africa
  "naqel", // Naqel Express
  "smsa", // SMSA Express
  "saudi", // Saudi Post
  "emirates", // Emirates Post
  "imile", // iMile
  "jordan", // Jordan Post
  "safr", // South African Post Office
  "il", // Israel Post

  // South / SE Asia (regional last-mile)
  "dtdc", // DTDC India
  "bluedart", // Blue Dart (India)
  "in", // India Post
  "kerry", // Kerry Express (Thailand)
  "thai", // Thailand Post
  "skynetm", // Skynet Malaysia
  "malpos", // Malaysia Post
  "phlpost", // Philpost
  "indon", // Indonesia Post
  "bluecare", // Bluecare Express

  // Latin America (regional)
  "oca", // OCA Argentina
  "chilex", // Chilexpress
  "colomb", // Colombia post (4-72)
  "corm", // Correos de Mexico
  "estafe", // Estafeta (Mexico)
  "redpack", // Redpack (Mexico)
  "paquet", // Paquetexpress (Mexico)
  "serpost", // Serpost (Peru)
  "corurg", // Correo Uruguayo
  "corbra", // Correios (Brazil)
  "vasp", // Vasp Expresso (Brazil)

  // Eastern Europe (regional last-mile)
  "econt", // Econt Express (Bulgaria)
  "bolg", // Bulgarian Post
  "serbia", // Serbia Post
  "hr", // Hrvatska pošta (Croatia)
  "hrpar", // HR Parcel (Croatia)
  "hung", // Magyar Posta (Hungary)
  "ceska", // Česká pošta
  "slovak", // Slovenská pošta
  "slv", // Pošta Slovenije
  "litva", // Lietuvos paštas
  "ee", // Eesti Post (Estonia)
  "lv", // Latvijas Pasts (Latvia)
  "cypr", // Cyprus Post
  "geniki", // Geniki Taxydromiki (Greece)
  "elta", // Elta (Greece)
  "venipak", // Venipak (Baltics)

  // Oceania (regional last-mile)
  "airroad", // AirRoad (AU)
  "star", // StarTrack Express (AU)
  "fastau", // Fastway AU
  "tntau", // TNT Australia
  "couple", // Couriers Please (AU)
  "northline", // Northline (AU)
  "allied", // Allied Express (AU)
  "sendle", // Sendle (AU)
  "coup", // CourierPost (NZ)
  "fastnz", // Fastway NZ
  "pbt", // PBT New Zealand
  "parcelpnt", // ParcelPoint (AU)

  // Spain (domestic last-mile)
  "acs", // ACS Courier (Greece)
  "asmred", // GLS Spain
  "celeritas", // Celeritas
  "chrexp", // Correos Express
  "cor", // Correos
  "envia", // Ontime - Envialia
  "mrw", // MRW
  "nacex", // Nacex
  "seur", // SEUR
  "tipsac", // Tipsa
  "tourline", // CTT Express (Spain/Portugal)
  "zel", // Zeleris

  // Italy (domestic last-mile)
  "bartol", // Bartolini
  "glsit", // GLS Italy

  // Malta / Turkey / Pakistan
  "malta", // MaltaPost
  "turk", // PTT (Turkey)
  "pk", // Pakistan Post

  // UK / Germany heavy goods & niche
  "arrowxl", // Arrow XL (UK heavy goods)
  "dx", // DX (UK)
  "her2mann", // Hermes 2-Mann-Handling (German heavy goods)

  // Niche cargo
  "hawai", // Hawaiian Air Cargo
  "koreanair", // Korean Air Cargo
]);

function isBlacklistedCarrier(code: string): boolean {
  return (
    BLACKLISTED_PREFIXES.some((prefix) => code.startsWith(prefix)) ||
    BLACKLISTED_CARRIERS.has(code)
  );
}

/** Fetches carrier names and returns word-boundary regexes. */
export async function getCarrierNamePatterns(logger: Logger): Promise<RegExp[]> {
  const carriers = await fetchCarrierList(logger);
  if (!carriers) return [];
  return carriers.map((c) => new RegExp(`\\b${escapeRegExp(c.name)}\\b`, "i"));
}

function escapeRegExp(str: string): string {
  return str.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
