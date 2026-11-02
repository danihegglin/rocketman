#!/usr/bin/env bash
# Turn the glibc 2.17 release tarball into distro packages: a .deb (Debian,
# Ubuntu), an .rpm (RHEL/CentOS 7+, Fedora, openSUSE) and, when
# APPIMAGETOOL points at appimagetool, an AppImage.
#
# All of them use the tarball's layout unchanged (binaries plus the bundled
# xkbcommon libraries in lib/, found through the RPATH), so they run on the
# same systems the tarball does. The deb and rpm install it to /opt/rocketman
# with `rocketman` and `rocketman-tui` symlinked into /usr/bin, plus a desktop
# entry and the app icon (PNGs in every hicolor size plus the SVG).
#
# Needs dpkg-deb and rpmbuild (apt-get install rpm), and desktop-file-validate
# (desktop-file-utils) for the AppImage. Run after
# scripts/package-linux-glibc217.sh.
# Usage: scripts/package-linux-distro.sh <tarball> <tag> <out-dir>
# Prints the paths of the packages it wrote.
set -euo pipefail

TARBALL=$(realpath "$1")
TAG=$2
OUT=$3

cd "$(dirname "$0")/.."
ROOT=$PWD
mkdir -p "$OUT"
OUT=$(realpath "$OUT")

VERSION=$(sed -n '/^\[package\]/,/^\[/{s/^version *= *"\(.*\)"/\1/p}' Cargo.toml | head -n1)
DESCRIPTION=$(sed -n '/^\[package\]/,/^\[/{s/^description *= *"\(.*\)"/\1/p}' Cargo.toml | head -n1)
HOMEPAGE=https://github.com/danihegglin/rocketman
# Both dpkg and rpm sort `~` before anything, so 0.2.0~beta.1 < 0.2.0.
PKG_VERSION=${VERSION/-/\~}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# --- Shared file tree -------------------------------------------------------
stage=$work/root
mkdir -p "$stage/opt/rocketman" "$stage/usr/bin" "$stage/usr/share/applications" \
  "$stage/usr/share/icons/hicolor/scalable/apps" "$stage/usr/share/doc/rocketman"
ICON_SIZES=(16 24 32 48 64 128 256 512)
tar -xzf "$TARBALL" -C "$stage/opt/rocketman"
ln -s ../../opt/rocketman/rocketman "$stage/usr/bin/rocketman"
ln -s ../../opt/rocketman/rocketman-tui "$stage/usr/bin/rocketman-tui"
install -m 644 packaging/linux/rocketman.desktop "$stage/usr/share/applications/"
install -m 644 assets/icon/rocketman.svg "$stage/usr/share/icons/hicolor/scalable/apps/"
for size in "${ICON_SIZES[@]}"; do
  install -D -m 644 "assets/icon/png/${size}x${size}.png" \
    "$stage/usr/share/icons/hicolor/${size}x${size}/apps/rocketman.png"
done
install -m 644 README.md LICENSE "$stage/usr/share/doc/rocketman/"
chmod -R u=rwX,go=rX "$stage"

# --- .deb -------------------------------------------------------------------
deb=$work/deb
cp -a "$stage" "$deb"
mkdir -p "$deb/DEBIAN"
cat > "$deb/DEBIAN/control" <<CONTROL
Package: rocketman
Version: $PKG_VERSION
Architecture: amd64
Maintainer: danihegglin <2772547+danihegglin@users.noreply.github.com>
Installed-Size: $(du -sk --exclude=DEBIAN "$deb" | cut -f1)
Depends: libc6 (>= 2.17), libxcb1, libfontconfig1, libvulkan1
Recommends: mesa-vulkan-drivers | vulkan-icd
Section: net
Priority: optional
Homepage: $HOMEPAGE
Description: $DESCRIPTION
 rocketman is a desktop HTTP client (rocketman) and terminal UI
 (rocketman-tui) whose collections are plain OpenAPI 3.x files.
CONTROL
deb_out=$OUT/rocketman_${PKG_VERSION}_amd64.deb
# xz rather than dpkg's zstd default, which dpkg before 1.21.18 can't unpack.
dpkg-deb -Zxz --root-owner-group --build "$deb" "$deb_out" >&2
echo "$deb_out"

# --- .rpm -------------------------------------------------------------------
# Dependencies are listed by soname so they resolve on every rpm distro whatever
# the package is called there. Automatic dependencies are off so the bundled
# libraries don't show up as provided by this package.
cat > "$work/rocketman.spec" <<SPEC
Name:           rocketman
Version:        $PKG_VERSION
Release:        1
Summary:        $DESCRIPTION
License:        MIT
URL:            $HOMEPAGE
BuildArch:      x86_64
AutoReqProv:    no
Requires:       libc.so.6(GLIBC_2.17)(64bit)
Requires:       libxcb.so.1()(64bit)
Requires:       libfontconfig.so.1()(64bit)
Requires:       libvulkan.so.1()(64bit)

%description
rocketman is a desktop HTTP client (rocketman) and terminal UI
(rocketman-tui) whose collections are plain OpenAPI 3.x files.

%install
cp -a $stage/. %{buildroot}/

%files
/opt/rocketman
/usr/bin/rocketman
/usr/bin/rocketman-tui
/usr/share/applications/rocketman.desktop
/usr/share/icons/hicolor/*/apps/rocketman.*
%doc /usr/share/doc/rocketman
SPEC
# The binaries are already stripped, and the bundled libraries must stay
# byte-for-byte (see package-linux-glibc217.sh), so rpmbuild leaves files alone.
# An xz payload (not zstd) keeps the rpm installable with CentOS 7's rpm 4.11.
rpmbuild -bb "$work/rocketman.spec" \
  --define "_topdir $work/rpmbuild" \
  --define "_rpmdir $work/rpms" \
  --define "_build_id_links none" \
  --define "debug_package %{nil}" \
  --define "__os_install_post %{nil}" \
  --define "_binary_payload w9.xzdio" \
  --define "_binary_filedigest_algorithm 8" >&2
rpm_out=$OUT/rocketman-${PKG_VERSION}-1.x86_64.rpm
mv "$work"/rpms/x86_64/rocketman-*.rpm "$rpm_out"
echo "$rpm_out"

# --- AppImage ---------------------------------------------------------------
if [ -n "${APPIMAGETOOL:-}" ]; then
  app=$work/AppDir
  mkdir -p "$app/usr/bin"
  cp -a "$stage/opt/rocketman/." "$app/usr/bin/"
  cp -a "$stage/usr/share" "$app/usr/"
  cp packaging/linux/rocketman.desktop assets/icon/rocketman.png "$app/"
  ln -s rocketman.png "$app/.DirIcon"
  # Runs the desktop app, or the terminal app when started through a link
  # named rocketman-tui (ln -s rocketman-*.AppImage rocketman-tui).
  cat > "$app/AppRun" <<'APPRUN'
#!/bin/sh
HERE=$(dirname "$(readlink -f "$0")")
case "$(basename "${ARGV0:-$0}")" in
  rocketman-tui*) exec "$HERE/usr/bin/rocketman-tui" "$@" ;;
  *) exec "$HERE/usr/bin/rocketman" "$@" ;;
esac
APPRUN
  chmod 755 "$app/AppRun"
  appimage_out=$OUT/rocketman-$TAG-x86_64.AppImage
  ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$APPIMAGETOOL" --no-appstream "$app" "$appimage_out" >&2
  echo "$appimage_out"
fi
