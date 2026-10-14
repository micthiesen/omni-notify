# PCB

tscircuit source for the Rev A mainboard. Requirements are in
[../docs/electrical.md](../docs/electrical.md).

| Board | State |
| --- | --- |
| `mainboard/` | Placeholder outline plus the capture plan (`design/board-spec.ts`: parts and signal nets). Capture waits on gate G-01. |

Tooling matches crystal-shim and stillair: Bun 1.3.11, Node 24 for `tsci`, TypeScript
5.9.3 (TypeScript 7 breaks tscircuit's Rollup plugin), tscircuit 0.0.2465, oxlint and
oxfmt. `bun.lock` is crystal-shim's lock, so the transitive tscircuit packages are the
verified set; a fresh resolve pulls an incompatible `circuit-json`. `circuit-json-to-kicad`
and `polygon-clipping` are unused until the KiCad handoff and stay for lock parity.

```bash
bun install --frozen-lockfile
bun run check && bun run typecheck
bun run check:mainboard && bun run test
bun run dev:mainboard           # local viewer
```

Before capture, port crystal-shim's `$pcb`, `$konnect` and `$kicad-manufacture` skills and
its handoff tools with `$sync`. tscircuit owns schematic, specification and placement;
KiCad owns routing and declared fabrication additions; `.kicad_*` files are never edited as
text.
