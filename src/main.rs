use std::{
    collections::BTreeMap,
    env,
    path::{Path, PathBuf},
    process::{Command, Output},
};

type Result<T> = std::result::Result<T, String>;

fn command(repo: &Path, args: &[&str]) -> Result<Output> {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("{}: cannot run Git: {e}", repo.display()))
}

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let output = command(repo, args)?;
    if !output.status.success() {
        return Err(format!(
            "{}: git {} failed:\n{}",
            repo.display(),
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|_| format!("{}: non-UTF-8 Git output is not supported", repo.display()))
}

fn succeeds(repo: &Path, args: &[&str]) -> Result<bool> {
    Ok(command(repo, args)?.status.success())
}

fn head(repo: &Path) -> Result<String> {
    Ok(git(repo, &["rev-parse", "--verify", "HEAD"])?.trim().into())
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
fn modules(repo: &Path) -> Result<Vec<Module>> {
    let mut recorded = BTreeMap::new();
    for entry in git(repo, &["ls-tree", "-rz", "HEAD"])?
        .split('\0')
        .filter(|s| !s.is_empty())
    {
        let (meta, path) = entry.split_once('\t').ok_or("Invalid ls-tree output")?;
        let fields: Vec<_> = meta.split(' ').collect();
        if fields.first() == Some(&"160000") {
            recorded.insert(path.to_owned(), fields[2].to_owned());
        }
    }
    let mut indexed = BTreeMap::new();
    for entry in git(repo, &["ls-files", "--stage", "-z"])?
        .split('\0')
        .filter(|s| !s.is_empty())
    {
        let (meta, path) = entry.split_once('\t').ok_or("Invalid ls-files output")?;
        let fields: Vec<_> = meta.split(' ').collect();
        if fields.get(2) != Some(&"0") {
            return Err(format!(
                "{}: unresolved conflict at {path:?}",
                repo.display()
            ));
        }
        if fields.first() == Some(&"160000") {
            indexed.insert(path.to_owned(), fields[1].to_owned());
        }
    }
    if !recorded.keys().eq(indexed.keys()) {
        return Err(format!(
            "{}: added, deleted, or type-changed submodules must be committed manually",
            repo.display()
        ));
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

fn inspect(path: &Path, repos: &mut Vec<Repo>, errors: &mut Vec<String>) -> Result<()> {
    let top = git(path, &["rev-parse", "--show-toplevel"])?;
    if Path::new(top.trim())
        .canonicalize()
        .map_err(|e| e.to_string())?
        != path.canonicalize().map_err(|e| e.to_string())?
    {
        return Err(format!("{}: submodule is not initialized", path.display()));
    }
    head(path)?;
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
        let state_path = git(path, &["rev-parse", "--git-path", state])?;
        if path.join(state_path.trim()).exists() {
            errors.push(format!(
                "{}: unfinished or active Git operation ({state})",
                path.display()
            ));
        }
    }
    let modules = modules(path)?;
    let status = git(
        path,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )?;
    let mut entries = status.split('\0').filter(|s| !s.is_empty());
    while let Some(entry) = entries.next() {
        let name = entry.get(3..).ok_or("Invalid status output")?;
        let xy = &entry.as_bytes()[..2];
        let is_module = modules.iter().any(|m| m.path == name);
        if !is_module || xy.iter().any(|b| !matches!(b, b' ' | b'M')) {
            errors.push(format!("{}: uncommitted work: {entry:?}", path.display()));
        }
        if xy.contains(&b'R') || xy.contains(&b'C') {
            entries.next();
        }
    }
    for module in &modules {
        let child = path.join(&module.path);
        // Git must not accidentally resolve an uninitialized child to its parent.
        if !child.join(".git").exists() {
            errors.push(format!("{}: submodule is not initialized", child.display()));
            continue;
        }
        match head(&child) {
            Ok(current) if module.indexed != module.recorded && module.indexed != current => {
                errors.push(format!(
                    "{}: staged submodule reference differs from its checkout",
                    child.display()
                ));
            }
            Err(e) => {
                errors.push(e);
                continue;
            }
            _ => {}
        }
        if let Err(e) = inspect(&child, repos, errors) {
            errors.push(e);
        }
    }
    let branch = command(path, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    let branch = if branch.status.success() {
        Some(
            String::from_utf8(branch.stdout)
                .map_err(|e| e.to_string())?
                .trim()
                .to_owned(),
        )
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

fn tree(root: &Path) -> Result<Vec<Repo>> {
    let mut repos = Vec::new();
    let mut errors = Vec::new();
    if let Err(e) = inspect(root, &mut repos, &mut errors) {
        errors.push(e);
    }
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    Ok(repos)
}

fn changes(repo: &Repo) -> Result<Vec<(String, String, String)>> {
    let mut changes = Vec::new();
    for module in &repo.modules {
        let current = head(&repo.path.join(&module.path))?;
        if module.recorded != current {
            changes.push((module.path.clone(), module.recorded.clone(), current));
        }
    }
    Ok(changes)
}

fn publishable(repo: &Repo, will_change: bool) -> Result<()> {
    let path = &repo.path;
    if let Some(branch) = &repo.branch {
        let remote = git(
            path,
            &["config", "--get", &format!("branch.{branch}.remote")],
        )?;
        if remote.trim() == "." || remote.trim().is_empty() {
            return Err(format!(
                "{}: branch must track a remote repository",
                path.display()
            ));
        }
        git(path, &["rev-parse", "--verify", "@{upstream}"])?;
        if !succeeds(
            path,
            &["merge-base", "--is-ancestor", "@{upstream}", "HEAD"],
        )? {
            return Err(format!(
                "{}: branch is behind or diverged from its upstream; update it manually",
                path.display()
            ));
        }
        if will_change {
            git(path, &["var", "GIT_AUTHOR_IDENT"])?;
            git(path, &["var", "GIT_COMMITTER_IDENT"])?;
        }
    } else {
        if will_change {
            return Err(format!(
                "{}: cannot commit reference changes on a detached HEAD; check out a branch first",
                path.display()
            ));
        }
        if git(
            path,
            &[
                "for-each-ref",
                "--format=%(refname)",
                "--contains=HEAD",
                "refs/remotes/",
            ],
        )?
        .trim()
        .is_empty()
        {
            return Err(format!(
                "{}: detached HEAD has no published remote-tracking reference",
                path.display()
            ));
        }
    }
    Ok(())
}

fn run() -> Result<()> {
    let args: Vec<_> = env::args().skip(1).collect();
    if args == ["--help"] || args == ["-h"] {
        println!(
            "git sync-submodule-refs\n\nRecursively commit and push existing submodule reference changes.\nRefuses all other uncommitted work. Does not pull or advance checkouts.\nNo flags are needed. See README.md for safety and limitations."
        );
        return Ok(());
    }
    if !args.is_empty() {
        return Err("Usage: git sync-submodule-refs (no flags required)".into());
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
            return Err(format!("Unset {variable} before running this command"));
        }
    }
    let cwd = env::current_dir().map_err(|e| e.to_string())?;
    let root = PathBuf::from(git(&cwd, &["rev-parse", "--show-toplevel"])?.trim());
    println!("Validating {} and all nested submodules...", root.display());
    let repos = tree(&root)?;
    // Fetch is read-only with respect to working files/commits and push destinations.
    for repo in &repos {
        git(&repo.path, &["fetch", "--all", "--no-recurse-submodules"])?;
    }
    // An inner pointer commit also changes every ancestor's pointer.
    let mut changing = Vec::<PathBuf>::new();
    let mut errors = Vec::new();
    for repo in &repos {
        let will_change = !changes(repo)?.is_empty()
            || repo
                .modules
                .iter()
                .any(|m| changing.contains(&repo.path.join(&m.path)));
        if will_change {
            changing.push(repo.path.clone());
        }
        if let Err(e) = publishable(repo, will_change) {
            errors.push(e);
        }
    }
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    println!("Preflight passed for {} repositories.", repos.len());
    for original in &repos {
        // Recheck the whole tree before each operation, including after hooks run.
        let current = tree(&root)?;
        let repo = current
            .iter()
            .find(|r| r.path == original.path)
            .ok_or("Repository tree changed during execution")?;
        if repo.branch != original.branch {
            return Err(format!(
                "{}: branch changed during execution",
                repo.path.display()
            ));
        }
        let changes = changes(repo)?;
        publishable(repo, !changes.is_empty())?;
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
                git(&repo.path, &["add", "--", name])?;
            }
            git(&repo.path, &["commit", "-m", &message])?;
            println!("{}:\n{message}", repo.path.display());
        }
        tree(&root)?;
        if let Some(branch) = &repo.branch {
            let remote = git(
                &repo.path,
                &["config", "--get", &format!("branch.{branch}.remote")],
            )?;
            let target = git(
                &repo.path,
                &["config", "--get", &format!("branch.{branch}.merge")],
            )?;
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
            )?;
            println!("Pushed {}", repo.path.display());
        } else {
            println!("Already published (detached): {}", repo.path.display());
        }
    }
    println!("All submodule references are committed and pushed.");
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!(
            "Error:\n{error}\n\nStopped. No automatic rollback is attempted. If preflight passed, earlier commits or pushes may already have succeeded."
        );
        std::process::exit(1);
    }
}
