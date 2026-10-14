import { boardSpec } from "./board-spec";

// Rev A mainboard. Outline only: capture starts after the panel FPC pinout gate
// (docs/decisions.md D-01) and follows the plan in board-spec.ts.
export default () => (
  <board width={`${boardSpec.widthMm}mm`} height={`${boardSpec.heightMm}mm`} />
);
