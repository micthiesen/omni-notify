# Build sequence

The PCB does not depend on the battery capacity or the measured energy numbers: the pouch
plugs into a connector and every firmware fallback runs on the same hardware. So the bench
spikes and the PCB proceed in parallel, and only the pouch purchase waits for measurements.

## Phase 0: bench spikes (dev kits)

Parts: two ESP32-H2 DevKits, a GDEY133F91 panel, a DESPI-C02 adapter, the OTBR RCP, and a
current analyser.

1. OTBR on boris with the RCP (G-03).
2. Panel bring-up on DESPI-C02: pinout, chip selects, BUSY polarity, scan direction, ink
   codes (G-01). Test patterns over the USB console.
3. Firmware: Thread SED attach with the `openthread` crate; light-sleep floor (G-02).
4. Energy: unchanged check, light vs deep sleep, frame transfer at three payload sizes,
   full and fast waveforms.

## Phase 1: mainboard

Capture in tscircuit after G-01 ([`pcb/README.md`](../pcb/README.md)), then the KiCad
handoff, routing and fabrication with the procedures ported from crystal-shim. Order
boards and the board BOM together.

## Phase 2: bring-up

Power tree, sleep current at the battery connector, back-powering, panel power cycling,
charging thermals, USB stay-awake, provisioning.

## Phase 3: battery and enclosure

Choose the pouch from measured energy (G-04), then design the enclosure around it.

## Procurement gates

| Purchase | Waits for |
| --- | --- |
| Dev kits, DESPI-C02, RCP | Nothing |
| GDEY133F91 | Stock confirmation |
| Mainboard PCB and board parts | G-01 and capture review |
| Pouch | G-04 |
