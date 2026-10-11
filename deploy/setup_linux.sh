#!/usr/bin/env bash
# Build from the repository root on Ubuntu with stable Rust and Node 22+ installed.
set -euo pipefail
cd "$(dirname "$0")/.."
sudo apt-get update
sudo apt-get install -y --no-install-recommends build-essential lld nginx python3 python3-venv
rustup target add --toolchain stable x86_64-unknown-linux-gnu
cargo +stable build --locked --release --bin isr-demo --bin isr-process
python3 scripts/build_web_fixture.py
python3 -m venv .venv
.venv/bin/python -m pip install -r webapp/requirements.txt
npm --prefix webapp ci
npm --prefix webapp run typecheck
npm --prefix webapp run build
.venv/bin/python -m unittest discover -s webapp/tests -p 'test_*.py'
echo 'Built and checked. Deploy this checkout at /var/www/craneliftx.'
echo 'Install deploy/craneliftx.service and deploy/nginx.conf, then restart those services.'
