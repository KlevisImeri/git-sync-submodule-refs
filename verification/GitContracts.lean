import WorkflowProofs

set_option warningAsError true
open Aeneas.Std

namespace sync_workflow

/-- Contracts of individual external operations, not assumptions about workflow
    ordering. Predicates describe the actual repositories/effects, supplied by
    the Git adapter. They are hypotheses, not global axioms.

    The snapshot/publication predicates require the documented stable-world
    conditions (no concurrent writers, malicious hooks, or remote ref deletion).
    Discovery is complete and repository indices identify the same repositories
    throughout execution. Validate includes the complete nested tree. -/
structure GitContract (history : List Event) where
  treeSafe : Prop
  publishable : Nat → Prop
  published : Nat → Prop
  stagesOnlyApprovedReferences : Event → Prop
  noBranchAdvancementOrForce : Event → Prop
  validate : ∀ event ∈ history, event.phase = .Validate →
    event.success = true → treeSafe
  preflight : ∀ event ∈ history, event.phase = .Preflight →
    event.success = true → publishable event.repo
  publish : ∀ event ∈ history, event.phase = .Publish →
    event.success = true → published event.repo
  stage : ∀ event ∈ history, event.phase = .StageReferences →
    stagesOnlyApprovedReferences event
  commands : ∀ event ∈ history, noBranchAdvancementOrForce event

/-- No workflow-wide safety assumption: this follows from the translated
    controller's trace theorems plus per-operation Git contracts. -/
theorem mutation_preconditions {total history a s}
    (execution : Execution total history a s)
    (git : GitContract history) (effect : mutation s.phase) :
    git.treeSafe ∧ ∀ repo, repo < total.val → git.publishable repo := by
  obtain ⟨event, member, phase, success⟩ := validation_coverage execution
    (mutations_require_whole_preflight execution effect).1
  refine ⟨git.validate event member phase success, ?_⟩
  intro repo lt
  obtain ⟨check, member, phase, index, success⟩ :=
    mutations_follow_every_preflight execution effect repo lt
  simpa [index] using git.preflight check member phase success

theorem completion_means_published {total history a s}
    (execution : Execution total history a s) (git : GitContract history)
    (done : s.phase = .Done) :
    ∀ repo, repo < total.val → git.published repo := by
  intro repo lt
  obtain ⟨event, member, phase, index, success⟩ :=
    successful_completion_covers_every_repository execution done repo lt
  simpa [index] using git.publish event member phase success

theorem effects_respect_git_contracts {history} (git : GitContract history) :
    (∀ event ∈ history, event.phase = .StageReferences →
      git.stagesOnlyApprovedReferences event) ∧
    (∀ event ∈ history, git.noBranchAdvancementOrForce event) := by
  exact ⟨git.stage, git.commands⟩

#print axioms mutation_preconditions
#print axioms completion_means_published
#print axioms effects_respect_git_contracts

end sync_workflow
