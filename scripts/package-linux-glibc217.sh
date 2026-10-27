#!/usr/bin/env bash
# Package the glibc 2.17 build into a release tarball that also runs on systems
# without libxkbcommon-x11 (e.g. RHEL 9 on Wayland, where it isn't installed).
#
# The xkbcommon libraries (libxkbcommon, libxkbcommon-x11, libxcb-xkb) are
# bundled in lib/ next to the binaries and found through the desktop binary's
# RPATH (not RUNPATH, which wouldn't apply to libxcb-xkb, loaded only by
# libxkbcommon-x11). The libraries themselves are left unpatched: patchelf
# breaks their ELF alignment. They are taken from CentOS 7 so that they need nothing newer than
# glibc 2.17 themselves. libxcb, Wayland, Vulkan and fontconfig still come from
# the host system.
#
# Needs docker and patchelf. Run after scripts/build-linux-glibc217.sh.
# Usage: scripts/package-linux-glibc217.sh <tag> <out-dir>
# Extra `docker run` arguments can be passed in DOCKER_RUN_ARGS, and another
# CentOS 7 image in CENTOS7_IMAGE.
set -euo pipefail

TAG=$1
OUT=$2

cd "$(dirname "$0")/.."

REL=target/x86_64-unknown-linux-gnu/release
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/lib"

# shellcheck disable=SC2086
docker run --rm ${DOCKER_RUN_ARGS:-} -v "$work/lib:/out" "${CENTOS7_IMAGE:-centos:7}" bash -ec '
  sed -i -e "s/^mirrorlist/#mirrorlist/" -e "s|^#baseurl=http://mirror.centos.org|baseurl=https://vault.centos.org|" /etc/yum.repos.d/*.repo
  yum install -y -q libxkbcommon-x11
  cp -L /usr/lib64/libxkbcommon.so.0 /usr/lib64/libxkbcommon-x11.so.0 /usr/lib64/libxcb-xkb.so.1 /out/
  chown '"$(id -u):$(id -g)"' /out/*' >&2

cp "$REL/rocketman" "$REL/rocketman-tui" "$work/"
patchelf --force-rpath --set-rpath '$ORIGIN/lib' "$work/rocketman"

mkdir -p "$OUT"
asset="$OUT/rocketman-$TAG-x86_64-linux-glibc2.17.tar.gz"
tar -czf "$asset" --owner=0 --group=0 -C "$work" rocketman rocketman-tui lib
echo "$asset"
