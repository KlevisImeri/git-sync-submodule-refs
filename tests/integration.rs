use assert_cmd::assert::OutputAssertExt;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::Duration,
};
use tempfile::TempDir;

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
    _temp: TempDir,
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
        let temp = tempfile::Builder::new()
            .prefix("git-sync-refs-test-")
            .tempdir_in(temp)
            .unwrap();
        let base = temp.path().to_owned();
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
            _temp: temp,
            base,
            root,
            child,
            grand,
        }
    }

    fn run(&self) -> Output {
        assert_cmd::Command::new(env!("CARGO_BIN_EXE_git-sync-submodule-refs"))
            .timeout(Duration::from_secs(30))
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
        let assertion = self.run().assert().failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
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

#[test]
fn clean_tree_is_a_noop() {
    let f = Fixture::new();
    let before = git(&f.root, &["rev-parse", "HEAD"]);
    f.run().assert().success();
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
fn staged_reference_different_from_checkout_blocks_everything() {
    let f = Fixture::new();
    let original = git(&f.grand, &["rev-parse", "HEAD"]);
    Fixture::commit(&f.grand);
    git(&f.child, &["add", "--", "nested"]);
    git(&f.grand, &["checkout", "--detach", &original]);
    let index = git(&f.child, &["ls-files", "--stage", "nested"]);
    f.unchanged_after_failure("staged submodule reference differs from its checkout");
    assert_eq!(index, git(&f.child, &["ls-files", "--stage", "nested"]));
}

#[test]
fn dirty_post_commit_hook_stops_before_push_or_parent_commit() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    git(&f.grand, &["checkout", "main"]);
    Fixture::commit(&f.grand);
    let root_before = git(&f.root, &["rev-parse", "HEAD"]);
    let remote_before = git(&f.base.join("child.git"), &["rev-parse", "main"]);
    let child_before = git(&f.child, &["rev-parse", "HEAD"]);
    let hook = f.child.join(git(
        &f.child,
        &["rev-parse", "--git-path", "hooks/post-commit"],
    ));
    fs::write(&hook, "#!/bin/sh\nprintf 'dirty\\n' > file.txt\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    let output = f.run();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("uncommitted work"));
    assert_ne!(child_before, git(&f.child, &["rev-parse", "HEAD"]));
    assert_eq!(root_before, git(&f.root, &["rev-parse", "HEAD"]));
    assert_eq!(
        remote_before,
        git(&f.base.join("child.git"), &["rev-parse", "main"])
    );
}

#[test]
fn rejected_commit_stops_before_push_or_parent_processing() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    git(&f.grand, &["checkout", "main"]);
    Fixture::commit(&f.grand);
    let root_before = git(&f.root, &["rev-parse", "HEAD"]);
    let child_before = git(&f.child, &["rev-parse", "HEAD"]);
    let remote_before = git(&f.base.join("child.git"), &["rev-parse", "main"]);
    let hook = f.child.join(git(
        &f.child,
        &["rev-parse", "--git-path", "hooks/pre-commit"],
    ));
    fs::write(
        &hook,
        "#!/bin/sh\necho 'commit rejected by test' >&2\nexit 1\n",
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    let output = f.run();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("commit rejected by test"));
    assert_eq!(root_before, git(&f.root, &["rev-parse", "HEAD"]));
    assert_eq!(child_before, git(&f.child, &["rev-parse", "HEAD"]));
    assert_eq!(
        remote_before,
        git(&f.base.join("child.git"), &["rev-parse", "main"])
    );
    // Earlier successful operations are not rolled back.
    assert_eq!(
        git(&f.grand, &["rev-parse", "HEAD"]),
        git(&f.base.join("grand.git"), &["rev-parse", "main"])
    );
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

#[test]
fn pushes_existing_commits_without_reference_changes() {
    let f = Fixture::new();
    Fixture::commit(&f.root);
    let before = git(&f.root, &["rev-parse", "HEAD"]);
    f.run().assert().success();
    assert_eq!(before, git(&f.root, &["rev-parse", "HEAD"]));
    assert_eq!(
        before,
        git(&f.base.join("root.git"), &["rev-parse", "main"])
    );
}

#[test]
fn submodule_ignore_config_cannot_hide_nested_work() {
    let f = Fixture::new();
    git(&f.root, &["config", "submodule.child module.ignore", "all"]);
    git(&f.child, &["config", "submodule.nested.ignore", "all"]);
    git(&f.grand, &["config", "status.showUntrackedFiles", "no"]);
    fs::write(f.grand.join("untracked"), "new").unwrap();
    f.unchanged_after_failure("untracked");
}

#[test]
fn unusual_filename_is_reported_and_blocks_everything() {
    let f = Fixture::new();
    fs::write(f.grand.join(" leading\tname\ntrailing "), "new").unwrap();
    f.unchanged_after_failure("leading\\tname\\ntrailing");
}

#[test]
fn staged_rename_blocks_everything() {
    let f = Fixture::new();
    git(&f.grand, &["mv", "file.txt", "new name"]);
    f.unchanged_after_failure("new name");
}

#[cfg(unix)]
#[test]
fn staged_type_change_blocks_everything() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    fs::remove_file(f.grand.join("file.txt")).unwrap();
    symlink("target", f.grand.join("file.txt")).unwrap();
    git(&f.grand, &["add", "file.txt"]);
    f.unchanged_after_failure("file.txt");
}

#[cfg(unix)]
#[test]
fn non_utf8_path_is_rejected_without_lossy_conversion() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let f = Fixture::new();
    fs::write(
        f.grand.join(OsString::from_vec(b"invalid-\xff".to_vec())),
        "new",
    )
    .unwrap();
    f.unchanged_after_failure("Non-UTF-8 Git paths are not supported");
}
