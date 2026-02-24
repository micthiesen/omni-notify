import type { Logger } from "@micthiesen/mitools/logging";
import { describe, expect, it, vi } from "vitest";
import { resolveCarrierCode } from "./carrierMap.js";

// Subset of real Parcel carrier list for testing
const MOCK_CARRIERS: Record<string, string> = {
  ups: "UPS",
  fedex: "FedEx",
  usps: "USPS",
  dhl: "DHL Express",
  dhlgm: "DHL Global Mail",
  amzlus: "Amazon US",
  amzlca: "Amazon Canada",
  ont: "OnTrac",
  laser: "OnTrac - Lasership",
  cp: "Canada Post",
  canpar: "Canpar",
  puro: "Purolator",
  intelc: "Dragonfly - Intelcom",
  loom: "Loomis Express",
  dicom: "GLS Canada",
  nationex: "Nationex",
  ics: "ICS Courier",
  rm: "Royal Mail",
  au: "Australia Post",
  pbi: "PBI - Pitney Bowes",
};

vi.mock("got", () => ({
  default: {
    __esModule: true,
    default: () => ({
      json: () => Promise.resolve(MOCK_CARRIERS),
    }),
  },
}));

// got is mocked at module level but we need to access the actual mock.
// The mock structure above means got(url).json() works.
// However, got is a default export function, let's fix the mock:
vi.mock("got", () => {
  const gotFn = () => ({
    json: () => Promise.resolve(MOCK_CARRIERS),
  });
  return { default: gotFn };
});

const mockLogger = {
  debug: vi.fn(),
  info: vi.fn(),
  warn: vi.fn(),
  error: vi.fn(),
  extend: vi.fn(),
} as unknown as Logger;

describe("resolveCarrierCode", () => {
  it("should match by exact code", async () => {
    expect(await resolveCarrierCode("ups", mockLogger)).toEqual({
      resolved: true,
      carrierCode: "ups",
    });
    expect(await resolveCarrierCode("fedex", mockLogger)).toEqual({
      resolved: true,
      carrierCode: "fedex",
    });
  });

  it("should match by exact name (case-insensitive)", async () => {
    expect(await resolveCarrierCode("FedEx", mockLogger)).toEqual({
      resolved: true,
      carrierCode: "fedex",
    });
    expect(await resolveCarrierCode("Canada Post", mockLogger)).toEqual({
      resolved: true,
      carrierCode: "cp",
    });
    expect(await resolveCarrierCode("canpar", mockLogger)).toEqual({
      resolved: true,
      carrierCode: "canpar",
    });
    expect(await resolveCarrierCode("Loomis Express", mockLogger)).toEqual({
      resolved: true,
      carrierCode: "loom",
    });
  });

  it("should match by substring", async () => {
    // "DHL" is a substring of "DHL Express"
    expect(await resolveCarrierCode("DHL", mockLogger)).toEqual({
      resolved: true,
      carrierCode: "dhl",
    });
    // "Purolator" exact name match
    expect(await resolveCarrierCode("Purolator", mockLogger)).toEqual({
      resolved: true,
      carrierCode: "puro",
    });
  });

  it("should prefer shorter name on substring match", async () => {
    // "UPS" matches "UPS" (3 chars) not some longer name
    expect(await resolveCarrierCode("UPS", mockLogger)).toEqual({
      resolved: true,
      carrierCode: "ups",
    });
  });

  it("should match by token overlap", async () => {
    // "Amazon" shares a token with "Amazon US" and "Amazon Canada"
    // Should pick shortest name → "Amazon US"
    expect(await resolveCarrierCode("Amazon", mockLogger)).toEqual({
      resolved: true,
      carrierCode: "amzlus",
    });
    // "Intelcom" shares token with "Dragonfly - Intelcom"
    expect(await resolveCarrierCode("Intelcom", mockLogger)).toEqual({
      resolved: true,
      carrierCode: "intelc",
    });
  });

  it("should be case-insensitive", async () => {
    expect(await resolveCarrierCode("FEDEX", mockLogger)).toEqual({
      resolved: true,
      carrierCode: "fedex",
    });
    expect(await resolveCarrierCode("canada post", mockLogger)).toEqual({
      resolved: true,
      carrierCode: "cp",
    });
  });

  it("should trim whitespace", async () => {
    expect(await resolveCarrierCode("  UPS  ", mockLogger)).toEqual({
      resolved: true,
      carrierCode: "ups",
    });
  });

  it("should return resolved: false for unknown carriers", async () => {
    expect(await resolveCarrierCode("Totally Fake Carrier", mockLogger)).toEqual({
      resolved: false,
    });
  });
});
