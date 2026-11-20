#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
: "${DATABASE_URL:?Set DATABASE_URL to an empty Epic_Game_V PostgreSQL database}"
for script in db/init/*.sql; do
    psql "$DATABASE_URL" -X --set ON_ERROR_STOP=1 --file "$script"
done
