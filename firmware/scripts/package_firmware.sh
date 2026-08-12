#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "$0")/.." && pwd)"
platformio_core_dir="${PLATFORMIO_CORE_DIR:-${HOME}/.platformio}"
build_dir="$project_dir/.pio/build/codex-beacon"
dist_dir="$project_dir/dist"
esptool="$platformio_core_dir/packages/tool-esptoolpy/esptool.py"
boot_app0="$platformio_core_dir/packages/framework-arduinoespressif32/tools/partitions/boot_app0.bin"
output="$dist_dir/Codex-Beacon-StickS3.factory.bin"

cd "$project_dir"
pio run -e codex-beacon

if [[ ! -f "$esptool" || ! -f "$boot_app0" ]]; then
  echo "PlatformIO's esptool or boot_app0.bin is missing" >&2
  exit 1
fi

mkdir -p "$dist_dir"
python3 "$esptool" --chip esp32s3 merge_bin \
  --flash_mode dio \
  --flash_freq 80m \
  --flash_size 8MB \
  -o "$output" \
  0x0000 "$build_dir/bootloader.bin" \
  0x8000 "$build_dir/partitions.bin" \
  0xe000 "$boot_app0" \
  0x10000 "$build_dir/firmware.bin"

echo "$output"
