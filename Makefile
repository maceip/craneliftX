# Top-level entry points for the remill/anvill lift and Cranelift drop.
#
#   make lifters   build vendored remill and anvill-decompile-spec (one LLVM)
#   make runner    build the Cranelift/Pulley drop runner
#   make demo      analyze sample_network and validate every lift on Pulley

.PHONY: lifters runner demo sample

lifters:
	pipeline/build_lifters.sh

runner:
	cargo build --manifest-path ceremony-wasm/Cargo.toml

sample:
	$(MAKE) -C liftmap

demo: sample runner
	python3 liftmap/lift_drop_demo.py
