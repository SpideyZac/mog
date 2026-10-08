#!/usr/bin/env bash
# fills the package templates in for one release.
# usage: packaging/render.sh <version without v> <folder with the release archives> <out folder>
set -euo pipefail

version="$1"
dist="$2"
out="$3"
here="$(cd "$(dirname "$0")" && pwd)"

sha() {
  local file
  file=$(ls "$dist"/mog-v"$version"-"$1".* | grep -v -e '\.sha256$' -e '\.minisig$' -e '\.sig$' | head -n 1)
  if command -v sha256sum > /dev/null; then
    sha256sum "$file" | cut -d ' ' -f 1
  else
    shasum -a 256 "$file" | cut -d ' ' -f 1
  fi
}

targets=(
  x86_64-unknown-linux-gnu
  aarch64-unknown-linux-gnu
  x86_64-apple-darwin
  aarch64-apple-darwin
  x86_64-pc-windows-msvc
)

script="s/{{version}}/$version/g"
for target in "${targets[@]}"; do
  hash=$(sha "$target")
  upper=$(printf '%s' "$hash" | tr '[:lower:]' '[:upper:]')
  script="$script;s/{{$target-upper}}/$upper/g;s/{{$target}}/$hash/g"
done

mkdir -p "$out/homebrew" "$out/aur" "$out/winget"
sed "$script" "$here/homebrew/mog.rb" > "$out/homebrew/mog.rb"
sed "$script" "$here/aur/PKGBUILD" > "$out/aur/PKGBUILD"
for manifest in "$here"/winget/*.yaml; do
  sed "$script" "$manifest" > "$out/winget/$(basename "$manifest")"
done
echo "rendered packages for v$version into $out"
