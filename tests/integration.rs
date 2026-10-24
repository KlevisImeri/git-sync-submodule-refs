use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn command(path: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(path)
        .env("GIT_ALLOW_PROTOCOL", "file")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    cmd
}

fn git(path: &Path, args: &[&str]) -> String {
    let output = command(path).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}

fn identity(path: &Path) {
    git(path, &["config", "user.name", "Test User"]);
    git(path, &["config", "user.email", "test@example.invalid"]);
    git(path, &["config", "commit.gpgsign", "false"]);
}

struct Fixture {
    base: PathBuf,
    root: PathBuf,
    child: PathBuf,
    grand: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = if Path::new("/tmp/opencode").is_dir() {
            PathBuf::from("/tmp/opencode")
        } else {
            std::env::temp_dir()
        };
        let base = temp.join(format!(
            "git-sync-refs-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&base).unwrap();
        for name in ["grand", "child", "root"] {
            let bare = base.join(format!("{name}.git"));
            fs::create_dir(&bare).unwrap();
            git(&bare, &["init", "--bare", "-b", "main"]);
            let work = base.join(format!("{name}-source"));
            fs::create_dir(&work).unwrap();
            git(&work, &["init", "-b", "main"]);
            identity(&work);
            fs::write(work.join("file.txt"), "initial\n").unwrap();
            git(&work, &["add", "."]);
            git(&work, &["commit", "-m", "initial"]);
            git(&work, &["remote", "add", "origin", bare.to_str().unwrap()]);
            git(&work, &["push", "-u", "origin", "main"]);
        }
        let source = base.join("child-source");
        git(
            &source,
            &[
                "submodule",
                "add",
                "-b",
                "main",
                base.join("grand.git").to_str().unwrap(),
                "nested",
            ],
        );
        git(&source, &["commit", "-am", "add nested"]);
        git(&source, &["push"]);
        let root = base.join("root-source");
        git(
            &root,
            &[
                "submodule",
                "add",
                "-b",
                "main",
                base.join("child.git").to_str().unwrap(),
                "child module",
            ],
        );
        git(&root, &["commit", "-am", "add child"]);
        git(&root, &["push"]);
        git(&root, &["submodule", "update", "--init", "--recursive"]);
        let child = root.join("child module");
        let grand = child.join("nested");
        identity(&child);
        identity(&grand);
        Self {
            base,
            root,
            child,
            grand,
        }
    }

    fn run(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_git-sync-submodule-refs"))
            .current_dir(&self.root)
            .env("GIT_ALLOW_PROTOCOL", "file")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap()
    }

    fn commit(path: &Path) {
        fs::write(path.join("file.txt"), "updated\n").unwrap();
        git(path, &["add", "file.txt"]);
        git(path, &["commit", "-m", "update file"]);
    }

    fn unchanged_after_failure(&self, needle: &str) {
        let before: Vec<_> = [&self.root, &self.child, &self.grand]
            .iter()
            .map(|p| git(p, &["rev-parse", "HEAD"]))
            .collect();
        let remotes: Vec<_> = ["root.git", "child.git", "grand.git"]
            .iter()
            .map(|p| git(&self.base.join(p), &["rev-parse", "main"]))
            .collect();
        let output = self.run();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "Unexpected success");
        assert!(stderr.contains(needle), "Expected {needle:?}: {stderr}");
        for (i, path) in [&self.root, &self.child, &self.grand].iter().enumerate() {
            assert_eq!(before[i], git(path, &["rev-parse", "HEAD"]));
        }
        for (i, name) in ["root.git", "child.git", "grand.git"].iter().enumerate() {
            assert_eq!(
                remotes[i],
                git(&self.base.join(name), &["rev-parse", "main"])
            );
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.base).unwrap();
    }
}

#[test]
fn clean_tree_is_a_noop() {
    let f = Fixture::new();
    let before = git(&f.root, &["rev-parse", "HEAD"]);
    let output = f.run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(before, git(&f.root, &["rev-parse", "HEAD"]));
}

#[test]
fn recursively_commits_and_pushes_inside_out() {
    let f = Fixture::new();
    git(&f.grand, &["checkout", "main"]);
    Fixture::commit(&f.grand);
    let output = f.run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for (path, remote) in [
        (&f.root, "root.git"),
        (&f.child, "child.git"),
        (&f.grand, "grand.git"),
    ] {
        assert_eq!(
            git(path, &["rev-parse", "HEAD"]),
            git(&f.base.join(remote), &["rev-parse", "main"])
        );
        assert!(git(path, &["status", "--porcelain"]).is_empty());
    }
    assert!(
        git(&f.child, &["log", "-1", "--format=%B"])
            .starts_with("[Automatic] Update submodule nested: ")
    );
    assert!(
        git(&f.root, &["log", "-1", "--format=%B"])
            .starts_with("[Automatic] Update submodule child module: ")
    );
}

#[test]
fn dirty_nested_file_blocks_all_commits_and_pushes() {
    let f = Fixture::new();
    Fixture::commit(&f.child);
    fs::write(f.grand.join("file.txt"), "uncommitted").unwrap();
    f.unchanged_after_failure("uncommitted work");
}

#[test]
fn staged_root_file_blocks_everything() {
    let f = Fixture::new();
    Fixture::commit(&f.child);
    fs::write(f.root.join("file.txt"), "staged").unwrap();
    git(&f.root, &["add", "file.txt"]);
    f.unchanged_after_failure("uncommitted work");
}

#[test]
fn untracked_nested_file_blocks_everything() {
    let f = Fixture::new();
    fs::write(f.grand.join("not tracked"), "new").unwrap();
    f.unchanged_after_failure("not tracked");
}

#[test]
fn ignored_files_are_allowed() {
    let f = Fixture::new();
    let exclude = git(&f.grand, &["rev-parse", "--git-path", "info/exclude"]);
    fs::write(f.grand.join(exclude), "ignored\n").unwrap();
    fs::write(f.grand.join("ignored"), "local").unwrap();
    let output = f.run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn unpublished_detached_head_is_rejected() {
    let f = Fixture::new();
    Fixture::commit(&f.grand);
    f.unchanged_after_failure("detached HEAD");
}

#[test]
fn detached_parent_needing_commit_is_rejected() {
    let f = Fixture::new();
    git(&f.child, &["checkout", "--detach"]);
    git(&f.grand, &["checkout", "main"]);
    Fixture::commit(&f.grand);
    f.unchanged_after_failure("cannot commit reference changes on a detached HEAD");
}

#[test]
fn behind_branch_is_rejected_before_pushing_anything() {
    let f = Fixture::new();
    let source = f.base.join("child-source");
    Fixture::commit(&source);
    git(&source, &["push"]);
    f.unchanged_after_failure("behind or diverged");
}

#[test]
fn uninitialized_submodule_is_rejected() {
    let f = Fixture::new();
    git(&f.root, &["submodule", "deinit", "child module"]);
    let before = git(&f.root, &["rev-parse", "HEAD"]);
    let output = f.run();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not initialized"));
    assert_eq!(before, git(&f.root, &["rev-parse", "HEAD"]));
}

#[test]
fn staged_submodule_reference_is_allowed() {
    let f = Fixture::new();
    Fixture::commit(&f.child);
    git(&f.root, &["add", "child module"]);
    let output = f.run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(git(&f.root, &["status", "--porcelain"]).is_empty());
}

#[test]
fn unfinished_operation_is_rejected() {
    let f = Fixture::new();
    let state = git(&f.grand, &["rev-parse", "--git-path", "CHERRY_PICK_HEAD"]);
    fs::write(
        f.grand.join(state),
        format!("{}\n", git(&f.grand, &["rev-parse", "HEAD"])),
    )
    .unwrap();
    f.unchanged_after_failure("unfinished or active Git operation");
}

#[test]
fn reports_dirty_files_in_multiple_repositories() {
    let f = Fixture::new();
    fs::write(f.root.join("root-untracked"), "new").unwrap();
    fs::write(f.grand.join("grand-untracked"), "new").unwrap();
    let output = f.run();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("root-untracked"));
    assert!(stderr.contains("grand-untracked"));
}

#[test]
fn missing_upstream_blocks_everything() {
    let f = Fixture::new();
    Fixture::commit(&f.child);
    git(&f.root, &["branch", "--unset-upstream"]);
    f.unchanged_after_failure("branch.main.remote");
}

#[cfg(unix)]
#[test]
fn rejected_child_push_does_not_commit_parent_reference() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    Fixture::commit(&f.child);
    let hook = f.base.join("child.git/hooks/pre-receive");
    fs::write(&hook, "#!/bin/sh\necho 'test rejection' >&2\nexit 1\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    f.unchanged_after_failure("test rejection");
}
