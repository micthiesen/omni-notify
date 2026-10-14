# State

Last updated: **2026-10-14** (project set up).

## Now

- **Project scaffolded from the 2026-10 design study.** Dossier, BOM, test matrix,
  firmware workspaces and the tscircuit PCB project exist. Nothing is purchased, captured
  or implemented.
- **Departures from the study** are recorded in [decisions.md](decisions.md): bare-metal
  Rust with the esp-rs `openthread` crate instead of ESP-IDF (D-03), an Omni-owned OTBR on
  boris (D-05), server-directed check intervals and refresh policy (D-07, D-09),
  verify-before-waveform (D-10), rx-on-when-idle transfers (D-11), stay awake on USB
  (D-16), and a pin map that keeps panel lines off the H2 ROM's UART0 TX.
- **No OTA** by owner decision (D-04).

## Next

**Order the bench kit and run Phase 0** ([build.md](build.md)): two H2 DevKits, the
GDEY133F91, a DESPI-C02 and an RCP dongle. Gate G-01 (panel pinout, chip selects, ink
codes) blocks PCB capture, and G-02 (bare-metal SED light sleep) is the largest firmware
risk; both need only dev kits. Purchasing needs Michael.

## Candidates Not Chosen

- **Capture the mainboard in tscircuit now:** waits on G-01; the FPC pinout and
  chip-select count change the connector wiring and pin map. First port crystal-shim's
  `$pcb`, `$konnect` and `$kicad-manufacture` skills and handoff tools.
- **Implement `slate-protocol`'s codec and the server encoder:** startable now with no
  hardware, but ink codes and scan direction are provisional until G-01.
- **Stand up OTBR on boris:** needs the RCP dongle (G-03).
- **Design the dashboard content:** independent server-side work; sets the refresh policy
  numbers.

## Learned Recently

- esp-rom-sys 0.1.5 (needed by esp-storage 0.10 through esp-bootloader-esp-idf 0.6)
  requires Rust 1.95; the firmware `rust-version` is 1.95 so the resolver picks it.
- USB_PRESENT sits on GPIO12 because the H2 wakes from deep sleep only on GPIO7 to 14
  (to confirm in the datasheet); [electrical.md](electrical.md#pin-map-provisional).
- The PCB `bun.lock` is crystal-shim's, so tscircuit's transitive pins match; a fresh
  resolve of tscircuit 0.0.2465 pulls an incompatible `circuit-json`.
