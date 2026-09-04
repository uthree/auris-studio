#!/usr/bin/env bash
#
# Downloads the SoundFonts Auris Studio ships with.
#
# The fonts are hundreds of megabytes, which is more than GitHub accepts in a single file and far
# more than every clone of a source repository should have to carry. So they are fetched: this
# script puts them in `SoundFonts/` at the top of the checkout, where a `cargo run` build finds
# them, and the release workflow runs it before assembling each archive.
#
# The list is not written here. `auris soundfonts --manifest` prints it, straight out of
# `auris_session::library::SHIPPED`, so there is exactly one place a URL or a digest is recorded
# and no way for the two to drift apart.
#
#   tools/fetch-soundfonts.sh [directory]
#
# A font already present with the right digest is left alone, so running this twice costs one
# hash rather than one download.

set -euo pipefail

destination="${1:-$(cd "$(dirname "$0")/.." && pwd)/SoundFonts}"

# `sha256sum` on Linux and Git Bash, `shasum -a 256` on macOS. Neither is everywhere.
if command -v sha256sum >/dev/null 2>&1; then
  digest() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
  digest() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
  echo "fetch-soundfonts: neither sha256sum nor shasum is installed" >&2
  exit 1
fi

manifest="$(cargo run --quiet --locked -p auris-cli -- soundfonts --manifest)"
if [ -z "$manifest" ]; then
  echo "fetch-soundfonts: the manifest is empty" >&2
  exit 1
fi

mkdir -p "$destination"

while IFS=$'\t' read -r id file bytes sha url license_url; do
  [ -n "${id:-}" ] || continue
  target="$destination/$file"

  # The notice travels with the font, always — including on the run that finds the font already
  # there, because an archive assembled from a directory missing it would be the one that ships.
  notice="$destination/${file%.*}_License.md"
  curl --fail --location --show-error --silent --connect-timeout 15 --max-time 600 \
    --retry 2 --output "$notice.part" "$license_url"
  mv -f "$notice.part" "$notice"

  if [ -f "$target" ] && [ "$(digest "$target")" = "$sha" ]; then
    echo "$id: already installed at $target"
    continue
  fi

  echo "$id: fetching $(( bytes / 1048576 )) MB from $url"
  # Downloaded under a temporary name and renamed only once the digest agrees. A half-finished
  # download left where the application looks would be found, loaded and refused by the parser,
  # and the error would name a corrupt file rather than an interrupted one.
  partial="$target.part"
  curl --fail --location --show-error --silent --connect-timeout 15 --max-time 600 \
    --retry 2 --output "$partial" "$url"

  actual="$(digest "$partial")"
  if [ "$actual" != "$sha" ]; then
    rm -f "$partial"
    echo "fetch-soundfonts: $file hashed to $actual, expected $sha" >&2
    exit 1
  fi

  mv -f "$partial" "$target"
  echo "$id: installed at $target"
done <<< "$manifest"
