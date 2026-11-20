#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
if [[ ! -f .env ]]; then
    cp .env.example .env
    chmod 600 .env
    echo 'Created .env. Set FIREBASE_PROJECT_ID to enable authenticated gameplay.'
fi
exec docker compose up --build -d "$@"
