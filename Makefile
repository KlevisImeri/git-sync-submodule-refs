.PHONY: generate-lean verify-lean

generate-lean:
	@aeneas -version | grep -Fx 'aeneas 74aabd5'
	@charon version | grep -Fx '0.1.294 (260ba2c82588291b1f9efaf4f42d7100adc9c6c2)'
	mkdir -p target/verification verification/Generated
	charon rustc --preset=aeneas --sysroot=default --dest-file target/verification/sync_safety.llbc -- --crate-type lib --crate-name sync_safety --edition=2024 src/safety.rs --out-dir target/verification
	aeneas -backend lean -dest verification/Generated target/verification/sync_safety.llbc

verify-lean: generate-lean
	cd verification && lake build
