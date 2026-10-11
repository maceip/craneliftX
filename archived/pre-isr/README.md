# Historical pipeline entry points

These files describe the superseded whole-binary/LLVM/Pulley work. They are
preserved as reference and are not instructions for the current project.

The former root Makefile and automatic release workflow are parked here so the
normal build and CI use only the new root ISR crate. The old source directories
(`ceremony`, `ceremony-wasm`, `probe`, `pipeline`, `liftmap`, `webapp`, `vendor`,
`ci`, `docker`, and `deploy`) remain in their original locations to preserve
history and local work. They are not Cargo workspace members or dependencies.
Paths inside these historical documents refer to their original layout.

See the [current README](../../README.md) for the active architecture.
