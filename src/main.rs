mod git;
mod parse;
mod safety;
mod workflow;

use anyhow::{Context, Result, bail};
use git::{bytes, git, head, predicate};
use git_spawn::parse::StatusKind;
use std::{
    env,
    path::{Path, PathBuf},
};

impl From<StatusKind> for safety::ChangeStatus {
    fn from(status: StatusKind) -> Self {
        match status {
            StatusKind::Unmodified => Self::Unmodified,
            StatusKind::Modified => Self::Modified,
            _ => Self::Other,
        }
    }
}

#[cfg(test)]
mod safety_tests {
    use super::*;

    #[test]
    fn validation_report_blocks_after_any_rejection() {
        assert!(ValidationReport::new().finish().is_ok());
        let mut report = ValidationReport::new();
        report.reject("first repository is dirty".to_owned());
        report.reject("nested repository is dirty".to_owned());
        let error = report.finish().unwrap_err().to_string();
        assert_eq!(
            error,
            "first repository is dirty\nnested repository is dirty"
        );
    }

    #[test]
    fn successful_checks_cannot_clear_a_failed_validation() {
        let mut validation = safety::Validation::Passed.check(true);
        assert!(validation.passed());
        validation = validation.check(false);
        for _ in 0..10 {
            validation = validation.check(true);
            assert!(!validation.passed());
        }
    }

    #[test]
    fn staged_reference_must_be_unchanged_or_match_checkout() {
        for unchanged in [false, true] {
            for checkout in [false, true] {
                assert_eq!(
                    safety::staged_reference_allowed(unchanged, checkout),
                    unchanged || checkout
                );
            }
        }
    }

    #[test]
    fn git_status_adapter_preserves_classification() {
        let kinds = [
            StatusKind::Unmodified,
            StatusKind::Modified,
            StatusKind::Added,
            StatusKind::Deleted,
            StatusKind::Renamed,
            StatusKind::Copied,
            StatusKind::Unmerged,
            StatusKind::Untracked,
            StatusKind::Ignored,
            StatusKind::TypeChanged,
            StatusKind::Other('x'),
        ];
        for index in kinds {
            for worktree in kinds {
                assert!(!safety::allowed_change(
                    false,
                    index.into(),
                    worktree.into()
                ));
                let expected = [index, worktree]
                    .iter()
                    .all(|s| matches!(s, StatusKind::Unmodified | StatusKind::Modified));
                assert_eq!(
                    safety::allowed_change(true, index.into(), worktree.into()),
                    expected
                );
            }
        }
    }
}

#[derive(Clone)]
struct Module {
    path: String,
    recorded: String,
    indexed: String,
}

#[derive(Clone)]
struct Repo {
    path: PathBuf,
    modules: Vec<Module>,
    branch: Option<String>,
}

/// Keep diagnostics and the shared, monotonic validation gate together.
struct ValidationReport {
    errors: Vec<String>,
    validation: safety::Validation,
}

impl ValidationReport {
    fn new() -> Self {
        Self {
            errors: Vec::new(),
            validation: safety::Validation::Passed,
        }
    }

    fn reject(&mut self, error: String) {
        let validation = std::mem::replace(&mut self.validation, safety::Validation::Failed);
        self.validation = validation.check(false);
        self.errors.push(error);
    }

    fn finish(self) -> Result<()> {
        if !self.validation.passed() {
            bail!("{}", self.errors.join("\n"));
        }
        Ok(())
    }
}

// Parse NUL-delimited records, never whitespace-delimited file names.
async fn modules(repo: &Path) -> Result<Vec<Module>> {
    let recorded = parse::gitlinks(&bytes(repo, &["ls-tree", "-rz", "HEAD"]).await?, false)
        .with_context(|| format!("{}: cannot inspect committed submodules", repo.display()))?;
    let indexed = parse::gitlinks(&bytes(repo, &["ls-files", "--stage", "-z"]).await?, true)
        .with_context(|| format!("{}: cannot inspect staged submodules", repo.display()))?;
    if !recorded.keys().eq(indexed.keys()) {
        bail!(
            "{}: added, deleted, or type-changed submodules must be committed manually",
            repo.display()
        );
    }
    Ok(recorded
        .into_iter()
        .map(|(path, recorded)| Module {
            indexed: indexed[&path].clone(),
            path,
            recorded,
        })
        .collect())
}

async fn inspect(path: &Path, repos: &mut Vec<Repo>, errors: &mut ValidationReport) -> Result<()> {
    let top = git(path, &["rev-parse", "--show-toplevel"]).await?;
    if Path::new(top.strip_suffix('\n').unwrap_or(&top))
        .canonicalize()
        .with_context(|| format!("Cannot resolve repository {}", path.display()))?
        != path.canonicalize()?
    {
        bail!("{}: submodule is not initialized", path.display());
    }
    head(path).await?;
    for state in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-merge",
        "rebase-apply",
        "sequencer",
        "BISECT_LOG",
        "index.lock",
    ] {
        let state_path = git(path, &["rev-parse", "--git-path", state]).await?;
        if path
            .join(state_path.strip_suffix('\n').unwrap_or(&state_path))
            .exists()
        {
            errors.reject(format!(
                "{}: unfinished or active Git operation ({state})",
                path.display()
            ));
        }
    }
    let modules = modules(path).await?;
    let status = bytes(
        path,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )
    .await?;
    for entry in parse::status(&status)
        .with_context(|| format!("{}: cannot inspect working tree", path.display()))?
    {
        let is_module = modules.iter().any(|m| m.path == entry.path);
        if !safety::allowed_change(is_module, entry.index.into(), entry.worktree.into()) {
            errors.reject(format!(
                "{}: uncommitted work: {:?} ({:?}/{:?})",
                path.display(),
                entry.path,
                entry.index,
                entry.worktree
            ));
        }
    }
    for module in &modules {
        let child = path.join(&module.path);
        // Git must not accidentally resolve an uninitialized child to its parent.
        if !child.join(".git").exists() {
            errors.reject(format!("{}: submodule is not initialized", child.display()));
            continue;
        }
        match head(&child).await {
            Ok(current)
                if !safety::staged_reference_allowed(
                    module.indexed == module.recorded,
                    module.indexed == current,
                ) =>
            {
                errors.reject(format!(
                    "{}: staged submodule reference differs from its checkout",
                    child.display()
                ));
            }
            Err(e) => {
                errors.reject(format!("{e:#}"));
                continue;
            }
            _ => {}
        }
        if let Err(e) = Box::pin(inspect(&child, repos, errors)).await {
            errors.reject(format!("{e:#}"));
        }
    }
    let branch_args = ["symbolic-ref", "--quiet", "--short", "HEAD"];
    let branch = if predicate(path, &branch_args).await? {
        Some(git(path, &branch_args).await?.trim().to_owned())
    } else {
        None
    };
    repos.push(Repo {
        path: path.to_owned(),
        modules,
        branch,
    });
    Ok(())
}

async fn tree(root: &Path) -> Result<Vec<Repo>> {
    let mut repos = Vec::new();
    let mut errors = ValidationReport::new();
    if let Err(e) = inspect(root, &mut repos, &mut errors).await {
        errors.reject(format!("{e:#}"));
    }
    errors.finish()?;
    Ok(repos)
}

async fn changes(repo: &Repo) -> Result<Vec<(String, String, String)>> {
    let mut changes = Vec::new();
    for module in &repo.modules {
        let current = head(&repo.path.join(&module.path)).await?;
        if module.recorded != current {
            changes.push((module.path.clone(), module.recorded.clone(), current));
        }
    }
    Ok(changes)
}

async fn head_has_remote_tag(path: &Path) -> Result<bool> {
    let commit = head(path).await?;
    let remotes = git(path, &["remote"]).await?;

    for remote in remotes.lines().filter(|line| !line.is_empty()) {
        let refs = git(path, &["ls-remote", "--tags", remote]).await?;

        for line in refs.lines() {
            let mut fields = line.split_whitespace();

            if let (Some(oid), Some(reference)) = (fields.next(), fields.next())
                && oid == commit
                && reference.starts_with("refs/tags/")
            {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

async fn publishable(repo: &Repo, will_change: bool) -> Result<()> {
    let path = &repo.path;
    if let Some(branch) = &repo.branch {
        let remote = git(
            path,
            &["config", "--get", &format!("branch.{branch}.remote")],
        )
        .await?;
        if remote.trim() == "." || remote.trim().is_empty() {
            bail!("{}: branch must track a remote repository", path.display());
        }
        git(path, &["rev-parse", "--verify", "@{upstream}"]).await?;
        if !predicate(
            path,
            &["merge-base", "--is-ancestor", "@{upstream}", "HEAD"],
        )
        .await?
        {
            bail!(
                "{}: branch is behind or diverged from its upstream; update it manually",
                path.display()
            );
        }
        if will_change {
            git(path, &["var", "GIT_AUTHOR_IDENT"]).await?;
            git(path, &["var", "GIT_COMMITTER_IDENT"]).await?;
        }
    } else {
        if will_change {
            bail!(
                "{}: cannot commit reference changes on a detached HEAD; check out a branch first",
                path.display()
            );
        }
        if git(
            path,
            &[
                "for-each-ref",
                "--format=%(refname)",
                "--contains=HEAD",
                "refs/remotes/",
            ],
        )
        .await?
        .trim()
        .is_empty()
            && !head_has_remote_tag(path).await?
        {
            bail!(
                "{}: detached HEAD has no published remote-tracking branch or matching remote tag",
                path.display()
            );
        }
    }
    Ok(())
}

struct Prepared {
    repo: Repo,
    changes: Vec<(String, String, String)>,
    message: String,
}

/// Trusted effect boundary for the translated workflow controller.
/// Contracts and the deliberately unverified adapter are documented in verification/.
struct GitBoundary {
    root: PathBuf,
    repos: Vec<Repo>,
    changing: Vec<PathBuf>,
    prepared: Option<Prepared>,
}

impl GitBoundary {
    async fn validate(&self) -> Result<()> {
        let current = tree(&self.root).await?;
        if current.len() != self.repos.len()
            || current
                .iter()
                .zip(&self.repos)
                .any(|(a, b)| a.path != b.path)
        {
            bail!("Repository tree changed during execution");
        }
        for (parent, repo) in self.repos.iter().enumerate() {
            for module in &repo.modules {
                let child_path = repo.path.join(&module.path);
                let child = self
                    .repos
                    .iter()
                    .position(|r| r.path == child_path)
                    .context("Repository tree is missing a child")?;
                if !workflow::child_before_parent(child, parent) {
                    bail!("Repository order is not child-first");
                }
            }
        }
        Ok(())
    }

    async fn fetch(&self, index: usize) -> Result<()> {
        git(
            &self.repos[index].path,
            &["fetch", "--all", "--no-recurse-submodules"],
        )
        .await?;
        Ok(())
    }

    async fn preflight(&mut self, index: usize) -> Result<()> {
        let repo = &self.repos[index];
        let will_change = !changes(repo).await?.is_empty()
            || repo
                .modules
                .iter()
                .any(|m| self.changing.contains(&repo.path.join(&m.path)));
        if will_change {
            self.changing.push(repo.path.clone());
        }
        publishable(repo, will_change).await
    }

    async fn prepare(&mut self, index: usize) -> Result<bool> {
        let original = &self.repos[index];
        let current = tree(&self.root).await?;
        let repo = current
            .into_iter()
            .find(|r| r.path == original.path)
            .context("Repository tree changed during execution")?;
        if repo.branch != original.branch {
            bail!("{}: branch changed during execution", repo.path.display());
        }
        let changes = changes(&repo).await?;
        publishable(&repo, !changes.is_empty()).await?;
        let message = changes
            .iter()
            .map(|(name, old, new)| {
                format!(
                    "[Automatic] Update submodule {name}: {} -> {}",
                    &old[..7],
                    &new[..7]
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let has_changes = !changes.is_empty();
        self.prepared = Some(Prepared {
            repo,
            changes,
            message,
        });
        Ok(has_changes)
    }

    async fn stage_references(&self) -> Result<()> {
        let prepared = self.prepared.as_ref().context("No prepared repository")?;
        for (name, _, _) in &prepared.changes {
            git(&prepared.repo.path, &["add", "--", name]).await?;
        }
        Ok(())
    }

    async fn commit(&self) -> Result<()> {
        let prepared = self.prepared.as_ref().context("No prepared repository")?;
        git(&prepared.repo.path, &["commit", "-m", &prepared.message]).await?;
        println!("{}:\n{}", prepared.repo.path.display(), prepared.message);
        Ok(())
    }

    async fn revalidate(&self) -> Result<()> {
        tree(&self.root).await?;
        Ok(())
    }

    async fn publish(&self) -> Result<()> {
        let repo = &self
            .prepared
            .as_ref()
            .context("No prepared repository")?
            .repo;
        if let Some(branch) = &repo.branch {
            let remote = git(
                &repo.path,
                &["config", "--get", &format!("branch.{branch}.remote")],
            )
            .await?;
            let target = git(
                &repo.path,
                &["config", "--get", &format!("branch.{branch}.merge")],
            )
            .await?;
            git(
                &repo.path,
                &[
                    "-c",
                    "push.followTags=false",
                    "push",
                    "--no-force",
                    "--recurse-submodules=check",
                    remote.trim(),
                    &format!("HEAD:{}", target.trim()),
                ],
            )
            .await?;
            println!("Pushed {}", repo.path.display());
        } else {
            println!("Already published (detached): {}", repo.path.display());
        }
        Ok(())
    }

    async fn execute(&mut self, phase: workflow::Phase, index: usize) -> Result<bool> {
        use workflow::Phase;
        match phase {
            Phase::Validate => self.validate().await?,
            Phase::Fetch => self.fetch(index).await?,
            Phase::Preflight => self.preflight(index).await?,
            Phase::Prepare => return self.prepare(index).await,
            Phase::StageReferences => self.stage_references().await?,
            Phase::Commit => self.commit().await?,
            Phase::Revalidate => self.revalidate().await?,
            Phase::Publish => self.publish().await?,
            Phase::Done | Phase::Failed => bail!("Terminal workflow has no Git operation"),
        }
        Ok(false)
    }
}

async fn run() -> Result<()> {
    let args: Vec<_> = env::args().skip(1).collect();
    if args == ["--help"] || args == ["-h"] {
        println!(
            "git sync-submodule-refs\n\nRecursively commit and push existing submodule reference changes.\nRefuses all other uncommitted work. Does not pull or advance checkouts.\nNo flags are needed. See README.md for safety and limitations."
        );
        return Ok(());
    }
    if !args.is_empty() {
        bail!("Usage: git sync-submodule-refs (no flags required)");
    }
    // Reject inherited repository/index overrides so -C always addresses the intended repo.
    for variable in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        if env::var_os(variable).is_some() {
            bail!("Unset {variable} before running this command");
        }
    }
    let cwd = env::current_dir().context("Cannot determine current directory")?;
    let root = git(&cwd, &["rev-parse", "--show-toplevel"]).await?;
    let root = PathBuf::from(root.strip_suffix('\n').unwrap_or(&root));
    println!("Validating {} and all nested submodules...", root.display());
    let repos = tree(&root).await?;
    let mut state = workflow::Workflow::new(repos.len());
    let mut boundary = GitBoundary {
        root,
        repos,
        changing: Vec::new(),
        prepared: None,
    };
    let mut failure = None;
    loop {
        match state.phase {
            workflow::Phase::Done => break,
            workflow::Phase::Failed => return Err(failure.context("Workflow rejected its state")?),
            _ => {}
        }
        match boundary.execute(state.phase, state.cursor).await {
            Ok(has_changes) => state = state.respond(true, has_changes),
            Err(error) => {
                state = state.respond(false, false);
                failure = Some(error);
            }
        }
    }
    println!("All submodule references are committed and pushed.");
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = run().await {
        eprintln!(
            "Error:\n{error:#}\n\nStopped. No automatic rollback is attempted. If preflight passed, earlier commits or pushes may already have succeeded."
        );
        std::process::exit(1);
    }
}
