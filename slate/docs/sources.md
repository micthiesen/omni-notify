# Sources

Every specified number in the dossier comes from one of these documents, as read by the
2026-10 design study. None has been re-read against the primary document yet; mark a row
verified when someone does, and fix the docs if a value differs.

| Document | Values used | Verified |
| --- | --- | --- |
| Good Display GDEY133F91 product page and specification | 960 x 680, four inks, SSD2677, SPI, 24-pin FPC, 25 s full / 20 s fast, 108 mW typical refresh, VCI 2.3 to 3.6 V, 36 mA operating, about 20 uA sleep with RAM, about 1 uA deep sleep, outline and active area, about US$152 | No |
| Good Display GDEY133F91 reference application circuit | HV boost network parts and values | No |
| SSD2677 datasheet | 3-wire and 4-wire SPI | No |
| Espressif ESP32-H2-MINI-1 datasheet | 4 MB H4S variant, 320 KB SRAM, GPIO count, USB Serial/JTAG, module size, 7 uA deep sleep, 25 uA light sleep, 29 mA RX, 38 mA TX at -2 dBm, 54 mA at +7 dBm, 123 mA at +18 dBm | No |
| Espressif ESP32-H2 datasheet | Strapping pins, ADC channels, UART0 and USB pin assignment (pin map) | No |
| TI TPS63900 datasheet | 1.8 to 5.5 V in, 75 nA IQ, over 90% at 10 uA, over 400 mA at 3.3 V, 2.2 uH, 22 uF out | No |
| TI BQ25185 datasheet | 4 uA battery-only, 100/500/1100 mA input limits, 18 kOhm and 600 Ohm settings, capacitor recommendations | No |
| TI TPS22917 datasheet | 1 to 5.5 V, 2 A, 80 mOhm, 0.5 uA on, 10 nA off, CT and QOD | No |
| TI BQ2970 datasheet | Protection functions, about 4 uA | No |
| ST USBLC6-2SC6 datasheet | USB 2.0 ESD protection | No |
| GCT USB4105 datasheet | 16-contact USB 2.0 Type-C, 3.31 mm profile | No |
| Hirose FH12 datasheet | 24-pin 0.5 mm, 2.0 mm high | No |
| esp-rs `openthread` 0.4 README and source | Prebuilt riscv32imac libraries, MTD/SED, link mode and poll period API, no CSL | Yes, 2026-10-14 (crate source) |
