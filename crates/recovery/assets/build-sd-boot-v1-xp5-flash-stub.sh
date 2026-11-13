#!/usr/bin/env bash
# Assemble sd-boot-v1-xp5-flash-stub.S into sd-boot-v1-xp5-flash-stub.generated.bin and write the
# symbol offsets next to it. Needs clang with the AArch64 target and llvm-objcopy/llvm-nm
# (any version; the file is hand-written assembly, so the result does not depend on a compiler).
#   build-sd-boot-v1-xp5-flash-stub.sh          write
#   build-sd-boot-v1-xp5-flash-stub.sh --check  fail if the checked-in files differ
set -euo pipefail
cd "$(dirname "$0")"
CLANG=${CLANG:-clang}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
"$CLANG" --target=aarch64-none-elf -c -x assembler sd-boot-v1-xp5-flash-stub.S -o "$work/f.o"
objcopy=$(command -v llvm-objcopy || command -v llvm-objcopy-18 || command -v objcopy)
nm=$(command -v llvm-nm || command -v llvm-nm-18 || command -v nm)
"$objcopy" -O binary -j .text "$work/f.o" "$work/f.bin"
offsets() {
  "$nm" "$work/f.o" | while read -r address _ name; do
    case "$name" in fail_entry|gate_entry|release_entry|stub_params) echo "$name $((16#$address))" ;; esac
  done | sort
}
{
  echo "{"
  echo "  \"schema\": 1,"
  echo "  \"image_bytes\": $(stat -c %s "$work/f.bin"),"
  echo "  \"symbols\": {"
  offsets | awk '{printf "%s    \"%s\": %s", (NR>1?",\n":""), $1, $2} END {print ""}'
  echo "  }"
  echo "}"
} > "$work/f.json"
if [ "${1:-}" = "--check" ]; then
  cmp "$work/f.bin" sd-boot-v1-xp5-flash-stub.generated.bin
  cmp "$work/f.json" sd-boot-v1-xp5-flash-stub.generated.json
else
  cp "$work/f.bin" sd-boot-v1-xp5-flash-stub.generated.bin
  cp "$work/f.json" sd-boot-v1-xp5-flash-stub.generated.json
fi
