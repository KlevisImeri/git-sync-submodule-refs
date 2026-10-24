import Generated.SyncSafety

set_option warningAsError true

namespace sync_safety

/-- An ordinary-file change is rejected regardless of its index/worktree status.
    This theorem is about the Aeneas translation of the classifier used by the CLI. -/
theorem ordinary_file_rejected (index worktree : ChangeStatus) :
    allowed_change false index worktree = .ok false := by
  rfl

#print axioms ordinary_file_rejected

end sync_safety
