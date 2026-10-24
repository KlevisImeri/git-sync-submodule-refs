# Lean verification

The first property is `sync_safety.ordinary_file_rejected`: an ordinary-file change is rejected for every index and working-tree status.

`src/main.rs` calls the classifier in `src/safety.rs`. Charon extracts that exact Rust file, and Aeneas generates `Generated/SyncSafety.lean`. The handwritten proof in `SafetyProofs.lean` refers to the generated function, not an independently rewritten implementation. Do not edit generated definitions manually.

## Run

Requires the globally installed translators and Elan/Lean on `PATH`:

- Aeneas commit `74aabd52f77001cf9eaad51b4f4b867a7dc6b111` (`aeneas 74aabd5`).
- Charon commit `260ba2c82588291b1f9efaf4f42d7100adc9c6c2` (`0.1.294`), with its Rust toolchain `nightly-2026-09-17`.
- Lean `4.35.0`, selected by `lean-toolchain`.

From this directory, initialize the pinned Lake dependencies once:

```sh
lake update
```

From the repository root:

```sh
make verify-lean
```

This regenerates the translation from current Rust before checking the proof, so a stale generated file cannot substitute for the current classifier. `make generate-lean` regenerates without checking proofs. Lake manages the Lean libraries locally under the ignored `.lake/` directory; no Opam switch is needed to run the installed translator.

`#print axioms ordinary_file_rejected` reports the theorem's assumptions. The proof uses no `sorry`, `admit`, custom axioms, or finite-input testing.

The checked axiom report is `[propext, Classical.choice, Quot.sound]`, Lean's standard logical axioms; the theorem is not claimed to be axiom-free.

## Scope and trust boundary

This proves rejection when `is_submodule` is false. It does not yet prove whole-tree traversal, execution ordering, staging/pushing behavior, the mapping from Git status to `ChangeStatus`, or Git parsing. Those boundaries retain integration/unit tests. As with other translated-code proofs, we trust the compiler/extraction/translation pipeline and Lean's checking infrastructure; this is not an end-to-end proof of the executable or external Git.
