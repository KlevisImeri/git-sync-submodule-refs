import Generated.SyncSafety

set_option warningAsError true

namespace sync_safety

/-- An ordinary-file change is rejected regardless of its index/worktree status.
    This theorem is about the Aeneas translation of the classifier used by the CLI. -/
theorem ordinary_file_rejected (index worktree : ChangeStatus) :
    allowed_change false index worktree = .ok false := by
  rfl

/-- Independent specification: only the two reference-compatible statuses pass. -/
def referenceStatus (status : ChangeStatus) : Prop :=
  status = .Unmodified ∨ status = .Modified

/-- Both directions: no disallowed combination is accepted, and every allowed
    combination succeeds. This checks all constructor cases, not test samples. -/
theorem allowed_change_exact (isSubmodule : Bool) (index worktree : ChangeStatus) :
    allowed_change isSubmodule index worktree = .ok true ↔
      isSubmodule = true ∧ referenceStatus index ∧ referenceStatus worktree := by
  cases isSubmodule <;> cases index <;> cases worktree <;>
    simp [allowed_change, status_allowed, referenceStatus]

theorem allowed_change_total (isSubmodule : Bool) (index worktree : ChangeStatus) :
    ∃ result, allowed_change isSubmodule index worktree = .ok result := by
  cases isSubmodule <;> cases index <;> cases worktree <;>
    simp [allowed_change, status_allowed]

theorem failed_validation_stays_failed (accepted : Bool) :
    Validation.check .Failed accepted = .ok .Failed := by
  rfl

theorem rejected_check_closes_gate (validation : Validation) :
    validation.check false = .ok .Failed := by
  cases validation <;> rfl

theorem validation_check_exact (validation : Validation) (accepted : Bool) :
    validation.check accepted = .ok .Passed ↔
      validation = .Passed ∧ accepted = true := by
  cases validation <;> cases accepted <;> simp [Validation.check]

theorem validation_gate_exact (validation : Validation) :
    validation.passed = .ok true ↔ validation = .Passed := by
  cases validation <;> simp [Validation.passed]

theorem rejected_check_blocks_gate (validation : Validation) :
    (do
      let next ← validation.check false
      next.passed) = .ok false := by
  cases validation <;> simp [Validation.check, Validation.passed]

theorem staged_reference_exact (unchanged checkout : Bool) :
    staged_reference_allowed unchanged checkout = .ok true ↔
      unchanged = true ∨ checkout = true := by
  cases unchanged <;> cases checkout <;> simp [staged_reference_allowed]

/-- Explicit boundary assumptions for a stable snapshot of the entire tree.
    `actualChange` concerns relevant tracked/untracked changes, not ignored files.
    Completeness includes discovery of every nested repository. Faithfulness
    includes our parsing and status adaptation, NOT just Git's own correctness.
    No Rust traversal or parser theorem establishes this contract yet. -/
structure SnapshotContract (Change : Type) where
  actualChange : Change → Prop
  observed : Change → Prop
  referenceOnly : Change → Prop
  isSubmodule : Change → Bool
  index : Change → ChangeStatus
  worktree : Change → ChangeStatus
  complete : ∀ change, actualChange change → observed change
  faithful : ∀ change, observed change →
    (isSubmodule change = true ∧ referenceStatus (index change) ∧
      referenceStatus (worktree change)) → referenceOnly change

/-- Conditional snapshot result, not an end-to-end theorem about the async CLI.
    The acceptance hypothesis must eventually be established by a traversal proof.
    We do not declare Git or parsing correctness as global axioms. -/
theorem accepted_snapshot_reference_only {Change : Type}
    (contract : SnapshotContract Change)
    (accepted : ∀ change, contract.observed change →
      allowed_change (contract.isSubmodule change) (contract.index change)
        (contract.worktree change) = .ok true) :
    ∀ change, contract.actualChange change → contract.referenceOnly change := by
  intro change actual
  have observed := contract.complete change actual
  exact contract.faithful change observed
    ((allowed_change_exact _ _ _).mp (accepted change observed))

#print axioms ordinary_file_rejected
#print axioms allowed_change_exact
#print axioms allowed_change_total
#print axioms failed_validation_stays_failed
#print axioms rejected_check_closes_gate
#print axioms validation_check_exact
#print axioms validation_gate_exact
#print axioms rejected_check_blocks_gate
#print axioms staged_reference_exact
#print axioms accepted_snapshot_reference_only

end sync_safety
