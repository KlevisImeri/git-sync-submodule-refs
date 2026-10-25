//! Pure change classification, shared by the CLI and the Lean translation.

pub enum ChangeStatus {
    Unmodified,
    Modified,
    Other,
}

fn status_allowed(status: ChangeStatus) -> bool {
    match status {
        ChangeStatus::Unmodified | ChangeStatus::Modified => true,
        ChangeStatus::Other => false,
    }
}

pub fn allowed_change(is_submodule: bool, index: ChangeStatus, worktree: ChangeStatus) -> bool {
    is_submodule && status_allowed(index) && status_allowed(worktree)
}

/// A failed check cannot be cleared by a later successful check.
pub enum Validation {
    Passed,
    Failed,
}

impl Validation {
    pub fn check(self, accepted: bool) -> Self {
        match self {
            Self::Passed if accepted => Self::Passed,
            _ => Self::Failed,
        }
    }

    pub fn passed(&self) -> bool {
        matches!(self, Self::Passed)
    }
}

/// A staged reference must either be unchanged or match the current checkout.
pub fn staged_reference_allowed(
    index_matches_recorded: bool,
    index_matches_checkout: bool,
) -> bool {
    index_matches_recorded || index_matches_checkout
}
