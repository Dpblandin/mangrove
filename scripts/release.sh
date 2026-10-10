#!/bin/sh
# Prepare a release on main: bump the version, write its CHANGELOG.md section,
# then commit and tag locally. Pushing the tag starts the Release workflow.
set -eu

usage() {
  printf 'Usage: %s <version>   (exemple : %s 0.2.0)\n' "$0" "$0"
}
case "${1-}" in
  -h|--help)
    usage
    exit 0
    ;;
esac
if [ "$#" -ne 1 ]; then
  usage >&2
  exit 2
fi
version=$1
if ! printf '%s\n' "$version" | grep -Eq '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'; then
  printf 'Version invalide : %s (attendu : X.Y.Z)\n' "$version" >&2
  exit 2
fi
tag="v$version"
cd -- "$(dirname -- "$0")/.."
# An installed git-cliff wins; otherwise npx runs a pinned release without installing it.
if command -v git-cliff >/dev/null 2>&1; then
  cliff() { git-cliff "$@"; }
elif command -v npx >/dev/null 2>&1; then
  cliff() { npx --yes git-cliff@2.14.2 "$@"; }
else
  echo 'git-cliff ou Node.js est requis : brew install git-cliff (ou brew install node).' >&2
  exit 1
fi
if [ "$(git branch --show-current)" != main ]; then
  echo 'Une release se prépare depuis la branche main.' >&2
  exit 1
fi
if [ -n "$(git status --porcelain)" ]; then
  echo 'L’arbre de travail doit être propre avant une release.' >&2
  exit 1
fi
if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  printf 'Le tag %s existe déjà.\n' "$tag" >&2
  exit 1
fi
previous=$(git describe --tags --abbrev=0 --match 'v[0-9]*.[0-9]*.[0-9]*' 2>/dev/null || true)
if [ -n "$previous" ] && ! printf '%s\n' "${previous#v}" "$version" | awk -F. '
  NR == 1 { split($0, old, ".") }
  NR == 2 {
    for (part = 1; part <= 3; part++) {
      if ($part + 0 != old[part] + 0) exit !($part + 0 > old[part] + 0)
    }
    exit 1
  }'; then
  printf 'La version %s doit suivre la dernière release %s.\n' "$version" "$previous" >&2
  exit 1
fi
if ! cliff --unreleased --tag "$tag" --strip all 2>/dev/null | grep -q '^### '; then
  echo 'Aucun commit feat, fix ou perf depuis la dernière release : rien à publier.' >&2
  exit 1
fi

# The application and the engine crates share one product version.
for manifest in Cargo.toml src-gpui/Cargo.toml; do
  awk -v version="$version" '
    !done && /^version = "/ { print "version = \"" version "\""; done = 1; next }
    { print }
  ' "$manifest" > "$manifest.release"
  mv "$manifest.release" "$manifest"
done
cargo metadata --format-version 1 >/dev/null
cargo metadata --format-version 1 --manifest-path src-gpui/Cargo.toml >/dev/null

if [ -f CHANGELOG.md ]; then
  cliff --unreleased --tag "$tag" --prepend CHANGELOG.md
else
  cliff --unreleased --tag "$tag" --output CHANGELOG.md
fi
sh scripts/release-notes.sh "$version" >/dev/null

git add Cargo.toml Cargo.lock src-gpui/Cargo.toml src-gpui/Cargo.lock CHANGELOG.md
git commit --quiet -m "chore(release): $tag"
git tag -a "$tag" -m "Mangrove $version"
printf 'Release %s prête localement (commit et tag).\n' "$tag"
printf 'Relire : git show HEAD -- CHANGELOG.md\n'
printf 'Publier : git push --atomic origin main %s\n' "$tag"
