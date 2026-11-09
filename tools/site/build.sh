#!/usr/bin/env bash
# Assemble the GitHub Pages site into one directory (default `target/site`).
#
#   tools/site/build.sh [OUT]
#
# The page itself lives in `site/`. The screenshots and the application icon are taken from the
# places the README and the application already read them, so the site cannot drift from them,
# and the Atelier web kit is fetched at a pinned revision of uthree/design-language rather than
# copied into this repository. Serve the result with any static server:
#
#   python3 -m http.server --directory target/site
set -euo pipefail

# The revision of uthree/design-language whose `web/atelier` kit the page is drawn with. The
# application pins an older revision in crates/auris-gpui/Cargo.toml; the web kit is newer than
# that pin, and the two are moved independently.
ATELIER_REV="71ca5b3917995dad5a091fa091bc7c78ac133e4f"
ATELIER_FILES=(
  atelier.css
  atelier.js
  ambient.js
  generated/tokens.css
  generated/icons.js
  generated/ambient.js
)

root="$(cd "$(dirname "$0")/../.." && pwd)"
out="${1:-$root/target/site}"

rm -rf "$out"
mkdir -p "$out/screenshots" "$out/atelier/generated"

cp -R "$root/site/." "$out/"
rm -f "$out/README.md"
cp "$root"/docs/screenshots/readme/*.png "$out/screenshots/"
cp "$root/crates/auris-gpui/assets/auris-studio.svg" "$out/icon.svg"
cp "$root/crates/auris-gpui/assets/auris-studio.png" "$out/icon.png"

base="https://raw.githubusercontent.com/uthree/design-language/$ATELIER_REV"
for file in "${ATELIER_FILES[@]}"; do
  curl --fail --silent --show-error --location --retry 3 \
    "$base/web/atelier/$file" -o "$out/atelier/$file"
done
curl --fail --silent --show-error --location --retry 3 "$base/LICENSE" -o "$out/atelier/LICENSE"

# GitHub Pages runs Jekyll over the upload unless told not to.
touch "$out/.nojekyll"

echo "site written to $out"
