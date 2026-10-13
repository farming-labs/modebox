#!/usr/bin/env bash
# Renders the social preview image and the icon fallbacks with headless Chrome.
# Run it after changing og.html or icon.html, and commit the results.
set -euo pipefail
cd "$(dirname "$0")"
CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"

shot() { # page, width,height, output
  "$CHROME" --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=1 \
    --virtual-time-budget=3000 --window-size="$2" --screenshot="$3" "file://$PWD/$1" >/dev/null 2>&1
}

shot og.html 1200,630 ../../src/app/opengraph-image.png
shot icon.html 180,180 ../../public/apple-touch-icon.png
tmp=$(mktemp -d)
shot icon.html 32,32 "$tmp/favicon.png"
sips -s format ico "$tmp/favicon.png" --out ../../public/favicon.ico >/dev/null
rm -rf "$tmp"
echo "rendered opengraph-image.png, apple-touch-icon.png and favicon.ico"
