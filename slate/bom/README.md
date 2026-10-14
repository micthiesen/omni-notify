# BOM

[`bom.csv`](bom.csv) is the parts list with purchase tracking. Columns follow stillair:

- **Design status**: the state of the selection (selected, provisional, gated, DNP), with
  gates from [../docs/decisions.md](../docs/decisions.md).
- **Purchase status**: `Not ordered`, `Ordered (<vendor> <order>)`, `Received <date>`.
- **Order ref**: vendor part or order code.
- **Notes**: sourcing detail, alternates, dated price and stock snapshots. Re-verify
  before ordering.

Passives are one placeholder line until capture fixes their packages. Purchases wait on
the gates in [../docs/build.md](../docs/build.md#procurement-gates).
