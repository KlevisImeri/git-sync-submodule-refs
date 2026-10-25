//! The CLI's effect protocol. No Git, filesystem, or async implementation lives here.
//! A response is supplied only after the requested operation has completed.

#[derive(Clone, Copy)]
pub enum Phase {
    Validate,
    Fetch,
    Preflight,
    Prepare,
    StageReferences,
    Commit,
    Revalidate,
    Publish,
    Done,
    Failed,
}

#[derive(Clone, Copy)]
pub struct Workflow {
    pub phase: Phase,
    pub cursor: usize,
    pub total: usize,
    /// Number of repositories whose publication operation succeeded.
    pub published: usize,
}

impl Workflow {
    pub fn new(total: usize) -> Self {
        Self {
            phase: Phase::Validate,
            cursor: 0,
            total,
            published: 0,
        }
    }

    /// `has_changes` is meaningful only for a successful Prepare response.
    pub fn respond(mut self, succeeded: bool, has_changes: bool) -> Self {
        if !succeeded {
            self.phase = Phase::Failed;
            return self;
        }
        match self.phase {
            Phase::Validate => {
                self.phase = if self.total == 0 {
                    Phase::Done
                } else {
                    Phase::Fetch
                };
            }
            Phase::Fetch | Phase::Preflight | Phase::Publish => {
                // This guard also makes malformed states fail closed.
                if self.cursor >= self.total {
                    self.phase = Phase::Failed;
                    return self;
                }
                let next = self.cursor + 1;
                match self.phase {
                    Phase::Fetch => {
                        if next == self.total {
                            self.cursor = 0;
                            self.phase = Phase::Preflight;
                        } else {
                            self.cursor = next;
                        }
                    }
                    Phase::Preflight => {
                        if next == self.total {
                            self.cursor = 0;
                            self.phase = Phase::Prepare;
                        } else {
                            self.cursor = next;
                        }
                    }
                    _ => {
                        self.cursor = next;
                        self.published = next;
                        self.phase = if next == self.total {
                            Phase::Done
                        } else {
                            Phase::Prepare
                        };
                    }
                }
            }
            Phase::Prepare => {
                self.phase = if has_changes {
                    Phase::StageReferences
                } else {
                    Phase::Revalidate
                };
            }
            Phase::StageReferences => self.phase = Phase::Commit,
            Phase::Commit => self.phase = Phase::Revalidate,
            Phase::Revalidate => self.phase = Phase::Publish,
            Phase::Done | Phase::Failed => {}
        }
        self
    }
}

/// The observed dependency graph must agree with the execution order.
/// The Git boundary supplies accurate edges; this predicate checks their order.
pub fn child_before_parent(child: usize, parent: usize) -> bool {
    child < parent
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(phase: Phase) -> u8 {
        match phase {
            Phase::Validate => 0,
            Phase::Fetch => 1,
            Phase::Preflight => 2,
            Phase::Prepare => 3,
            Phase::StageReferences => 4,
            Phase::Commit => 5,
            Phase::Revalidate => 6,
            Phase::Publish => 7,
            Phase::Done => 8,
            Phase::Failed => 9,
        }
    }

    #[test]
    fn whole_preflight_precedes_inside_out_publication() {
        let mut state = Workflow::new(3);
        let mut trace = Vec::new();
        while !matches!(state.phase, Phase::Done | Phase::Failed) {
            trace.push((code(state.phase), state.cursor));
            state = state.respond(true, true);
        }
        let mut expected = vec![(0, 0)];
        for phase in [1, 2] {
            expected.extend((0..3).map(|index| (phase, index)));
        }
        for index in 0..3 {
            expected.extend((3..=7).map(|phase| (phase, index)));
        }
        assert_eq!(trace, expected);
        assert!(matches!(state.phase, Phase::Done));
        assert_eq!(state.published, 3);
    }

    #[test]
    fn every_failure_is_terminal() {
        let mut state = Workflow::new(2);
        while !matches!(state.phase, Phase::Done | Phase::Failed) {
            let stopped = state.respond(false, false);
            assert!(matches!(stopped.phase, Phase::Failed));
            assert!(matches!(stopped.respond(true, true).phase, Phase::Failed));
            assert_eq!(stopped.cursor, state.cursor);
            assert_eq!(stopped.published, state.published);
            state = state.respond(true, true);
        }
    }

    #[test]
    fn no_changes_still_revalidates_and_publishes() {
        let mut state = Workflow::new(1);
        let mut trace = Vec::new();
        while !matches!(state.phase, Phase::Done | Phase::Failed) {
            trace.push(code(state.phase));
            state = state.respond(true, false);
        }
        assert_eq!(trace, [0, 1, 2, 3, 6, 7]);
        assert_eq!(state.published, 1);
    }

    #[test]
    fn empty_plan_validates_before_completion() {
        let state = Workflow::new(0);
        assert!(matches!(state.phase, Phase::Validate));
        let state = state.respond(true, false);
        assert!(matches!(state.phase, Phase::Done));
        assert_eq!(state.published, 0);
    }

    #[test]
    fn cursor_boundaries_fail_closed_without_overflow() {
        for phase in [Phase::Fetch, Phase::Preflight, Phase::Publish] {
            let invalid = Workflow {
                phase,
                cursor: usize::MAX,
                total: usize::MAX,
                published: 0,
            };
            assert!(matches!(invalid.respond(true, false).phase, Phase::Failed));
            let last = Workflow {
                cursor: usize::MAX - 1,
                ..invalid
            };
            let advanced = last.respond(true, false);
            assert!(!matches!(advanced.phase, Phase::Failed));
        }
    }

    #[test]
    fn dependencies_must_precede_their_parents() {
        assert!(child_before_parent(0, 1));
        assert!(!child_before_parent(1, 1));
        assert!(!child_before_parent(2, 1));
    }
}
