#!/usr/bin/env bash
# Build the engine to WebAssembly and put it next to the page: web/engine.wasm.
# Needs Rust with the wasm32-unknown-unknown target (rustup target add wasm32-unknown-unknown).
# Then serve web/ with any static file server, e.g. `python3 -m http.server -d web`.
set -euo pipefail
cd "$(dirname "$0")"
cargo build --manifest-path engine/Cargo.toml --release --target wasm32-unknown-unknown
cp engine/target/wasm32-unknown-unknown/release/storm_engine.wasm web/engine.wasm
ls -l web/engine.wasm
