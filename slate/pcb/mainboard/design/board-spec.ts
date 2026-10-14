// Mainboard capture plan. This is the agreed part list and connectivity intent from
// docs/electrical.md, held as data so capture can be checked against it. It is not a
// schematic: nothing here is captured, footprint-verified or placed yet.

export type Status = "selected" | "provisional" | "dnp";

export interface PartPlan {
  ref: string;
  mpn: string;
  role: string;
  package: string;
  status: Status;
}

// Outline is a placeholder until the panel FPC tail position and the battery envelope are
// fixed (docs/mechanical.md). The ESP32-H2 antenna end must sit on a board edge.
export const boardSpec = {
  widthMm: 90,
  heightMm: 45,
  thicknessMm: 1.0,
  layers: 2,
  // Every part except the USB-C receptacle (about 3.3 mm, at the board edge).
  maxComponentHeightMm: 2.5,
} as const;

export const parts: readonly PartPlan[] = [
  {
    ref: "U1",
    mpn: "ESP32-H2-MINI-1-H4S",
    role: "MCU and 802.15.4 radio",
    package: "MINI-1 module",
    status: "selected",
  },
  {
    ref: "U2",
    mpn: "TPS63900DSKR",
    role: "3.3 V buck-boost",
    package: "WSON-10",
    status: "selected",
  },
  {
    ref: "U3",
    mpn: "BQ25185DLHR",
    role: "Charger and power path",
    package: "WSON-10",
    status: "selected",
  },
  {
    ref: "U4",
    mpn: "TPS22917DBVR",
    role: "Panel rail load switch",
    package: "SOT-23-6",
    status: "selected",
  },
  {
    ref: "U5",
    mpn: "USBLC6-2SC6",
    role: "USB D+/D- ESD",
    package: "SOT-23-6",
    status: "selected",
  },
  {
    ref: "U6",
    mpn: "BQ29700DSER",
    role: "Board-side cell protection",
    package: "WSON-6",
    status: "dnp",
  },
  {
    ref: "Q1",
    mpn: "SI1308EDL-T1-GE3",
    role: "Panel HV boost switch",
    package: "SC-70-3",
    status: "selected",
  },
  {
    ref: "D1",
    mpn: "MBR0530",
    role: "Panel HV rectifier",
    package: "SOD-123",
    status: "selected",
  },
  {
    ref: "D2",
    mpn: "MBR0530",
    role: "Panel HV rectifier",
    package: "SOD-123",
    status: "selected",
  },
  {
    ref: "D3",
    mpn: "MBR0530",
    role: "Panel HV rectifier",
    package: "SOD-123",
    status: "selected",
  },
  {
    ref: "L1",
    mpn: "XFL4020-222ME",
    role: "Buck-boost inductor 2.2 uH",
    package: "4 x 4 mm",
    status: "provisional",
  },
  {
    ref: "L2",
    mpn: "SRN4018-470M",
    role: "Panel HV inductor 47 uH",
    package: "4 x 4 mm",
    status: "provisional",
  },
  {
    ref: "J1",
    mpn: "USB4105-GF-A",
    role: "USB-C charge and USB Serial/JTAG",
    package: "USB-C 16-pin",
    status: "selected",
  },
  {
    ref: "J2",
    mpn: "FH12-24S-0.5SH(55)",
    role: "Panel FPC",
    package: "24-pin 0.5 mm ZIF",
    status: "selected",
  },
  {
    ref: "J3",
    mpn: "S3B-PH-SM4-TB",
    role: "Battery BAT+, NTC, BAT-",
    package: "JST PH 3-pin RA",
    status: "provisional",
  },
];

// Signal nets the capture must contain. Endpoints use datasheet signal names, not pin
// numbers; capture resolves them. Passive-only nets (dividers, HV network, decoupling)
// follow docs/electrical.md and are not listed.
export const nets: Readonly<Record<string, readonly string[]>> = {
  VBUS: ["J1.VBUS", "U3.IN", "U5.VBUS"],
  VSYS: ["U3.SYS", "U2.VIN"],
  VBAT: ["U3.BAT", "J3.BAT+"],
  V3V3: ["U2.VOUT", "U1.3V3", "U4.VIN"],
  V3V3_EPD: ["U4.VOUT", "J2.VCI", "L2.1"],
  EPD_PWR_EN: ["U1.GPIO0", "U4.ON"],
  EPD_SCLK: ["U1.GPIO10", "J2.SCLK"],
  EPD_MOSI: ["U1.GPIO11", "J2.SDI"],
  EPD_CS_N: ["U1.GPIO13", "J2.CS"],
  EPD_DC: ["U1.GPIO14", "J2.DC"],
  EPD_RST_N: ["U1.GPIO22", "J2.RES"],
  EPD_BUSY: ["U1.GPIO23", "J2.BUSY"],
  EPD_GDR: ["J2.GDR", "Q1.G"],
  USB_DP: ["J1.DP", "U5.IO1", "U1.GPIO27"],
  USB_DN: ["J1.DN", "U5.IO2", "U1.GPIO26"],
  NTC: ["J3.NTC", "U3.TS"],
  // Charger status output; confirm the BQ25185 pin name at capture.
  CHG_STAT: ["U3.STATUS", "U1.GPIO4"],
  VBAT_SENSE: ["U1.GPIO1"],
  USB_PRESENT: ["U1.GPIO12"],
  EPD_CS2_N: ["U1.GPIO5"],
  TP_EN: ["U1.EN"],
  TP_BOOT: ["U1.GPIO9"],
  TP_TX0: ["U1.GPIO24"],
  BAT_N: ["J3.BAT-", "U3.GND"],
};
