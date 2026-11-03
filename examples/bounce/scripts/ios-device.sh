#!/usr/bin/env bash
# 构建 iOS 版本，打包成 .app，签名后装到真机并启动。
# 用法: scripts/ios-device.sh [设备 UDID]
# 环境变量 NGE_DEBUG_STALL_MS=200：启动时开启主线程卡顿压测，并在终端输出日志。
set -euo pipefail

# 切到 workspace 根目录（编译产物在根目录 target/ 下）
cd "$(dirname "$0")/../../.."

BUNDLE_ID="com.lewu.nshgame"
APP_NAME="NshGame"
DEVICE="${1:-00008140-0016683422EB001C}"
PROFILE_NAME="iOS Team Provisioning Profile: ${BUNDLE_ID}"
OUT="target/ios-device"
APP="${OUT}/${APP_NAME}.app"

# 找到对应 Bundle ID 的 provisioning profile（由 tools/NshGame 项目自动生成）
PROFILE=""
for p in "$HOME/Library/Developer/Xcode/UserData/Provisioning Profiles"/*.mobileprovision; do
  if security cms -D -i "$p" 2>/dev/null | grep -q "<string>${PROFILE_NAME}</string>"; then
    PROFILE="$p"
  fi
done
[ -n "$PROFILE" ] || { echo "找不到 ${PROFILE_NAME}，先构建一次 tools/NshGame"; exit 1; }

mkdir -p "$OUT"
security cms -D -i "$PROFILE" > "${OUT}.profile.plist"
/usr/libexec/PlistBuddy -x -c "Print :Entitlements" "${OUT}.profile.plist" > "${OUT}.entitlements.plist"
CERT_SHA=$(/usr/libexec/PlistBuddy -c "Print :DeveloperCertificates:0" "${OUT}.profile.plist" | openssl x509 -inform DER -noout -fingerprint -sha1 | cut -d= -f2 | tr -d :)

cargo build -p bounce --release --target aarch64-apple-ios

rm -rf "$APP"
mkdir -p "$APP"
cp target/aarch64-apple-ios/release/bounce "$APP/${APP_NAME}"
cp "$PROFILE" "$APP/embedded.mobileprovision"
cat > "$APP/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>${BUNDLE_ID}</string>
  <key>CFBundleExecutable</key><string>${APP_NAME}</string>
  <key>CFBundleName</key><string>${APP_NAME}</string>
  <key>CFBundleDisplayName</key><string>NSH 弹球</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>CFBundleSupportedPlatforms</key><array><string>iPhoneOS</string></array>
  <key>DTPlatformName</key><string>iphoneos</string>
  <key>MinimumOSVersion</key><string>17.0</string>
  <key>LSRequiresIPhoneOS</key><true/>
  <key>UIDeviceFamily</key><array><integer>1</integer></array>
  <key>UIRequiredDeviceCapabilities</key><array><string>arm64</string></array>
  <key>UILaunchScreen</key><dict/>
  <key>UIApplicationSceneManifest</key>
  <dict><key>UIApplicationSupportsMultipleScenes</key><false/></dict>
  <key>CADisableMinimumFrameDurationOnPhone</key><true/>
</dict>
</plist>
EOF

codesign --force --sign "$CERT_SHA" --entitlements "${OUT}.entitlements.plist" --timestamp=none "$APP"
codesign --verify --verbose=1 "$APP"

xcrun devicectl device install app --device "$DEVICE" "$APP"
if [ -n "${NGE_DEBUG_STALL_MS:-}" ]; then
  xcrun devicectl device process launch --device "$DEVICE" --terminate-existing --console \
    --environment-variables "{\"NGE_DEBUG_STALL_MS\": \"${NGE_DEBUG_STALL_MS}\"}" "$BUNDLE_ID"
else
  xcrun devicectl device process launch --device "$DEVICE" --terminate-existing "$BUNDLE_ID"
fi
