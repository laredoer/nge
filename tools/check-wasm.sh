#!/bin/sh
# 检查引擎全部 crate 和能上 web 的示例编得成 wasm32-unknown-unknown（见 docs/web.md）。
# zstd-sys 要 llvm-ar：没在 PATH 里时用 Homebrew 的 llvm@20（可以用 LLVM_BIN 指定别的目录）。
set -e
cd "$(dirname "$0")/.."
if ! command -v llvm-ar >/dev/null 2>&1; then
  LLVM_BIN="${LLVM_BIN:-$(brew --prefix llvm@20 2>/dev/null)/bin}"
  PATH="$LLVM_BIN:$PATH"
fi
command -v llvm-ar >/dev/null 2>&1 || { echo "找不到 llvm-ar：brew install llvm@20，或设 LLVM_BIN" >&2; exit 1; }
packages=""
for dir in crates/nge-*; do
  # 中继是服务器程序（tokio 的网络），不上浏览器
  [ "$(basename "$dir")" = nge-relay ] && continue
  packages="$packages -p $(basename "$dir")"
done
cargo check --target wasm32-unknown-unknown $packages -p sanguo "$@"
# 单独编示例：只开它自己选的 nge-scene 功能（和上面一起编时功能会合并，看不出缺了什么）
cargo check --target wasm32-unknown-unknown -p sanguo "$@"
