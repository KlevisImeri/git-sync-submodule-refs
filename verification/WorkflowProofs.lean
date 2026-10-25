import Generated.SyncWorkflow

open Aeneas Aeneas.Std
set_option warningAsError true
set_option maxHeartbeats 4000000

namespace sync_workflow

/-- Ghost bookkeeping of successful external responses. Not executable workflow
    logic: all transitions being verified are `Workflow.respond` from Rust. -/
structure Audit where
  validated : Bool := false
  preflighted : Nat := 0
  published : Nat := 0
  changes : Bool := false
  staged : Bool := false
  revalidated : Bool := false

def Audit.record (a : Audit) (phase : Phase) (success changes : Bool) : Audit :=
  if !success then a else
  match phase with
  | .Validate => { a with validated := true }
  | .Preflight => { a with preflighted := a.preflighted + 1 }
  | .Prepare => { a with changes, staged := false, revalidated := false }
  | .StageReferences => { a with staged := true }
  | .Revalidate => { a with revalidated := true }
  | .Publish => { a with published := a.published + 1 }
  | _ => a

def ready (a : Audit) (s : Workflow) : Prop :=
  a.validated = true ∧ a.preflighted = s.total.val ∧
  a.published = s.cursor.val ∧ s.published.val = a.published ∧
  s.cursor.val < s.total.val

/-- Declarative protocol specification. Counters refer to successful responses,
    not to assumptions that the Rust controller followed the right order. -/
def invariant (a : Audit) (s : Workflow) : Prop :=
  match s.phase with
  | .Validate => s.cursor.val = 0 ∧ s.published.val = 0 ∧
      a.validated = false ∧ a.preflighted = 0 ∧ a.published = 0
  | .Fetch => a.validated = true ∧ a.preflighted = 0 ∧ a.published = 0 ∧
      s.published.val = 0 ∧ s.cursor.val < s.total.val
  | .Preflight => a.validated = true ∧ a.preflighted = s.cursor.val ∧
      a.published = 0 ∧ s.published.val = 0 ∧ s.cursor.val < s.total.val
  | .Prepare => ready a s
  | .StageReferences => ready a s ∧ a.changes = true
  | .Commit => ready a s ∧ a.changes = true ∧ a.staged = true
  | .Revalidate => ready a s
  | .Publish => ready a s ∧ a.revalidated = true
  | .Done => a.validated = true ∧ a.preflighted = s.total.val ∧
      a.published = s.total.val ∧ s.published.val = a.published
  | .Failed => True

theorem failure_stops (s : Workflow) (changes : Bool) :
    s.respond false changes = .ok { s with phase := .Failed } := by
  rfl

theorem failure_is_terminal (s : Workflow) (changes : Bool)
    (failed : s.phase = .Failed) : s.respond true changes = .ok s := by
  simp [Workflow.respond, failed]

theorem new_invariant (total : Usize) :
    invariant {} { phase := .Validate, cursor := 0#usize, total, published := 0#usize } := by
  simp [invariant]

theorem child_before_parent_exact (child parent : Usize) :
    child_before_parent child parent = .ok true ↔ child.val < parent.val := by
  simp [child_before_parent]

private theorem successor (x total : Usize) (lt : x.val < total.val) :
    ∃ next : Usize, (x + 1#usize : Result Usize) = .ok next ∧
      next.val = x.val + 1 := by
  have bounds : UScalar.inBounds .Usize (x.val + 1) := by scalar_tac
  have spec := UScalar.tryMkOpt_eq .Usize (x.val + 1)
  cases eq : UScalar.tryMkOpt .Usize (x.val + 1) with
  | none =>
    simp [eq] at spec
    scalar_tac
  | some next =>
    simp only [eq] at spec
    refine ⟨next, ?_, ?_⟩
    · change UScalar.tryMk .Usize (x.val + 1) = .ok next
      simp [UScalar.tryMk, eq]
    · exact spec.1

theorem respond_preserves (a : Audit) (s next : Workflow) (success changes : Bool)
    (inv : invariant a s) (transition : s.respond success changes = .ok next) :
    invariant (a.record s.phase success changes) next ∧ next.total = s.total := by
  cases success with
  | false =>
    simp [Workflow.respond] at transition
    subst next
    simp [invariant]
  | true =>
    cases phase : s.phase <;>
      simp only [Workflow.respond, phase, ↓reduceIte] at transition
    case Fetch | Preflight | Publish =>
      have lt : s.cursor.val < s.total.val := by
        simp only [invariant, phase, ready] at inv
        tauto
      obtain ⟨z, add, zval⟩ := successor s.cursor s.total lt
      have guard : ¬s.cursor >= s.total := by scalar_tac
      simp only [guard, ↓reduceIte, add, bind_ok] at transition
      split_ifs at transition
      all_goals
        simp only [Result.ok.injEq] at transition
        subst next
        simp_all [invariant, ready, Audit.record]
        all_goals omega
    all_goals
      (try split_ifs at transition) <;>
        simp only [Result.ok.injEq] at transition <;>
        subst next <;>
        simp_all [invariant, ready, Audit.record]
    all_goals omega

theorem respond_total (s : Workflow) (success changes : Bool) :
    ∃ next, s.respond success changes = .ok next := by
  cases success with
  | false => exact ⟨_, failure_stops s changes⟩
  | true =>
    cases phase : s.phase <;>
      simp only [Workflow.respond, phase, ↓reduceIte]
    case Fetch | Preflight | Publish =>
      split_ifs with guard
      · exact ⟨_, rfl⟩
      · have lt : s.cursor.val < s.total.val := by scalar_tac
        obtain ⟨z, add, _⟩ := successor s.cursor s.total lt
        simp only [add, bind_ok]
        split_ifs <;> exact ⟨_, rfl⟩
    all_goals (try split_ifs) <;> exact ⟨_, rfl⟩

structure Event where
  phase : Phase
  repo : Nat
  success : Bool
  changes : Bool

/-- Traces of the actual translated constructor and response function.
    Events are newest first. Terminal states dispatch no effects. -/
inductive Execution (total : Usize) : List Event → Audit → Workflow → Prop
  | start : Execution total [] {}
      { phase := .Validate, cursor := 0#usize, total, published := 0#usize }
  | step {history a s next success changes}
      (prior : Execution total history a s)
      (notDone : s.phase ≠ .Done) (notFailed : s.phase ≠ .Failed)
      (transition : s.respond success changes = .ok next) :
      Execution total
        ({ phase := s.phase, repo := s.cursor.val, success, changes } :: history)
        (a.record s.phase success changes) next

theorem new_exact (total : Usize) :
    Workflow.new total = .ok
      { phase := .Validate, cursor := 0#usize, total, published := 0#usize } := by
  rfl

theorem execution_invariant {total history a s}
    (execution : Execution total history a s) : invariant a s ∧ s.total = total := by
  induction execution with
  | start => exact ⟨new_invariant _, rfl⟩
  | step prior notDone notFailed transition ih =>
    have preserved := respond_preserves _ _ _ _ _ ih.1 transition
    exact ⟨preserved.1, preserved.2.trans ih.2⟩

def mutation (phase : Phase) : Prop :=
  phase = .StageReferences ∨ phase = .Commit ∨ phase = .Publish

theorem mutations_require_whole_preflight {total history a s}
    (execution : Execution total history a s) (effect : mutation s.phase) :
    a.validated = true ∧ a.preflighted = total.val := by
  obtain ⟨inv, same⟩ := execution_invariant execution
  cases phase : s.phase <;> simp_all [mutation, invariant, ready]

theorem validation_coverage {total history a s}
    (execution : Execution total history a s) (validated : a.validated = true) :
    ∃ event ∈ history, event.phase = .Validate ∧ event.success = true := by
  induction execution with
  | start => simp at validated
  | @step history a s next success changes prior notDone notFailed transition ih =>
    by_cases old : a.validated = true
    · obtain ⟨event, member, facts⟩ := ih old
      exact ⟨event, List.mem_cons_of_mem _ member, facts⟩
    · cases success <;> cases phase : s.phase <;>
        simp_all [Audit.record]

theorem publication_in_order {total history a s}
    (execution : Execution total history a s) (publish : s.phase = .Publish) :
    s.cursor.val = a.published := by
  have inv := (execution_invariant execution).1
  simp only [invariant, publish, ready] at inv
  omega

theorem mutations_in_order {total history a s}
    (execution : Execution total history a s) (effect : mutation s.phase) :
    s.cursor.val = a.published := by
  have inv := (execution_invariant execution).1
  cases phase : s.phase <;> simp_all [mutation, invariant, ready]

theorem commit_requires_approved_staging {total history a s}
    (execution : Execution total history a s) (commit : s.phase = .Commit) :
    a.changes = true ∧ a.staged = true := by
  have inv := (execution_invariant execution).1
  simp only [invariant, commit, ready] at inv
  tauto

theorem publication_requires_revalidation {total history a s}
    (execution : Execution total history a s) (publish : s.phase = .Publish) :
    a.revalidated = true := by
  have inv := (execution_invariant execution).1
  simp only [invariant, publish] at inv
  exact inv.2

theorem successful_completion_count {total history a s}
    (execution : Execution total history a s) (done : s.phase = .Done) :
    a.published = total.val := by
  obtain ⟨inv, same⟩ := execution_invariant execution
  simp_all [invariant]

def Covers (history : List Event) (phase : Phase) (count : Nat) : Prop :=
  ∀ index, index < count → ∃ event ∈ history,
    event.phase = phase ∧ event.repo = index ∧ event.success = true

private theorem covers_cons {history phase count event}
    (covered : Covers history phase count) : Covers (event :: history) phase count := by
  intro index lt
  obtain ⟨old, member, facts⟩ := covered index lt
  exact ⟨old, List.mem_cons_of_mem _ member, facts⟩

private theorem covers_next {history phase count event}
    (covered : Covers history phase count)
    (facts : event.phase = phase ∧ event.repo = count ∧ event.success = true) :
    Covers (event :: history) phase (count + 1) := by
  intro index lt
  by_cases previous : index < count
  · exact covers_cons covered index previous
  · have eq : index = count := by omega
    exact ⟨event, List.mem_cons_self, by simpa [eq] using facts⟩

theorem publication_coverage {total history a s}
    (execution : Execution total history a s) : Covers history .Publish a.published := by
  induction execution with
  | start => simp [Covers]
  | @step history a s next success changes prior notDone notFailed transition ih =>
    have lifted := @covers_cons history .Publish a.published
      { phase := s.phase, repo := s.cursor.val, success, changes } ih
    cases success <;> cases phase : s.phase <;>
      simp_all [Audit.record]
    apply covers_next ih
    exact ⟨rfl, publication_in_order prior phase, rfl⟩

theorem preflight_coverage {total history a s}
    (execution : Execution total history a s) : Covers history .Preflight a.preflighted := by
  induction execution with
  | start => simp [Covers]
  | @step history a s next success changes prior notDone notFailed transition ih =>
    have lifted := @covers_cons history .Preflight a.preflighted
      { phase := s.phase, repo := s.cursor.val, success, changes } ih
    cases success <;> cases phase : s.phase <;>
      simp_all [Audit.record]
    apply covers_next ih
    have inv := (execution_invariant prior).1
    simp only [invariant, phase] at inv
    exact ⟨rfl, inv.2.1.symm, rfl⟩

theorem mutations_follow_every_preflight {total history a s}
    (execution : Execution total history a s) (effect : mutation s.phase) :
    Covers history .Preflight total.val := by
  rw [← (mutations_require_whole_preflight execution effect).2]
  exact preflight_coverage execution

theorem successful_completion_covers_every_repository {total history a s}
    (execution : Execution total history a s) (done : s.phase = .Done) :
    Covers history .Publish total.val := by
  rw [← successful_completion_count execution done]
  exact publication_coverage execution

theorem children_published_before_parent {total history a s} (child : Usize)
    (execution : Execution total history a s) (publish : s.phase = .Publish)
    (dependency : child_before_parent child s.cursor = .ok true) :
    ∃ event ∈ history, event.phase = .Publish ∧ event.repo = child.val ∧
      event.success = true := by
  apply publication_coverage execution child.val
  rw [← publication_in_order execution publish]
  exact (child_before_parent_exact _ _).mp dependency

theorem children_published_before_parent_mutation {total history a s} (child : Usize)
    (execution : Execution total history a s) (effect : mutation s.phase)
    (dependency : child_before_parent child s.cursor = .ok true) :
    ∃ event ∈ history, event.phase = .Publish ∧ event.repo = child.val ∧
      event.success = true := by
  apply publication_coverage execution child.val
  rw [← mutations_in_order execution effect]
  exact (child_before_parent_exact _ _).mp dependency

#print axioms failure_stops
#print axioms failure_is_terminal
#print axioms new_invariant
#print axioms child_before_parent_exact
#print axioms respond_preserves
#print axioms respond_total
#print axioms execution_invariant
#print axioms mutations_require_whole_preflight
#print axioms mutations_follow_every_preflight
#print axioms commit_requires_approved_staging
#print axioms publication_requires_revalidation
#print axioms publication_in_order
#print axioms successful_completion_covers_every_repository
#print axioms children_published_before_parent
#print axioms children_published_before_parent_mutation

end sync_workflow
