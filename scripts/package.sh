#!/bin/sh
set -eu

version=${1:?usage: scripts/package.sh VERSION}
binary=target/release/radeon-smi
test -x "$binary"
command -v dpkg-deb >/dev/null 2>&1 || { echo 'dpkg-deb is required' >&2; exit 1; }
command -v objdump >/dev/null 2>&1 || { echo 'objdump is required' >&2; exit 1; }
manifest_version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)
test "$version" = "$manifest_version" || { echo 'version does not match Cargo.toml' >&2; exit 1; }
glibc=$(objdump -T "$binary" | sed -n 's/.*GLIBC_\([0-9][0-9.]*\).*/\1/p' | sort -V | tail -n 1)
test -n "$glibc" || { echo 'cannot determine minimum glibc version' >&2; exit 1; }
mkdir -p dist
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT HUP INT TERM

archive="radeon-smi-${version}-linux-x86_64"
mkdir -p "$stage/$archive"
cp "$binary" README.md LICENSE "$stage/$archive/"
tar -czf "dist/$archive.tar.gz" -C "$stage" "$archive"

package="$stage/package"
mkdir -p "$package/DEBIAN" "$package/usr/bin" "$package/usr/share/doc/radeon-smi"
cp "$binary" "$package/usr/bin/radeon-smi"
cp README.md LICENSE "$package/usr/share/doc/radeon-smi/"
size=$(du -sk "$package/usr" | cut -f1)
cat > "$package/DEBIAN/control" <<EOF
Package: radeon-smi
Version: $version
Section: utils
Priority: optional
Architecture: amd64
Maintainer: kuma-loong <https://github.com/kuma-loong>
Depends: libc6 (>= $glibc)
Installed-Size: $size
Homepage: https://github.com/kuma-loong/radeon-smi
Description: SMI-style monitoring for legacy AMD Radeon GPUs
 Read-only GPU monitoring through Linux DRM and sysfs.
EOF
dpkg-deb --build --root-owner-group "$package" "dist/radeon-smi_${version}_amd64.deb"

(cd dist && sha256sum "$archive.tar.gz" "radeon-smi_${version}_amd64.deb" > SHA256SUMS)
