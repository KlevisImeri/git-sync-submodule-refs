# git-sync-submodule-refs

Recursively commit and push submodule reference updates, only when no other uncommitted changes exist anywhere in the repository tree.

## Install

### RPM (Fedora and other DNF-based distributions)

Once a release has been published, install the latest x86_64 RPM directly:

```sh
sudo dnf install https://github.com/KlevisImeri/git-sync-submodule-refs/releases/latest/download/git-sync-submodule-refs.x86_64.rpm
```

The RPM installs `/usr/bin/git-sync-submodule-refs` and requires Git; Rust/Cargo is not needed to use it. Locally built aarch64 releases use the filename `git-sync-submodule-refs.aarch64.rpm` instead.

### From source

Requires Rust/Cargo to build, and Git on your `PATH` at runtime. Git operations run through `git-spawn`, using your installed Git rather than libgit2.

```sh
cargo install --git https://github.com/KlevisImeri/git-sync-submodule-refs.git --locked
```

Ensure `~/.cargo/bin` is on your `PATH`. Git discovers the installed executable as a custom command:

```sh
git sync-submodule-refs
```

Run it anywhere inside the parent repository. It always includes nested submodules and pushes; no flags are needed. It never walks upward into parent repositories outside the current repository.

## Behavior

1. Validate the root and every nested submodule before committing or pushing anything. Only changes to existing submodule commit references are allowed. Staged/unstaged file edits, untracked non-ignored files, conflicts, uninitialized submodules, and unfinished Git operations cause failure, with repository paths and reasons.
2. Fetch remote references without pulling, merging, rebasing, or advancing working checkouts. Check publishing prerequisites across the entire tree. Branches must have a remote upstream and cannot be behind or diverged. Detached checkouts are accepted only when already published and no reference commit is needed there.
3. Process children before parents. Push existing branch commits, record changed child references in each parent, and push the parent. Never commit ordinary file changes or force-push.

Automatic commit messages use:

```text
[Automatic] Update submodule <path>: <old-hash> -> <new-hash>
```

Multiple updates in one repository are recorded in one commit, with one line per submodule. The currently checked-out child commits are used: this command does **not** update dependencies to their latest versions. Added/deleted submodules and deliberately staged references different from the checkout require manual handling.

## Safety and limitations

- If preflight fails, no commits or pushes are made anywhere. Fetching may update local remote-tracking references.
- Repositories with no reference changes can still have existing committed work pushed. Review your commits before running this command.
- Configured Git hooks still run. Do not edit repositories concurrently while the command runs. Working-tree checks are repeated, but this is not a transaction or a lock across the tree.
- Pushes across repositories are not atomic. If an operation fails after preflight, earlier commits/pushes remain; inspect the reported state and retry after fixing the problem. No destructive rollback is attempted.
- Preflight cannot guarantee remote write permission or prevent network failures. Normal Git authentication must already work; interactive terminal credential prompts are disabled.
- Remote-tracking refs are used as evidence that detached commits are published. Their availability to other users still depends on remote access permissions.
- Non-UTF-8 paths/output are rejected rather than interpreted incorrectly.

## Development

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo install --path . --locked
```

Integration tests use isolated local repositories and bare remotes, not your working projects.

The implementation uses `git-spawn` for command execution and porcelain status parsing, Tokio for its async runtime, `anyhow` for contextual errors, and `bstr` for byte-oriented tree/index parsing. Tests use `tempfile` for temporary repositories and `assert_cmd` for CLI assertions and timeouts. Recursive validation and push ordering remain explicit in this tool; no broad commit-all or sync workflow helpers are used.

## Releases

Release creation runs entirely on your Linux machine, with no GitHub Actions workflow:

```sh
cargo xtask deploy patch  # 0.1.0 -> 0.1.1: fixes
cargo xtask deploy minor  # 0.1.0 -> 0.2.0: features
cargo xtask deploy major  # 0.1.0 -> 1.0.0: breaking changes
```

Run just one of these commands. Each increments the chosen component and resets smaller components to zero.

Prerequisites:

- A clean, committed working tree on a branch; `origin` must point to this GitHub repository. Deploy pushes the current branch and its new tag to `origin` without force.
- Rust with rustfmt, Clippy, and the native musl target (`rustup target add x86_64-unknown-linux-musl`, or `aarch64-unknown-linux-musl` on ARM).
- A native musl C toolchain (Fedora: `sudo dnf install musl-gcc`; Ubuntu: `sudo apt-get install musl-tools`). The `rpm` Rust library is a dependency of `xtask`, tracked in `Cargo.lock`; no packaging tool installation, `rpmbuild`, or RPM spec file is needed.
- GitHub CLI (`gh`), authenticated with permission to push and create releases (`gh auth login`), plus working Git push authentication.
- Make and the pinned Aeneas, Charon, and Lean tools described in [verification/README.md](verification/README.md).

Deploy runs `cargo fmt --all -- --check`, workspace Clippy with warnings denied, all workspace Rust tests (including doctests), and `make verify-lean`. If verification regenerates tracked files, commit those changes and rerun deploy. After checks pass, it bumps `Cargo.toml`/`Cargo.lock`, builds the binary, packages it directly with the `rpm` Rust library using package metadata from `Cargo.toml`, commits the version bump, creates an annotated `v<version>` tag, pushes the branch and tag atomically, and creates a GitHub Release with the RPM attached. No release or push occurs before checks and RPM building succeed.

Deploy produces a statically linked musl binary and an unsigned RPM at `target/rpm/git-sync-submodule-refs.<architecture>.rpm`, containing the binary and MIT license, with `git` as a runtime dependency. Builds support native Linux x86_64 and aarch64. Signing RPMs or maintaining a DNF repository is not included.

If a build fails after the version bump, the uncommitted version changes remain for review; restore only those changes before retrying deploy. If publishing fails after the push, the command prints recovery commands; do not bump the version again to retry uploading the same release.

## Formal verification

The CLI's actual workflow controller and pure safety functions are translated from Rust through Charon and Aeneas into Lean. Proofs cover classification, preflight before mutations, child-first processing of checked dependencies, failure stopping, and publication coverage on successful completion. Git-facing wrappers and the thin async dispatcher have explicit trusted contracts; Git itself and our parser/adapter are not proved. Run `make verify-lean` to regenerate and check the proofs. See [verification/README.md](verification/README.md) for the guarantees, assumptions, and pinned tools. Integration tests remain in place.

## License

MIT — Copyright (c) 2026 Klevis Imeri. Retain the copyright and license notice when distributing copies or substantial portions. Provided as is, without warranty. See [LICENSE](LICENSE).
