#!/bin/sh
# 带上我们改过的 Android 运行时跑 nsh（见 docs/android.md），比如：
#   tools/nsh-android.sh package --target android --abi arm64-v8a -o out examples/platformer
# nsh 打 APK 时用运行时目录里的 classes.dex：这里先用 javac + d8 把 vendor/nsh-platform-android/java 编成它
# （和 nsh 的 `cargo xtask android` 一样的步骤，不用 Gradle），再让 NSH_RUNTIME 指过去。
# 只编 Java 不跑 nsh：tools/nsh-android.sh --runtime-only
set -e
root="$(cd "$(dirname "$0")/.." && pwd)"
ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
[ -d "$ANDROID_HOME" ] || { echo "找不到 Android SDK：设 ANDROID_HOME" >&2; exit 1; }
export ANDROID_HOME
# 最新的一个（版本号按数字排）
newest() { ls "$1" 2>/dev/null | sort -t. -k1,1n -k2,2n -k3,3n | tail -n 1; }
tools="$(newest "$ANDROID_HOME/build-tools")"
platform="$ANDROID_HOME/platforms/android-35/android.jar"
[ -n "$tools" ] || { echo "SDK 里没有 build-tools：sdkmanager \"build-tools;35.0.0\"" >&2; exit 1; }
[ -f "$platform" ] || { echo "SDK 里没有 android-35：sdkmanager \"platforms;android-35\"" >&2; exit 1; }
if [ -z "$ANDROID_NDK_HOME" ]; then
  ndk="$(newest "$ANDROID_HOME/ndk")"
  [ -n "$ndk" ] || { echo "SDK 里没有 NDK：sdkmanager \"ndk;27.3.13750724\"" >&2; exit 1; }
  ANDROID_NDK_HOME="$ANDROID_HOME/ndk/$ndk"
fi
export ANDROID_NDK_HOME

java_src="$root/vendor/nsh-platform-android/java"
runtime="$root/target/nge-android/runtime"
dex="$runtime/android/release/classes.dex"
# 有比 classes.dex 新的 Java 源文件（或者还没有）才重编
if [ ! -f "$dex" ] || [ -n "$(find "$java_src" -newer "$dex" -print -quit)" ]; then
  build="$root/target/nge-android/java"
  rm -rf "$build"
  mkdir -p "$build/classes"
  find "$java_src" -name '*.java' > "$build/sources.txt"
  javac --release 17 -Xlint:-options -encoding UTF-8 -classpath "$platform" -d "$build/classes" @"$build/sources.txt"
  (cd "$build/classes" && jar cf "$build/classes.jar" .)
  "$ANDROID_HOME/build-tools/$tools/d8" --min-api 28 --lib "$platform" --output "$build" "$build/classes.jar"
  for variant in release dev; do
    # nsh 按 lib/ 下的目录认运行时有哪些 ABI；库本身由 nsh 用应用的 crate 编
    mkdir -p "$runtime/android/$variant/lib/arm64-v8a" "$runtime/android/$variant/lib/x86_64"
    cp "$build/classes.dex" "$runtime/android/$variant/classes.dex"
  done
  echo "编好了 Java 运行时：$dex" >&2
fi
[ "$1" = "--runtime-only" ] && exit 0
NSH_RUNTIME="$runtime"
export NSH_RUNTIME
exec nsh "$@"
