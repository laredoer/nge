#!/bin/sh
# 带上 web 构建要的环境跑 nsh（见 docs/web.md），比如：tools/nsh-web.sh package --target web -o out examples/sanguo
# - zstd-sys 要 llvm-ar：没在 PATH 里时用 Homebrew 的 llvm@20（可以用 LLVM_BIN 指定别的目录）。
# - Rust 1.99.0 的 macOS 工具链里 rust-lld 找不到 libLLVM.dylib：跑不起来时让它到工具链的 lib 目录里找。
set -e
if ! command -v llvm-ar >/dev/null 2>&1; then
  LLVM_BIN="${LLVM_BIN:-$(brew --prefix llvm@20 2>/dev/null)/bin}"
  PATH="$LLVM_BIN:$PATH"
  export PATH
fi
command -v llvm-ar >/dev/null 2>&1 || { echo "找不到 llvm-ar：brew install llvm@20，或设 LLVM_BIN" >&2; exit 1; }
sysroot="$(cd "$(dirname "$0")/.." && rustc --print sysroot)"
lld="$(find "$sysroot/lib/rustlib" -path '*/bin/rust-lld' | head -n 1)"
if [ -n "$lld" ] && ! "$lld" -flavor wasm --version >/dev/null 2>&1; then
  DYLD_FALLBACK_LIBRARY_PATH="$sysroot/lib${DYLD_FALLBACK_LIBRARY_PATH:+:$DYLD_FALLBACK_LIBRARY_PATH}"
  export DYLD_FALLBACK_LIBRARY_PATH
fi
exec nsh "$@"
