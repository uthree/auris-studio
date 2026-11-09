#!/usr/bin/env bash
# Verify the actual release archives before publishing, including linked documentation.
set -euo pipefail

release="${1:?release tag required}"
artifacts="$(cd "${2:?artifact directory required}" && pwd)"
version="${release#v}"
[[ "$release" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]

expected=(
  "auris-${release}-linux-x86_64.tar.gz"
  "auris-studio-${release}-macos-aarch64.dmg"
  "auris-studio-${release}-macos-aarch64.tar.gz"
  "auris-studio-${release}-windows-x86_64-setup.exe"
  "auris-studio-${release}-windows-x86_64.zip"
)
shopt -s nullglob dotglob
actual=("$artifacts"/*)
[[ ${#actual[@]} -eq ${#expected[@]} ]]
for name in "${expected[@]}"; do
  test -s "$artifacts/$name"
done

scratch="$(mktemp -d "${TMPDIR:-/tmp}/auris-release-verify.XXXXXXXX")"
test "$(dirname "$(cd "$scratch" && pwd -P)")" = "$(cd "${TMPDIR:-/tmp}" && pwd -P)"
trap 'rm -rf -- "$scratch"' EXIT
for platform in linux macos windows; do
  destination="$scratch/$platform"
  mkdir -p "$destination"
  case "$platform" in
    linux)
      stage="auris-${release}-linux-x86_64"
      tar -xzf "$artifacts/$stage.tar.gz" -C "$destination"
      binaries=(auris)
      ;;
    macos)
      stage="auris-studio-${release}-macos-aarch64"
      tar -xzf "$artifacts/$stage.tar.gz" -C "$destination"
      binaries=(auris-studio auris auris-mcp auris-update-helper)
      ;;
    windows)
      stage="auris-studio-${release}-windows-x86_64"
      unzip -q "$artifacts/$stage.zip" -d "$destination"
      binaries=(auris-studio.exe auris.exe auris-mcp.exe auris-update-helper.exe)
      ;;
  esac
  root="$destination/$stage"
  for document in README.md README.ja.md CHANGELOG.md LICENSE docs/development.md docs/agent-workflows.ja.md; do
    test -s "$root/$document"
  done
  grep -Fq "## $version " "$root/CHANGELOG.md"
  binary_root="$root"
  dictionary="$root/Dictionary"
  if [ "$platform" = macos ]; then
    app="$root/Auris Studio.app/Contents"
    binary_root="$app/MacOS"
    dictionary="$app/Resources/Dictionary"
    for key in CFBundleShortVersionString CFBundleVersion; do
      actual_version="$(awk -v key="$key" '
        index($0, "<key>" key "</key>") { getline; gsub(/<[^>]*>|[[:space:]]/, ""); print; exit }
      ' "$app/Info.plist")"
      test "$actual_version" = "$version"
    done
  fi
  for binary in "${binaries[@]}"; do
    test -s "$binary_root/$binary"
  done
  metadata=("$dictionary"/*/metadata.json)
  notices=("$dictionary"/*_License.md)
  [[ ${#metadata[@]} -gt 0 && ${#notices[@]} -gt 0 ]]
  for file in "${metadata[@]}" "${notices[@]}"; do
    test -s "$file"
  done
  echo "Verified $platform archive: binaries, documentation, dictionary and version"
done

(
  cd "$artifacts"
  sha256sum "${expected[@]}" > SHA256SUMS
  sha256sum --check SHA256SUMS
)
