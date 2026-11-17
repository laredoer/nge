#!/bin/sh
# 检查引擎全部 crate 和能上 Android 的示例编得成 aarch64-linux-android、x86_64-linux-android，
# 再编一次 Java 运行时（见 docs/android.md）。C 依赖（ring、zstd 等）用 NDK 的 clang。
set -e
cd "$(dirname "$0")/.."
ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
if [ -z "$ANDROID_NDK_HOME" ]; then
  ndk="$(ls "$ANDROID_HOME/ndk" 2>/dev/null | sort -t. -k1,1n | tail -n 1)"
  [ -n "$ndk" ] || { echo "SDK 里没有 NDK：sdkmanager \"ndk;27.3.13750724\"" >&2; exit 1; }
  ANDROID_NDK_HOME="$ANDROID_HOME/ndk/$ndk"
fi
bin="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/darwin-x86_64/bin"
[ -d "$bin" ] || bin="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin"
# 和 nsh 一样按 API 28 编
export CC_aarch64_linux_android="$bin/aarch64-linux-android28-clang"
export CXX_aarch64_linux_android="$bin/aarch64-linux-android28-clang++"
export AR_aarch64_linux_android="$bin/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$CC_aarch64_linux_android"
export CC_x86_64_linux_android="$bin/x86_64-linux-android28-clang"
export CXX_x86_64_linux_android="$bin/x86_64-linux-android28-clang++"
export AR_x86_64_linux_android="$bin/llvm-ar"
export CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER="$CC_x86_64_linux_android"
# Rust 1.99.0 的 macOS 工具链里 rust-objcopy 找不到 libLLVM.dylib（见 docs/web.md）：让它到工具链的 lib 里找
DYLD_FALLBACK_LIBRARY_PATH="$(rustc --print sysroot)/lib${DYLD_FALLBACK_LIBRARY_PATH:+:$DYLD_FALLBACK_LIBRARY_PATH}"
export DYLD_FALLBACK_LIBRARY_PATH
packages=""
for dir in crates/nge-*; do
  # 中继是服务器程序，不上手机
  [ "$(basename "$dir")" = nge-relay ] && continue
  packages="$packages -p $(basename "$dir")"
done
for target in aarch64-linux-android x86_64-linux-android; do
  cargo check --target "$target" $packages -p platformer -p sanguo "$@"
done
tools/nsh-android.sh --runtime-only
