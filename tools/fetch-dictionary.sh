#!/usr/bin/env bash
#
# Downloads the Japanese dictionary Auris Studio ships with.
#
# The dictionary is what reads kanji lyrics and — since the composer learned to write from
# lyrics — what gives a composed melody the words' pitch accent to follow. Like the SoundFonts
# it is fetched rather than committed: this script unpacks it into `Dictionary/` at the top of
# the checkout, where a `cargo run` build finds it, and the release workflow runs it before
# assembling each archive.
#
# The record is not written here. `auris dictionary --manifest` prints it, straight out of
# `auris_session::library::JAPANESE_DICTIONARY`, so there is exactly one place the URL and the
# digest are recorded and no way for the two to drift apart.
#
#   tools/fetch-dictionary.sh [directory]
#
# A dictionary already present is left alone — the folder has no single file to hash, so
# presence of the metadata jpreprocess writes is the test, the same one the application uses.

set -euo pipefail

destination="${1:-$(cd "$(dirname "$0")/.." && pwd)/Dictionary}"

if command -v sha256sum >/dev/null 2>&1; then
  digest() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
  digest() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
  echo "fetch-dictionary: neither sha256sum nor shasum is installed" >&2
  exit 1
fi

manifest="$(cargo run --quiet --locked -p auris-cli -- dictionary --manifest)"
if [ -z "$manifest" ]; then
  echo "fetch-dictionary: the manifest is empty" >&2
  exit 1
fi

mkdir -p "$destination"

while IFS=$'\t' read -r id folder bytes sha url license_url; do
  [ -n "${id:-}" ] || continue
  target="$destination/$folder"

  # The notice travels with the dictionary, always — including on the run that finds it
  # already there, because an archive assembled from a directory missing it would be the one
  # that ships.
  curl --fail --location --show-error --silent \
    --output "$destination/${folder}_License.md" "$license_url"

  if [ -f "$target/metadata.json" ]; then
    echo "$id: already installed at $target"
    continue
  fi

  echo "$id: fetching $(( bytes / 1048576 )) MB from $url"
  # Downloaded and verified before anything is extracted, so an interrupted download can
  # never leave a half-dictionary where the application looks for a whole one.
  archive="$destination/$folder.tar.gz.part"
  curl --fail --location --show-error --silent --output "$archive" "$url"

  actual="$(digest "$archive")"
  if [ "$actual" != "$sha" ]; then
    rm -f "$archive"
    echo "fetch-dictionary: the archive hashed to $actual, expected $sha" >&2
    exit 1
  fi

  # Extracted beside the destination first: the archive's own top directory is `$folder`,
  # and a rename is the atomic move the download had.
  staging="$destination/.unpack"
  rm -rf "$staging"
  mkdir -p "$staging"
  tar -xzf "$archive" -C "$staging"
  rm -f "$archive"
  if [ ! -f "$staging/$folder/metadata.json" ]; then
    echo "fetch-dictionary: the archive did not contain $folder/metadata.json" >&2
    exit 1
  fi
  rm -rf "$target"
  mv "$staging/$folder" "$target"
  rm -rf "$staging"
  echo "$id: installed at $target"
done <<< "$manifest"
