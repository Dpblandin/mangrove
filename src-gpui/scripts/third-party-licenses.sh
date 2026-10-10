#!/bin/sh
# Write the licenses of the third-party components shipped in Mangrove builds.
set -eu

if [ "$#" -ne 1 ]; then
  printf 'Usage: %s <fichier>\n' "$0" >&2
  exit 2
fi
if ! command -v cargo-about >/dev/null 2>&1; then
  echo 'cargo-about est requis : cargo install --locked cargo-about --features cli' >&2
  exit 1
fi
gpui_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cargo about generate --locked --fail --manifest-path "$gpui_root/Cargo.toml" \
  -c "$gpui_root/packaging/about.toml" -o "$1" "$gpui_root/packaging/about.hbs"
# The brand logos are assets rather than crates.
for gpui_brand in GitLab:gitlab Bitbucket:bitbucket; do
  printf '\n## Logo %s\n\n```text\n' "${gpui_brand%%:*}"
  cat "$gpui_root/assets/brands/LICENSE.${gpui_brand#*:}"
  printf '```\n'
done >> "$1"
