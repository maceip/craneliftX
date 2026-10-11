# The active project is the in-process ISR Rust crate at the repository root.
.PHONY: all build check test demo
CARGO ?= cargo

all: check

build:
	RUSTFLAGS="-D warnings" $(CARGO) build --locked --all-targets

check:
	$(CARGO) fmt --all -- --check
	$(CARGO) clippy --locked --all-targets -- -D warnings
	RUSTFLAGS="-D warnings" $(CARGO) test --locked --all-targets

test:
	RUSTFLAGS="-D warnings" $(CARGO) test --locked --all-targets

demo:
	$(CARGO) run --locked --quiet

.PHONY: web-build web-check web-demo
web-build:
	$(CARGO) build --locked --release --bin isr-demo --bin isr-process
	python3 scripts/build_web_fixture.py
	npm --prefix webapp run typecheck
	npm --prefix webapp run build

web-check: web-build
	npm --prefix webapp run test:server
	npm --prefix webapp run test:e2e

web-demo: web-build
	.venv/bin/python webapp/server.py --host 127.0.0.1 --port 8877

.PHONY: verify-network
verify-network:
	$(CARGO) build --locked --release --bin isr-process
	python3 scripts/verify_rv64_network_server.py

.PHONY: verify-live
verify-live:
	$(CARGO) build --locked --release --bin isr-process --bin isr-events
	python3 scripts/verify_live_translation.py
