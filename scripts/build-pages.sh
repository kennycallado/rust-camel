#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
output_dir="${1:-$repo_root/pages-artifact}"
book_dir="$repo_root/docs/book"

if [[ ! -f "$book_dir/index.html" ]]; then
  echo "docs/book is missing; run mdbook build docs first" >&2
  exit 1
fi

rm -rf "$output_dir"
mkdir -p "$output_dir/docs"
cp -R "$repo_root/site/." "$output_dir/"
cp -R "$book_dir/." "$output_dir/docs/"

# mdBook's 404 page links to site-url with a root-relative URL. Keep the
# deployed behavior while making the composed artifact verifiable offline.
sed -i 's|href="/rust-camel/docs/"|href="./"|g' "$output_dir/docs/404.html"

while IFS= read -r page; do
  relative="${page#"$book_dir/"}"
  [[ "$relative" == "index.html" || "$relative" == "404.html" ]] && continue

  redirect="$output_dir/$relative"
  relative_dir="$(dirname "$relative")"
  prefix=""
  while [[ "$relative_dir" != "." ]]; do
    prefix="../$prefix"
    relative_dir="$(dirname "$relative_dir")"
  done
  target="${prefix}docs/$relative"
  mkdir -p "$(dirname "$redirect")"
  printf '%s\n' \
    '<!doctype html>' \
    '<html lang="en">' \
    '<head>' \
    '<meta charset="utf-8">' \
    "<meta http-equiv=\"refresh\" content=\"0; url=$target\">" \
    "<link rel=\"canonical\" href=\"$target\">" \
    '<title>Documentation moved</title>' \
    '</head>' \
    "<body><p>This page moved to <a href=\"$target\">$target</a>.</p></body>" \
    '</html>' > "$redirect"
done < <(find "$book_dir" -type f -name '*.html' | sort)

test -f "$output_dir/index.html"
test -f "$output_dir/docs/index.html"
