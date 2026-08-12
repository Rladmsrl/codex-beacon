#!/bin/zsh
set -euo pipefail

project_dir="${0:A:h:h}"
app_dir="$project_dir/dist/Codex Beacon.app"
contents="$app_dir/Contents"
arm_binary="$project_dir/target/aarch64-apple-darwin/release/codex-ble-bridge"
intel_binary="$project_dir/target/x86_64-apple-darwin/release/codex-ble-bridge"
icon_master="$project_dir/assets/AppIcon-1024.png"

if [[ ! -f "$icon_master" ]]; then
  echo "Missing app icon master: $icon_master" >&2
  exit 1
fi

cd "$project_dir"
cargo build --release --target aarch64-apple-darwin
cargo build --release --target x86_64-apple-darwin
rm -rf "$app_dir"
mkdir -p "$contents/MacOS" "$contents/Resources"
lipo -create "$arm_binary" "$intel_binary" -output "$contents/MacOS/codex-ble-bridge"
cp "$project_dir/scripts/Info.plist" "$contents/Info.plist"

icon_work="$(mktemp -d "${TMPDIR:-/tmp}/codex-ble-icon.XXXXXX")"
trap 'rm -rf "$icon_work"' EXIT
iconset="$icon_work/AppIcon.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$icon_master" \
    --out "$iconset/icon_${size}x${size}.png" >/dev/null
  double_size=$((size * 2))
  sips -z "$double_size" "$double_size" "$icon_master" \
    --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$contents/Resources/AppIcon.icns"

codesign --force --deep --sign - "$app_dir"
mkdir -p "$project_dir/dist"
COPYFILE_DISABLE=1 ditto -c -k --keepParent "$app_dir" "$project_dir/dist/Codex-Beacon-macOS.zip"
echo "$app_dir"
