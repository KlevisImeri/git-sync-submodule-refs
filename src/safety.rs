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
