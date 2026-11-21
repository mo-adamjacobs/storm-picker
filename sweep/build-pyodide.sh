#!/usr/bin/env bash
# Build the lightning_sweep wheel for Pyodide into lightning/static/wheels/.
# Needs (all user-level): uv tool install pyodide-cli --with pyodide-build;
# pyodide xbuildenv install <version matching worker.js>; pyodide xbuildenv install-emscripten;
# rustup toolchain install "$(pyodide config get rust_toolchain)" + target wasm32-unknown-emscripten.
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/.local/bin:$PATH"
emsdk="$(pyodide config get emscripten_dir)/../.."
# shellcheck disable=SC1091
source "$emsdk/emsdk_env.sh" > /dev/null 2>&1
export RUSTUP_TOOLCHAIN="$(pyodide config get rust_toolchain)"
out=../lightning/static/wheels
mkdir -p "$out"
rm -f "$out"/lightning_sweep-*.whl
pyodide build --outdir "$out"
whl="$(cd "$out" && ls lightning_sweep-*.whl | head -1)"
printf '{"lightning_sweep": "%s"}\n' "$whl" > "$out/manifest.json"
ls -l "$out"
