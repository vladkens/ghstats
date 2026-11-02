#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

host=${BACKUP_HOST:-srv}

mkdir -p data
scp "$host:/root/data/ghstats.db" "data/backup-$(date -u +%Y%m%d_%H%M).db"
cp "$(ls -1t data/*.db | head -n 1)" data/backup.db
