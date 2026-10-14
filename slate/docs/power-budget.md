# Power budget

The energy model from the design study, checked arithmetically. Specified values come from
datasheets ([sources.md](sources.md)); everything else is an assumption that a test matrix
row replaces with a measurement.

## Inputs

| Item | Value | Basis |
| --- | --- | --- |
| Cell nominal voltage | 3.7 V | 1S LiPo |
| Usable fraction of rated capacity | 80% | Reserve for tolerance, ageing, temperature, self-discharge, model error |
| 3.3 V conversion efficiency | 90% | Conservative; TPS63900 specifies over 90% at 10 uA |
| H2 light sleep (lowest) | 25 uA at 3.3 V | Datasheet |
| H2 deep sleep | 7 uA at 3.3 V | Datasheet |
| BQ25185 battery-only | 4 uA | Datasheet |
| Pack protection | 4 uA | BQ2970-class allowance |
| Battery divider | 0.86 uA | 3.7 V / 4.3 MOhm |
| TPS63900 IQ / TPS22917 off | 0.075 uA / 0.01 uA | Datasheets |
| 802.15.4 TX at -2 dBm / RX | 38 mA / 29 mA | Datasheet |
| Panel full refresh | 108 mW x 25 s = 2.70 J | Datasheet typical power and duration |
| Panel fast refresh | 108 mW x 20 s = 2.16 J | Duration specified; power assumed equal |

## Fixed overhead

Light-sleep floor at the cell: 25 uA x 3.3 V / (3.7 V x 0.9) = 24.8 uA, plus charger,
protection, divider and converter, about 34 uA. That is 34 uA x 3.7 V x 8760 h = 1.10 Wh
per year.

Unchanged check, nominal: 100 ms of CPU at 8 mA (2.64 mJ), 4 ms TX (0.50 mJ), 8 ms RX
(0.77 mJ), 0.6 mJ margin: 4.5 mJ at the rail, about 5 mJ from the cell. At 1,440 checks per
day: 7.2 J/day, 0.73 Wh/year.

Fixed total: about **1.8 Wh/year**. Tripling the check estimate still leaves polling small
next to refreshes at 50 per day or more.

## Per refresh

Panel 2.70 J / 0.9 = 3.00 J from the cell, plus 0.8 J for transfer (about 50 kB at 80
kbit/s application throughput), controller initialisation and SPI: **3.8 J per refresh**.

## Battery requirement

Yearly energy = 1.8 Wh + N x 365 x 3.8 J / 3600. Rated capacity = energy / (3.7 V x 0.8).

| Refreshes/day | 6 months | 1 year |
| ---: | ---: | ---: |
| 0 | 0.30 Ah | 0.61 Ah |
| 10 | 0.95 Ah | 1.91 Ah |
| 50 | 3.56 Ah | 7.12 Ah |
| 100 | 6.81 Ah | 13.62 Ah |

Life at 100 refreshes/day, nominal model: 8 Ah 214 days, 10 Ah 268 days, 12 Ah 321 days,
**15 Ah 402 days**, 20 Ah 536 days.

## Sensitivity

| Case | Fixed Wh/yr | J/refresh | 1 yr @10/day | @50/day | @100/day |
| --- | ---: | ---: | ---: | ---: | ---: |
| Best plausible | 1.0 | 3.0 | 1.37 Ah | 5.48 Ah | 10.61 Ah |
| Nominal | 1.8 | 3.8 | 1.91 Ah | 7.12 Ah | 13.62 Ah |
| Conservative raw/slow transfer | 2.7 | 5.6 | 2.83 Ah | 10.50 Ah | 20.09 Ah |

The two uncertainties that matter are frame-transfer energy and whether fast refresh is
acceptable. Slate's own levers, in order of size: fewer refreshes (server policy), a
smaller encoded frame, then the fast waveform. Sub-microamp board work comes last.

## What replaces these numbers

| Assumption | Test matrix row |
| --- | --- |
| Check energy, light vs deep sleep | `SPIKE-CHECK-ENERGY`, `SPIKE-LIGHT-VS-DEEP` |
| Transfer energy | `SPIKE-TRANSFER-ENERGY` |
| Panel energy | `PANEL-FULL-ENERGY`, `PANEL-FAST-ENERGY` |
| Board sleep current | `BOARD-SLEEP-CURRENT` |
