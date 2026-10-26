#!/usr/bin/env bash
# Build a Linux release binary that runs on glibc 2.17 (CentOS/RHEL 7) and newer.
#
# Uses cargo-zigbuild, which links against glibc 2.17 symbol stubs from zig
# instead of the host's glibc. Needs on the build machine:
#   - zig 0.13      (pip install ziglang==0.13.0, or a zig binary on PATH)
#   - cargo-zigbuild (cargo install cargo-zigbuild --locked)
#   - libxkbcommon-dev, libxkbcommon-x11-dev, libxcb1-dev, libfontconfig-dev,
#     libfreetype-dev (headers/.so to link against)
#
# Output: target/x86_64-unknown-linux-gnu/release/rocketman
set -euo pipefail

cd "$(dirname "$0")/.."

GLIBC=2.17
TRIPLE=x86_64-unknown-linux-gnu

rustup target add "$TRIPLE" >/dev/null
cargo zigbuild --release --locked --target "$TRIPLE.$GLIBC" "$@"

BIN="target/$TRIPLE/release/rocketman"

# Fail if anything pulled in a symbol newer than the target glibc.
max=$(objdump -T "$BIN" | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/GLIBC_//' | sort -uV | tail -1)
echo "$BIN: highest required GLIBC symbol version is $max"
if [ "$(printf '%s\n%s\n' "$GLIBC" "$max" | sort -V | tail -1)" != "$GLIBC" ]; then
  echo "error: binary requires GLIBC_$max, newer than $GLIBC" >&2
  exit 1
fi
