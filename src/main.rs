mod git;
mod parse;
mod safety;

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

async fn inspect(path: &Path, repos: &mut Vec<Repo>, errors: &mut Vec<String>) -> Result<()> {
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
            errors.push(format!(
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
            errors.push(format!(
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
            errors.push(format!("{}: submodule is not initialized", child.display()));
            continue;
        }
        match head(&child).await {
            Ok(current) if module.indexed != module.recorded && module.indexed != current => {
                errors.push(format!(
                    "{}: staged submodule reference differs from its checkout",
                    child.display()
                ));
            }
            Err(e) => {
                errors.push(format!("{e:#}"));
                continue;
            }
            _ => {}
        }
        if let Err(e) = Box::pin(inspect(&child, repos, errors)).await {
            errors.push(format!("{e:#}"));
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
    let mut errors = Vec::new();
    if let Err(e) = inspect(root, &mut repos, &mut errors).await {
        errors.push(format!("{e:#}"));
    }
    if !errors.is_empty() {
        bail!("{}", errors.join("\n"));
    }
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
        {
            bail!(
                "{}: detached HEAD has no published remote-tracking reference",
                path.display()
            );
        }
    }
    Ok(())
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
    // Fetch is read-only with respect to working files/commits and push destinations.
    for repo in &repos {
        git(&repo.path, &["fetch", "--all", "--no-recurse-submodules"]).await?;
    }
    // An inner pointer commit also changes every ancestor's pointer.
    let mut changing = Vec::<PathBuf>::new();
    let mut errors = Vec::new();
    for repo in &repos {
        let will_change = !changes(repo).await?.is_empty()
            || repo
                .modules
                .iter()
                .any(|m| changing.contains(&repo.path.join(&m.path)));
        if will_change {
            changing.push(repo.path.clone());
        }
        if let Err(e) = publishable(repo, will_change).await {
            errors.push(format!("{e:#}"));
        }
    }
    if !errors.is_empty() {
        bail!("{}", errors.join("\n"));
    }
    println!("Preflight passed for {} repositories.", repos.len());
    for original in &repos {
        // Recheck the whole tree before each operation, including after hooks run.
        let current = tree(&root).await?;
        let repo = current
            .iter()
            .find(|r| r.path == original.path)
            .context("Repository tree changed during execution")?;
        if repo.branch != original.branch {
            bail!("{}: branch changed during execution", repo.path.display());
        }
        let changes = changes(repo).await?;
        publishable(repo, !changes.is_empty()).await?;
        if !changes.is_empty() {
            let messages: Vec<_> = changes
                .iter()
                .map(|(name, old, new)| {
                    format!(
                        "[Automatic] Update submodule {name}: {} -> {}",
                        &old[..7],
                        &new[..7]
                    )
                })
                .collect();
            let message = messages.join("\n");
            for (name, _, _) in &changes {
                git(&repo.path, &["add", "--", name]).await?;
            }
            git(&repo.path, &["commit", "-m", &message]).await?;
            println!("{}:\n{message}", repo.path.display());
        }
        tree(&root).await?;
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
            // Explicit refspec avoids push.default, push.followTags, and remote push refspec surprises.
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
