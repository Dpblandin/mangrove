#!/bin/sh
# Stage the native application in a relocatable prefix (bin/ and share/).
set -eu

case "${1-}" in
  '') ;;
  -h|--help)
    printf 'Usage: %s\n' "$0"
    exit 0
    ;;
  *)
    printf 'Usage: %s\n' "$0" >&2
    exit 2
    ;;
esac
if [ "$(uname -s)" != Linux ]; then
  echo 'Le paquet Linux se construit sous Linux.' >&2
  exit 1
fi
gpui_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
gpui_target="$gpui_root/target"
gpui_identifier=dev.mangrove.gpui
gpui_icons="$gpui_root/assets/icons"
# Each entry maps a hicolor size to its source icon.
gpui_icon_sizes='32x32:32x32.png 128x128:128x128.png 256x256:128x128@2x.png'
for gpui_icon in $gpui_icon_sizes; do
  if [ ! -s "$gpui_icons/${gpui_icon#*:}" ]; then
    printf 'Icône Mangrove introuvable : %s\n' "$gpui_icons/${gpui_icon#*:}" >&2
    exit 1
  fi
done
cargo build --release --locked --manifest-path "$gpui_root/Cargo.toml" \
  --target-dir "$gpui_target"
gpui_executable="$gpui_target/release/mangrove-gpui"
if [ ! -x "$gpui_executable" ]; then
  printf 'Exécutable Mangrove introuvable : %s\n' "$gpui_executable" >&2
  exit 1
fi
# Extracting the prefix into ~/.local or /usr/local installs the launcher and icons.
gpui_prefix="$gpui_target/release/bundle/mangrove"
rm -rf "$gpui_prefix"
mkdir -p "$gpui_prefix/bin" "$gpui_prefix/share/applications"
cp "$gpui_executable" "$gpui_prefix/bin/mangrove-gpui"
cp "$gpui_root/packaging/linux/$gpui_identifier.desktop" "$gpui_prefix/share/applications/"
for gpui_icon in $gpui_icon_sizes; do
  gpui_icon_dir="$gpui_prefix/share/icons/hicolor/${gpui_icon%%:*}/apps"
  mkdir -p "$gpui_icon_dir"
  cp "$gpui_icons/${gpui_icon#*:}" "$gpui_icon_dir/$gpui_identifier.png"
done
if command -v desktop-file-validate >/dev/null 2>&1; then
  desktop-file-validate "$gpui_prefix/share/applications/$gpui_identifier.desktop" >&2
fi
printf '%s\n' "$gpui_prefix"
