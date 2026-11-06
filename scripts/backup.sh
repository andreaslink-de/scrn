#!/usr/bin/env bash
# Creates a backup of a SCRN installation that runs with Docker Compose:
# a consistent copy of the SQLite database (taken while the server keeps
# running) plus all screenshot files, packed into one archive.
#
# Usage: scripts/backup.sh [target directory]
#   Run it from the directory that contains compose.yaml and ./data.
#   The target defaults to ./backups. Archives are named
#   scrn-YYYYMMDD-HHMMSS.tar.gz.
#
# Restore: see "Backup and restore" in README.md.

set -euo pipefail

target_dir="${1:-./backups}"
data_dir="${SCRN_HOST_DATA_DIR:-./data}"
stamp="$(date -u +%Y%m%d-%H%M%S)"
snapshot="backup-${stamp}.db"
archive="${target_dir}/scrn-${stamp}.tar.gz"

if [[ ! -d "${data_dir}/files" ]]; then
  echo "error: ${data_dir}/files not found; run this next to compose.yaml" >&2
  exit 1
fi
mkdir -p "${target_dir}"

cleanup() {
  rm -f "${data_dir:?}/${snapshot}"
  rm -rf "${staging:-}"
}
trap cleanup EXIT

# 1. Consistent database snapshot inside the data volume.
docker compose exec -T scrn scrn-server backup "/data/${snapshot}"

# 2. Archive the snapshot as scrn.db together with the files. Files are
#    copied after the snapshot, so every screenshot in the snapshot has its
#    files (unless it is deleted in between); newer files are cleaned up
#    after a restore by the maintenance job.
staging="$(mktemp -d)"
ln "${data_dir}/${snapshot}" "${staging}/scrn.db" 2>/dev/null \
  || cp "${data_dir}/${snapshot}" "${staging}/scrn.db"
tar -czf "${archive}.partial" \
  -C "${staging}" scrn.db \
  -C "$(cd "${data_dir}" && pwd)" files
mv "${archive}.partial" "${archive}"

echo "backup written to ${archive}"
