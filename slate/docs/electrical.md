# Electrical

PCB handoff for the Rev A mainboard. The capture plan (parts and signal nets) is held as
data in [`pcb/mainboard/design/board-spec.ts`](../pcb/mainboard/design/board-spec.ts); this
document is the reasoning and the values. Part rationale is in [decisions.md](decisions.md);
purchase state is in [`bom/bom.csv`](../bom/bom.csv).

## Power tree

```text
USB-C VBUS (5 V)
  -> BQ25185 charger + power path (ILIM/VSET 18 kOhm: 4.2 V, 500 mA in; ISET 600 Ohm: ~500 mA)
       BAT <-> 1S protected pouch, pack NTC on TS
       SYS -> TPS63900 buck-boost -> 3.3 V system rail
                                       -> ESP32-H2-MINI-1-H4S
                                       -> TPS22917 load switch -> panel VCI + HV boost network
```

| Block | Part | Values and notes |
| --- | --- | --- |
| Charger | BQ25185DLHR | IN 1 uF 25 V X7R; SYS 10 uF 25 V X7R 1206; BAT 1 uF 10 V. Exposed pad to a copper pour: charging is the board's highest sustained dissipation. |
| Buck-boost | TPS63900DSKR | 2.2 uH shielded, >= 1.5 A (XFL4020 class); 22 uF 10 V X7R 1206 out; 10 uF in. Output set to 3.3 V by its configuration resistors; DVS unused. |
| Panel switch | TPS22917DBVR | ON from GPIO0 with a 100 kOhm pulldown so the panel stays off through reset. Use its quick output discharge so the rail returns to 0 V and the controller sees a clean power-on reset; set CT for a rise time that keeps panel inrush off the 3.3 V rail. |
| ESD | USBLC6-2SC6 | On D+/D-, at the connector. |
| USB-C | USB4105-GF-A | 5.1 kOhm Rd on CC1 and CC2. 22 Ohm series on D+/D- as DNP-able footprints. |
| Battery | JST PH 3-pin right angle | BAT+, NTC, BAT-. TS network matched to the pack thermistor once the pack is chosen. |
| Protection | BQ29700DSER + 2 N-FETs | DNP; populate only for an unprotected cell. |

Expected battery-side sleep current is 30 to 40 uA (H2 light sleep, charger battery mode,
pack protector, divider, converter IQ). Above about 50 uA there is a leakage path to find.

## Panel interface

The SSD2677 is on the panel. The board carries the FPC connector (Hirose
FH12-24S-0.5SH(55), 2.0 mm high, latch accessible) and the HV boost network from Good
Display's reference application circuit: 47 uH inductor (>= 500 mA, shielded), Si1308EDL
switch, three MBR0530 Schottkys, seven 4.7 uF 25 V X7R (1206 for DC-bias margin), three
1 uF 25 V X7R, 1 MOhm and 2.2 Ohm resistors. Copy the topology and values from the
reference sheet for this exact panel at capture time; the list here is the study's reading
of it and must be checked against that sheet.

4-wire SPI (SCLK, SDI, CS, D/C) plus RES and BUSY at 3.3 V; no level shifting.

**Back-powering rule.** While the panel rail is off, no line into the FPC may sit high. A
line at 3.3 V into an unpowered controller feeds its rail through the input protection
diodes, partially powers it, and makes the next power-on reset unreliable.
`PANEL-BACKPOWER` checks this.

| State | Panel rail | SCLK, MOSI, CS, CS2, DC, RST | BUSY |
| --- | --- | --- | --- |
| Reset and ROM boot | Off: EPD_PWR_EN pulled down | High impedance (reset default); no panel line is on a pin the ROM drives | Input |
| Awake or light sleep, panel off | Off | Driven low; light sleep keeps GPIO output state | Input, no pull |
| Panel on | On | Normal SPI | Input; wake source while a waveform runs |
| Deep sleep | Off: EPD_PWR_EN goes high impedance, pulldown holds the switch off | High impedance | High impedance |

High impedance is safe because nothing pulls those lines up. Do not add pull-ups to any
panel line on the board; if the panel's FPC carries its own pull-ups they are on the
switched rail and vanish with it.

## Pin map (provisional)

Mirrored in [`firmware/app/src/board.rs`](../firmware/app/src/board.rs). Confirm every pin
function against the ESP32-H2 and ESP32-H2-MINI-1 datasheets at capture.

| GPIO | Net | Why this pin |
| --- | --- | --- |
| 0 | EPD_PWR_EN | Not a strapping pin; 100 kOhm pulldown keeps the panel off at boot |
| 1 | VBAT_SENSE | ADC1 channel 0 |
| 4 | CHG_STAT | Charger status, open drain with pull-up |
| 5 | EPD_CS2_N | Reserved until G-01 says whether the panel has a second chip select |
| 10 | EPD_SCLK | |
| 11 | EPD_MOSI | |
| 12 | USB_PRESENT | VBUS through 100 kOhm over 150 kOhm: 3.0 V at 5.0 V, 2.85 V at 4.75 V (above the 2.475 V input-high threshold), 3.3 V at 5.5 V. Draws only while USB is plugged in. In the H2's deep-sleep wake range (GPIO7 to 14), so plugging in USB wakes a deep-sleeping device |
| 13 | EPD_CS_N | |
| 14 | EPD_DC | |
| 22 | EPD_RST_N | |
| 23 | EPD_BUSY | UART0 RX by default: an input during boot, so the ROM never drives it. Light-sleep wake source |

Kept free: GPIO2, 3, 8, 9 and 25 (strapping; GPIO9 is the BOOT pad), GPIO26/27 (USB
D-/D+), and GPIO24 (UART0 TX, which the ROM drives with its boot log; it would back-power
the panel if wired to it). GPIO24 goes to a test pad as a fallback log output.

GPIO12 and 13 are the H2's optional 32 kHz crystal pins, unused as such since Slate has no
external crystal. Confirm the deep-sleep wake range for GPIO12 in the H2 datasheet; if it
differs, USB_PRESENT moves to a pin that can wake the chip.

VBAT_SENSE: 3.3 MOhm over 1.0 MOhm, so 4.2 V reads as 0.98 V, with 10 nF from the ADC pin
to ground as the charge reservoir for the sampling capacitor (the divider's source
impedance is about 770 kOhm).

## Test points

VBAT, VSYS, 3V3, 3V3_EPD, GND, EN, BOOT (GPIO9), TX0 (GPIO24), CHG_STAT. A current
measurement path at the battery connector (a series jumper or a 0 Ohm link) so
`BOARD-SLEEP-CURRENT` measures the whole board, not the module.

## Layout constraints

- ESP32-H2-MINI-1 antenna on a board edge, with Espressif's keepout clear of copper, the
  pouch and the panel's metal backplane where possible.
- Two layers, 1.0 to 1.2 mm thick, components top side only, nothing taller than 2.5 mm
  except the USB-C receptacle (about 3.3 mm) at the edge; the stack (excluding battery)
  under 4.5 mm.
- USB-C on an accessible edge. No electrolytics; MLCCs and shielded SMD inductors only.
- Keep the TPS63900 and HV boost loops tight; the HV network sits beside the FPC connector.
- Parts were chosen for paste and hot-plate assembly: WSON parts are the only ones that
  are hard to hand-iron.
