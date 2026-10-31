#!/usr/bin/env bash
# Run on an approved build worker, not the live IRC host.
set -euo pipefail
cd "$(dirname "$0")/.."
command -v cargo >/dev/null || { echo 'Rust 1.99 and Cargo are required.' >&2; exit 2; }
case "${1:-}" in
  --prepare)
    cargo fmt --all
    python scripts/generate-toolkit-patch.py
    cargo generate-lockfile
    echo 'Prepared formatting, extraction patch and lockfile. Review and commit these before promotion.'
    ;;
  '')
    test -f Cargo.lock || { echo 'No Cargo.lock. Run with --prepare on an approved worker, then review it.' >&2; exit 2; }
    ;;
  *) echo 'Usage: scripts/qualify.sh [--prepare]' >&2; exit 2 ;;
esac
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
python -m pytest -q tests/test_artifact_contracts.py
# No publishing, service restart, configuration apply or deployment is performed.
