import type { Logger } from "@micthiesen/mitools/logging";
import { Effect, Schema } from "effect";
import {
  fetchPublicText,
  PUBLIC_HTTP_USER_AGENT,
  type PublicTextRequest,
} from "../../effect/publicHttp.js";
import { CarrierListError } from "../effect.js";

type CarrierEntry = { code: string; name: string };
// In-memory cache for Parcel's carrier list
let cachedCarriers: CarrierEntry[] | undefined;
let cachedAt = 0;
const CACHE_TTL_MS = 24 * 60 * 60 * 1000; // 24 hours
export const CARRIER_LIST_MAX_BYTES = 2 * 1024 * 1024;
const refreshMutex = Effect.unsafeMakeSemaphore(1);
const CarrierResponseSchema = Schema.Record({
  key: Schema.String,
  value: Schema.Union(
    Schema.String,
    Schema.Struct({ name: Schema.optional(Schema.String) }),
  ),
});

interface CarrierListDependencies {
  readonly request?: PublicTextRequest;
  readonly maxResponseBytes?: number;
}

export function getCarrierCodesForPromptEffect(
  logger: Logger,
  dependencies: CarrierListDependencies = {},
): Effect.Effect<string, never> {
  return Effect.map(fetchCarrierListEffect(logger, dependencies), (carriers) =>
    (carriers ?? []).map((c) => `${c.code}: ${c.name}`).join("\n"),
  );
}

/** Returns "code: name" lines for inclusion in the LLM extraction prompt. */
export function getValidCarrierCodesEffect(
  logger: Logger,
  dependencies: CarrierListDependencies = {},
): Effect.Effect<ReadonlySet<string> | undefined, never> {
  return Effect.map(fetchCarrierListEffect(logger, dependencies), (carriers) =>
    carriers ? new Set(carriers.map((c) => c.code)) : undefined,
  );
}

/**
 * Returns the set of valid Parcel carrier codes, or undefined when the carrier
 * list is unavailable (fetch failed and no cache).
 */
function fetchCarrierListEffect(
  logger: Logger,
  dependencies: CarrierListDependencies,
): Effect.Effect<CarrierEntry[] | undefined, never> {
  return refreshMutex.withPermits(1)(
    Effect.gen(function* () {
      const now = yield* Effect.clockWith((clock) => clock.currentTimeMillis);
      if (cachedCarriers && now - cachedAt < CACHE_TTL_MS) return cachedCarriers;

      const decoded = yield* fetchPublicText(
        "https://api.parcel.app/external/supported_carriers.json",
        {
          headers: { "User-Agent": PUBLIC_HTTP_USER_AGENT },
          timeout: { request: 15_000 },
        },
        "Fetch Parcel carrier list",
        dependencies.request,
        dependencies.maxResponseBytes ?? CARRIER_LIST_MAX_BYTES,
      ).pipe(
        Effect.flatMap(Schema.decodeUnknown(Schema.parseJson(CarrierResponseSchema))),
        Effect.mapError((cause) => new CarrierListError({ cause })),
        Effect.catchAll((error) => {
          logger.warn(`Failed to fetch Parcel carrier list: ${error.message}`);
          return Effect.succeed(undefined);
        }),
      );
      if (!decoded) return cachedCarriers;
      cachedCarriers = Object.entries(decoded)
        .filter(([code]) => !isBlacklistedCarrier(code))
        .flatMap(([code, value]) => {
          const name = typeof value === "string" ? value : value.name;
          return typeof name === "string" ? [{ code, name }] : [];
        });
      cachedAt = now;
      return cachedCarriers;
    }),
  );
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
export function getCarrierNamePatternsEffect(
  logger: Logger,
  dependencies: CarrierListDependencies = {},
): Effect.Effect<RegExp[], never> {
  return Effect.map(fetchCarrierListEffect(logger, dependencies), (carriers) =>
    (carriers ?? []).map((c) => new RegExp(`\\b${escapeRegExp(c.name)}\\b`, "i")),
  );
}

function escapeRegExp(str: string): string {
  return str.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
