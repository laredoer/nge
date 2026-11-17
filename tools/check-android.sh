#!/bin/sh
# 检查引擎全部 crate 和能上 Android 的示例编得成 aarch64-linux-android（NGE_ANDROID_X86_64=1 时加上 x86_64-linux-android），
# 再编一次 Java 运行时、跑运行时的测试（见 docs/android.md）。C 依赖（ring、zstd 等）用 NDK 的 clang。
set -e
cd "$(dirname "$0")/.."
ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
[ -d "$ANDROID_HOME" ] || { echo "找不到 Android SDK：设 ANDROID_HOME" >&2; exit 1; }
# 最新的一个（版本号按数字排，和 nsh-android.sh 一样）
newest() { ls "$1" 2>/dev/null | sort -t. -k1,1n -k2,2n -k3,3n | tail -n 1; }
if [ -z "$ANDROID_NDK_HOME" ]; then
  ndk="$(newest "$ANDROID_HOME/ndk")"
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
# Rust 1.99.0 的 macOS 工具链里 rust-objcopy 起不来，编构建脚本时会报「stripping debug info with rust-objcopy failed」：
# 只是警告，构建脚本没剥调试信息、照样能用。设 DYLD_FALLBACK_LIBRARY_PATH 或 build-override 的 strip 都压不住（见 docs/android.md）
packages=""
for dir in crates/nge-*; do
  # 中继是服务器程序，不上手机
  [ "$(basename "$dir")" = nge-relay ] && continue
  packages="$packages -p $(basename "$dir")"
done
# 默认只查 aarch64（真机和 arm64 模拟器）；NGE_ANDROID_X86_64=1 时再查 x86_64（x86 模拟器），多编一遍
targets="aarch64-linux-android"
[ "$NGE_ANDROID_X86_64" = 1 ] && targets="$targets x86_64-linux-android"
for target in $targets; do
  cargo check --target "$target" $packages -p platformer -p sanguo "$@"
done
tools/nsh-android.sh --runtime-only
# 运行时自己的测试（JNI 注册表对得上 Java、JNI 入口不让 panic 漏出去）：在它自己的目录里跑，跑完删掉生成的 Cargo.lock
(cd vendor/nsh-platform-android && cargo test --no-default-features --test native; status=$?; rm -f Cargo.lock; exit $status)
