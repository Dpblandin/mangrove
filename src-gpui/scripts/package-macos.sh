#!/bin/sh
# Package the native application with the original Mangrove icon.
set -eu

usage() {
  printf 'Usage: %s [--benchmark] [--target <triple>]\n' "$0"
}
benchmark=false
target=
while [ "$#" -gt 0 ]; do
  case "$1" in
    --benchmark) benchmark=true ;;
    --target)
      if [ "$#" -lt 2 ] || [ -z "$2" ]; then
        usage >&2
        exit 2
      fi
      target=$2
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      usage >&2
      exit 2
      ;;
  esac
  shift
done
if [ "$(uname -s)" != Darwin ]; then
  echo 'Le bundle .app se construit sur macOS.' >&2
  exit 1
fi
gpui_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
gpui_target="$gpui_root/target"
gpui_app_name=Mangrove
gpui_identifier=dev.mangrove.gpui
if [ "$benchmark" = true ]; then
  gpui_target="$gpui_target/benchmark"
  gpui_app_name='Mangrove GPUI Benchmark'
  gpui_identifier=dev.mangrove.gpui.benchmark
fi
gpui_icon="$gpui_root/assets/icons/icon.icns"
if [ ! -s "$gpui_icon" ]; then
  printf 'Icône Mangrove introuvable : %s\n' "$gpui_icon" >&2
  exit 1
fi
gpui_version=$(awk '
  /^\[package\][[:space:]]*$/ { package_section = 1; next }
  package_section && /^\[/ { exit }
  package_section && /^version[[:space:]]*=/ {
    sub(/^[^"]*"/, ""); sub(/".*$/, ""); print; exit
  }
' "$gpui_root/Cargo.toml")
case "$gpui_version" in
  ''|*[!0-9.]*)
    echo 'La version du bundle doit être numérique dans Cargo.toml.' >&2
    exit 1
    ;;
esac
set -- --release --locked --manifest-path "$gpui_root/Cargo.toml" --target-dir "$gpui_target"
if [ "$benchmark" = true ]; then
  set -- "$@" --features app-benchmark
fi
# A cross-compiled build lands in target/<triple>/release, like Cargo's own output.
gpui_release="$gpui_target/release"
if [ -n "$target" ]; then
  set -- "$@" --target "$target"
  gpui_release="$gpui_target/$target/release"
fi
cargo build "$@"
gpui_executable="$gpui_release/mangrove-gpui"
if [ ! -x "$gpui_executable" ]; then
  printf 'Exécutable Mangrove introuvable : %s\n' "$gpui_executable" >&2
  exit 1
fi
gpui_bundle_parent="$gpui_release/bundle"
gpui_bundle="$gpui_bundle_parent/$gpui_app_name.app"
mkdir -p "$gpui_bundle_parent"
gpui_stage=$(mktemp -d "$gpui_bundle_parent/.mangrove-package.XXXXXX")
gpui_previous="$gpui_stage/previous.app"
cleanup() {
  if [ -e "$gpui_previous" ] && [ ! -e "$gpui_bundle" ]; then
    mv "$gpui_previous" "$gpui_bundle"
  fi
  rm -rf "$gpui_stage"
}
trap cleanup 0
trap 'exit 1' HUP INT TERM
gpui_staged_bundle="$gpui_stage/$gpui_app_name.app"
mkdir -p "$gpui_staged_bundle/Contents/MacOS" "$gpui_staged_bundle/Contents/Resources"
cp "$gpui_executable" "$gpui_staged_bundle/Contents/MacOS/mangrove-gpui"
cp "$gpui_icon" "$gpui_staged_bundle/Contents/Resources/icon.icns"
cat > "$gpui_staged_bundle/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleDisplayName</key><string>$gpui_app_name</string>
  <key>CFBundleName</key><string>$gpui_app_name</string>
  <key>CFBundleIdentifier</key><string>$gpui_identifier</string>
  <key>CFBundleExecutable</key><string>mangrove-gpui</string>
  <key>CFBundleIconFile</key><string>icon.icns</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$gpui_version</string>
  <key>CFBundleVersion</key><string>$gpui_version</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSPrincipalClass</key><string>NSApplication</string>
</dict></plist>
PLIST
plutil -lint "$gpui_staged_bundle/Contents/Info.plist" >&2
# Seal Info.plist and resources with an ad-hoc signature bound to the bundle identifier.
codesign --force --sign - "$gpui_staged_bundle" >&2
if [ -e "$gpui_bundle" ] || [ -L "$gpui_bundle" ]; then
  mv "$gpui_bundle" "$gpui_previous"
fi
mv "$gpui_staged_bundle" "$gpui_bundle"
printf '%s\n' "$gpui_bundle"
