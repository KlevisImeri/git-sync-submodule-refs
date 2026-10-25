# Lean verification

The proofs check these properties of the translated Rust implementation:

| Theorem | Guarantee |
| --- | --- |
| `ordinary_file_rejected` | Ordinary-file entries are rejected for every status. |
| `allowed_change_exact` | Acceptance is equivalent to a submodule entry with both statuses unmodified or modified. |
| `allowed_change_total` | Classification always returns a Boolean successfully. |
| `failed_validation_stays_failed` | A later successful check cannot reopen a failed validation. |
| `rejected_check_closes_gate` | Any rejected check makes validation fail. |
| `validation_check_exact` | A check passes exactly when prior validation passed and the new check succeeds. |
| `validation_gate_exact` | The gate opens exactly for the passed state. |
| `rejected_check_blocks_gate` | Checking a failure and then consulting the gate returns false. |
| `staged_reference_exact` | A staged reference is allowed exactly when unchanged or matching the checkout. |

`src/main.rs` calls the functions in `src/safety.rs`. Whole-tree inspection uses `ValidationReport`, which records rejections through the translated validation gate. Charon extracts that exact Rust file, and Aeneas generates `Generated/SyncSafety.lean`. The handwritten proofs in `SafetyProofs.lean` refer to the generated functions, not independently rewritten implementations. Do not edit generated definitions manually.

## Workflow verification

The CLI now uses `src/workflow.rs` to decide **every operation after discovery**. The async loop dispatches the controller's current phase, waits for the result, and feeds success/failure back to `Workflow::respond`. It never dispatches terminal phases. Publication preflight fails immediately on error rather than collecting further publication errors; initial whole-tree inspection still collects diagnostics across repositories.

```text
Validate entire tree and check dependency order
  → Fetch every repository
  → Preflight every repository
  → for each repository, child-first:
      Prepare (recheck tree, branch, publication eligibility, reference changes)
      → [StageReferences → Commit] only when changes exist
      → Revalidate entire tree
      → Publish (push branch or confirm detached commit already published)
  → Done
Any failed response → Failed (no further operations)
```

`make verify-lean` also regenerates `Generated/SyncWorkflow.lean` from this **actual Rust controller**. `WorkflowProofs.lean` checks:

| Theorem | Guarantee |
| --- | --- |
| `respond_total` | Every response returns a controller state successfully, including at `usize` boundaries. |
| `failure_stops`, `failure_is_terminal` | Failure closes the workflow; a later success cannot reopen it. |
| `execution_invariant` | Every finite trace of the translated controller preserves the protocol invariant. |
| `mutations_follow_every_preflight` | Before staging, committing, or publishing, every repository has a successful publication-preflight event in the history. |
| `validation_coverage` | The validated flag corresponds to a successful whole-tree validation event, not an assumed flag. |
| `commit_requires_approved_staging` | A commit requires changes reported by Prepare and a successful StageReferences operation for the current repository. The wrapper contract supplies the actual path-scope guarantee. |
| `publication_requires_revalidation` | Publication requires a successful revalidation after preparation/any commit. |
| `publication_in_order`, `publication_coverage` | Publication indices match the number of previous successful publication operations; every lower index has a successful event. |
| `children_published_before_parent_mutation` | Every child whose dependency-order predicate passed has a successful earlier publication event before a parent mutation. |
| `successful_completion_covers_every_repository` | Done implies a successful publication operation for every discovered repository, including those without reference changes. |

`Audit` records successful events for the specification; it does not replace the implementation. `Execution` is defined using equality to the generated `Workflow.respond`, and `new_exact` connects its starting state to the generated constructor. The main induction proves preservation by the generated transition function; it does **not** assume the workflow invariant or correctness of the operation ordering. These are finite-trace safety/conditional-completion results, not a termination proof for Git/network operations.

## Trusted Git boundary

`GitBoundary` in `src/main.rs` implements the external operations. We **assume these wrappers meet their contracts**; we do not prove Git itself. `GitContracts.lean` expresses operation contracts as theorem parameters, not global axioms. It derives `mutation_preconditions` and `completion_means_published` using the controller's history proofs.

| Wrapper | Assumed behavior |
| --- | --- |
| Initial `tree` discovery | Enumerates the complete nested repository tree and accurately associates paths, branches, and dependency edges with stable repository indices. |
| `validate` | Checks the entire tree and every dependency edge using the translated `child_before_parent` predicate; success means no disallowed uncommitted work or active operation. |
| `fetch` | Updates remote-tracking data without changing checked-out branches, working files, or push destinations. |
| `preflight` | Correctly predicts reference updates, including propagation to ancestors, and checks publication/identity/branch prerequisites for this repository. |
| `prepare` | Rechecks the entire tree and branch; stores exactly the current validated reference changes and accurately reports whether that list is nonempty. |
| `stage_references` | Stages only the stored approved submodule-reference paths, including when staging fails partway through. |
| `commit` | Commits the prepared staged reference changes with the automatic message, or reports failure. |
| `revalidate` | Rechecks the entire tree, including changes caused by commit hooks. |
| `publish` | Pushes the current branch using the explicit non-force refspec, or confirms a detached commit was already published; reports failures accurately. |
| Dispatcher | Executes exactly the requested operation for the controller's repository index, preserves the prepared buffer until used, reports results faithfully, and dispatches nothing in Done/Failed. |

These contracts include our Git parsing, status adaptation, discovery and command construction—not just Git's internals. The async wrappers/dispatcher are a deliberately trusted adapter, not translated/proved code. A successful edge check is connected to strict index ordering by `child_before_parent_exact`; accurate/comprehensive edge discovery and checking remain wrapper obligations.

`effects_respect_git_contracts` explicitly carries through the assumed per-wrapper staging scope and absence of branch-advancing/force operations. It is not an independent proof of command strings. The controller's operation vocabulary has no pull/merge/rebase/checkout/force-push request.

Assume no concurrent writers or malicious hooks between checks and effects. Revalidation detects dirty hooks after commits, but cannot undo a hook's side effects or rule out a race after a check. Publication facts must remain valid (for example, remote commits are not concurrently removed). Failed calls may have partially succeeded; no rollback or atomicity is claimed. Rust tests remain in place for this trusted boundary.

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

This regenerates both translations from current Rust before checking the proofs, so stale generated files cannot substitute for the current classifier or controller. `make generate-lean` regenerates without checking proofs. Lake manages the Lean libraries locally under the ignored `.lake/` directory; no Opam switch is needed to run the installed translator.

`#print axioms` reports each theorem's assumptions. The proofs use no `sorry`, `admit`, custom axioms, or finite-input testing.

The checked axiom reports contain only subsets of `[propext, Classical.choice, Quot.sound]`, Lean's standard logical axioms; these theorems are not claimed to be axiom-free.

## Explicit snapshot assumptions

`SnapshotContract` expresses assumptions as theorem parameters rather than global axioms:

- **Completeness:** every relevant actual change in the entire nested tree appears in the observations. Ignored files are outside this policy.
- **Faithfulness:** observed submodule identity and statuses accurately imply a reference-only change when the specified conditions hold.
- **Stable snapshot:** the supplied facts concern the same world; using them across operations additionally requires no concurrent changes or mutating hooks.

`accepted_snapshot_reference_only` proves that, under this contract and the hypothesis that every observation was accepted by the actual translated classifier, every relevant actual change is reference-only.

This conditional theorem is not a proof of Rust traversal. In particular, assuming Git works does **not** establish our discovery, parsing, status adapter, or collection of all acceptance results. Those remain proof obligations. The contract deliberately exposes that gap rather than silently assuming the CLI is correct. It is also not a contract or proof for staging, commits, or pushes yet.

## Scope and trust boundary

The translated Rust controller's sequencing, gating, failure behavior, child-first processing of checked dependencies, and successful-completion coverage are proved. Their meaning for real repositories is conditional on the explicit Git-wrapper and dispatcher contracts above. Discovery/parsing, wrapper implementation, external Git, hooks, authentication/networking, concurrent state changes, and compiler/extraction/translation correctness are not proved here. Lean checks the generated definitions and handwritten theorems. This is conditional verification of the implemented controller, not an unconditional end-to-end proof of the executable.
