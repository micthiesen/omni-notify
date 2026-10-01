import { Result, Schema } from "effect";

// Protocol observations, independently implemented from Apple's public web client:
// https://www.icloud.com/applications/reminders2/2636Build17/en-us/main.js
// RecurrenceRule model and its RRule display adapter. Frequency is zero-based;
// pyicloud's one-based frequency enum must not be used for these records.
// Selector bounds follow RFC 5545 section 3.3.10. This codec transforms service
// VALUES only: the CloudKit wrapper types for selector/EndDate writes still need
// verification. It does not calculate occurrences or complete recurring reminders.
const MAX_SELECTOR_BYTES = 16_384;
const MAX_FIELD_COUNT = 32;
const frequencies = [
  "daily",
  "weekly",
  "monthly",
  "yearly",
  "hourly",
  "minutely",
  "secondly",
] as const;
export type RecurrenceFrequency = (typeof frequencies)[number];

const integer = (minimum: number, maximum: number) =>
  Schema.Number.check(Schema.isInt(), Schema.isBetween({ minimum, maximum }));
const signed = (maximum: number) =>
  integer(-maximum, maximum).check(Schema.makeFilter((value) => value !== 0));
const numericList = (item: Schema.Codec<number>) =>
  Schema.Array(item).check(Schema.isMaxLength(732));
const optional = <A, I>(schema: Schema.Codec<A, I>) =>
  Schema.optional(Schema.NullOr(schema));
const Day = Schema.Struct({
  dayOfTheWeek: integer(1, 7),
  // Apple uses zero to mean an unqualified weekday, unlike RFC's numeric prefix.
  weekNumber: Schema.optional(integer(-53, 53)),
});

/** Explicit rules only. Missing scalar defaults are never inferred from old clients. */
export const RecurrenceRuleSchema = Schema.Struct({
  frequency: Schema.Literals(frequencies),
  interval: integer(1, 2_147_483_647),
  occurrenceCount: optional(integer(0, 2_147_483_647)),
  endDate: optional(integer(0, 253_402_300_799_000)),
  // Existing server records use zero. Preserve it without assigning a weekday;
  // Apple's web recurrence display does not interpret this field.
  firstDayOfWeek: optional(integer(0, 7)),
  daysOfWeek: optional(Schema.Array(Day).check(Schema.isMaxLength(371))),
  daysOfMonth: optional(numericList(signed(31))),
  daysOfYear: optional(numericList(signed(366))),
  weeksOfYear: optional(numericList(signed(53))),
  monthsOfYear: optional(numericList(integer(1, 12))),
  setPositions: optional(numericList(signed(366))),
});
export type RecurrenceRule = Schema.Schema.Type<typeof RecurrenceRuleSchema>;
export type RecurrenceUnsupported = {
  readonly supported: false;
  readonly reason:
    | "invalid_fields"
    | "unknown_fields"
    | "unknown_frequency"
    | "invalid_selector"
    | "invalid_rule";
  /** Names only, bounded; never return arbitrary persisted payloads in errors. */
  readonly fields: readonly string[];
};
export type RecurrenceDecoded =
  | { readonly supported: true; readonly rule: RecurrenceRule }
  | RecurrenceUnsupported;
export type RecurrenceValues = Readonly<Record<string, number | string | null>>;
export type RecurrenceEncoded =
  | { readonly supported: true; readonly values: RecurrenceValues }
  | RecurrenceUnsupported;

const scalarFields = {
  Interval: "interval",
  OccurrenceCount: "occurrenceCount",
  EndDate: "endDate",
  FirstDayOfTheWeek: "firstDayOfWeek",
} as const;
const selectorFields = {
  DaysOfTheWeek: "daysOfWeek",
  DaysOfTheMonth: "daysOfMonth",
  DaysOfTheYear: "daysOfYear",
  WeeksOfTheYear: "weeksOfYear",
  MonthsOfTheYear: "monthsOfYear",
  SetPositions: "setPositions",
} as const;
const knownFields = new Set([
  "Frequency",
  ...Object.keys(scalarFields),
  ...Object.keys(selectorFields),
]);
const ValuesSchema = Schema.Record(Schema.String, Schema.Unknown);
const unsupported = (
  reason: RecurrenceUnsupported["reason"],
  fields: readonly string[] = [],
): RecurrenceUnsupported => ({
  supported: false,
  reason,
  fields: fields.slice(0, MAX_FIELD_COUNT).map((field) => field.slice(0, 128)),
});

function validateRule(input: unknown): RecurrenceDecoded {
  const decoded = Schema.decodeUnknownResult(RecurrenceRuleSchema, {
    onExcessProperty: "error",
  })(input);
  if (Result.isFailure(decoded)) return unsupported("invalid_rule");
  const rule = decoded.success;
  // RFC 5545 combinations; reject instead of silently dropping selectors.
  if (
    ((rule.occurrenceCount ?? 0) > 0 && rule.endDate != null) ||
    (rule.daysOfMonth?.length && rule.frequency === "weekly") ||
    (rule.daysOfYear?.length &&
      ["daily", "weekly", "monthly"].includes(rule.frequency)) ||
    (rule.weeksOfYear?.length && rule.frequency !== "yearly") ||
    (rule.daysOfWeek?.some((day) => day.weekNumber) &&
      (!["monthly", "yearly"].includes(rule.frequency) || rule.weeksOfYear?.length)) ||
    (rule.setPositions?.length &&
      !Object.values(selectorFields).some(
        (key) => key !== "setPositions" && rule[key]?.length,
      ))
  )
    return unsupported("invalid_rule");
  return { supported: true, rule };
}

/** Supply only recurrence value fields, after separating record identity/link metadata. */
export function decodeRecurrenceValues(input: unknown): RecurrenceDecoded {
  const decoded = Schema.decodeUnknownResult(ValuesSchema)(input);
  if (Result.isFailure(decoded)) return unsupported("invalid_fields");
  const values = decoded.success;
  const keys = Object.keys(values);
  if (keys.length > MAX_FIELD_COUNT) return unsupported("invalid_fields");
  const unknown = keys.filter((key) => !knownFields.has(key));
  if (unknown.length) return unsupported("unknown_fields", unknown);
  const frequency = values.Frequency;
  if (
    typeof frequency !== "number" ||
    !Number.isInteger(frequency) ||
    frequency < 0 ||
    frequency >= frequencies.length
  )
    return unsupported("unknown_frequency", ["Frequency"]);
  const rule: Record<string, unknown> = { frequency: frequencies[frequency] };
  for (const [wire, field] of Object.entries(scalarFields)) {
    if (Object.hasOwn(values, wire)) rule[field] = values[wire];
  }
  for (const [wire, field] of Object.entries(selectorFields)) {
    if (!Object.hasOwn(values, wire)) continue;
    const value = values[wire];
    if (value === null) {
      rule[field] = null;
      continue;
    }
    if (
      typeof value !== "string" ||
      value.length > (MAX_SELECTOR_BYTES * 4) / 3 + 4 ||
      !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(value)
    )
      return unsupported("invalid_selector", [wire]);
    const bytes = Buffer.from(value, "base64");
    if (bytes.length > MAX_SELECTOR_BYTES || bytes.toString("base64") !== value)
      return unsupported("invalid_selector", [wire]);
    try {
      rule[field] = JSON.parse(bytes.toString("utf8"));
    } catch {
      return unsupported("invalid_selector", [wire]);
    }
  }
  return validateRule(rule);
}

/** Does not choose unverified CloudKit field types or merge away unknown fields. */
export function encodeRecurrenceValues(input: unknown): RecurrenceEncoded {
  const decoded = validateRule(input);
  if (!decoded.supported) return decoded;
  const rule = decoded.rule;
  const values: Record<string, number | string | null> = {
    Frequency: frequencies.indexOf(rule.frequency),
  };
  for (const [wire, field] of Object.entries(scalarFields)) {
    if (rule[field] !== undefined) values[wire] = rule[field];
  }
  for (const [wire, field] of Object.entries(selectorFields)) {
    const value = rule[field];
    if (value === undefined) continue;
    if (value === null) {
      values[wire] = null;
      continue;
    }
    const json = JSON.stringify(value);
    if (Buffer.byteLength(json) > MAX_SELECTOR_BYTES)
      return unsupported("invalid_selector", [wire]);
    values[wire] = Buffer.from(json, "utf8").toString("base64");
  }
  return { supported: true, values };
}
