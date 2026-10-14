# Testing

[`test-matrix.csv`](test-matrix.csv) holds every acceptance check with its method, limit
and result. A row passes only with real evidence: fill `Result` with the measured value
and where the evidence lives, plus `Owner` and `Date`. Simulation and host tests never pass
a hardware row. When a measured value replaces a provisional number, update the doc that
held it in the same change.

The `SPIKE-*` and `PANEL-*` rows run on dev kits before the mainboard exists; the rest need
the assembled board.
