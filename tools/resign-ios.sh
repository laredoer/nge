#!/bin/sh
# 重签 nsh package --target ios 打出的 .ipa：nsh 0.4.0 把描述文件里的通配钥匙串组（TEAM.*）原样签进去，
# iOS 拒绝启动（见 docs/native-components.md）。这里用描述文件的权限，只把钥匙串组和应用 ID 换成本应用的。
set -eu

if [ $# -ne 3 ]; then
  echo "用法：$0 <ipa> <签名证书 SHA-1> <输出 ipa>" >&2
  exit 2
fi
ipa=$1
identity=$2
out=$(cd "$(dirname "$3")" && pwd)/$(basename "$3")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

unzip -q "$ipa" -d "$work"
set -- "$work"/Payload/*.app
if [ $# -ne 1 ] || [ ! -d "$1" ]; then
  echo "$ipa 的 Payload 里要正好有一个 .app" >&2
  exit 1
fi
app=$1
plist=/usr/libexec/PlistBuddy
security cms -D -i "$app/embedded.mobileprovision" > "$work/profile.plist"
bundle=$($plist -c 'Print :CFBundleIdentifier' "$app/Info.plist")
# 应用 ID 前缀取描述文件里的，老账号的前缀可能和团队 ID 不一样
prefix=$($plist -c 'Print :Entitlements:application-identifier' "$work/profile.plist" | cut -d. -f1)

ent=$work/ent.plist
$plist -x -c 'Print :Entitlements' "$work/profile.plist" > "$ent"
$plist -c "Set :application-identifier $prefix.$bundle" "$ent"
$plist -c 'Delete :keychain-access-groups' "$ent" 2>/dev/null || true
$plist -c 'Add :keychain-access-groups array' -c "Add :keychain-access-groups:0 string $prefix.$bundle" "$ent"

codesign -f -s "$identity" --entitlements "$ent" --generate-entitlement-der "$app"
codesign --verify --deep --strict "$app"
rm -f "$out"
(cd "$work" && zip -qr "$out" .)
echo "$out"
