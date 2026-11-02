#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

dir=assets/vendor
mkdir -p "$dir"

curl -fsSL -o "$dir/pico.min.css" https://unpkg.com/@picocss/pico@2/css/pico.min.css
curl -fsSL -o "$dir/chart.umd.js" https://unpkg.com/chart.js@4/dist/chart.umd.js
curl -fsSL -o "$dir/luxon.min.js" https://unpkg.com/luxon@3/build/global/luxon.min.js
curl -fsSL -o "$dir/chartjs-adapter-luxon.umd.min.js" https://unpkg.com/chartjs-adapter-luxon@1/dist/chartjs-adapter-luxon.umd.min.js
curl -fsSL -o "$dir/htmx.min.js" https://unpkg.com/htmx.org@2/dist/htmx.min.js

ls -lh "$dir"
