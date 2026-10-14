# Firmware

Contract in [../docs/firmware.md](../docs/firmware.md); wire format in
[../docs/protocol.md](../docs/protocol.md).

```bash
cargo test                      # host workspace: slate-protocol + slate-core
cargo fmt && cargo clippy --all-targets

cd app                          # ESP32-H2 binary, own workspace and target
cargo build --release
cargo run --release             # espflash flash --chip esp32h2 --monitor (needs a board)
```
