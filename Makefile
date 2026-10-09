# Top-level entry points for the remill/anvill lift and Cranelift drop.
#
#   make lifters   build vendored remill (LLVM 20) and anvill-decompile-spec
#   make runner    build the Cranelift/Pulley drop runner (if not already built)
#   make demo      analyze sample_network, lift every selected function, validate
#                  on Pulley. CI runs this as the hard gate on the x86_64 build:
#                  a lift/validation failure blocks the release.

.PHONY: lifters runner demo sample

lifters:
	pipeline/build_lifters.sh

runner:
	@if [ -z "$$(find ceremony-wasm/target target -name ceremony-wasm -type f 2>/dev/null)" ]; then \
	  cargo build --manifest-path ceremony-wasm/Cargo.toml; \
	else \
	  echo "ceremony-wasm runner already built; skipping cargo build"; \
	fi

sample:
	$(MAKE) -C liftmap

demo: sample runner
	python3 liftmap/lift_drop_demo.py
