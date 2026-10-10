#!/bin/sh
# Print the CHANGELOG.md section of a version, without its heading, as release notes.
set -eu

if [ "$#" -ne 1 ]; then
  printf 'Usage: %s <version>\n' "$0" >&2
  exit 2
fi
changelog="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)/CHANGELOG.md"
if [ ! -f "$changelog" ]; then
  printf 'Changelog introuvable : %s\n' "$changelog" >&2
  exit 1
fi
awk -v heading="## [$1]" '
  index($0, heading) == 1 { found = 1; next }
  found && /^## \[/ { exit }
  found {
    lines[++count] = $0
    if ($0 ~ /[^[:space:]]/) {
      if (!first) first = count
      last = count
    }
  }
  END {
    if (!first) exit 1
    for (line = first; line <= last; line++) print lines[line]
  }
' "$changelog" || {
  printf 'Aucune note pour la version %s dans CHANGELOG.md.\n' "$1" >&2
  exit 1
}
