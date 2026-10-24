use anyhow::{Context, Result, bail};
use git_spawn::{CommandOutput, command::CommandExecutor};
use std::{ffi::OsString, path::Path};

/// Keep exact argument order and bytes; do not use broad workflow shortcuts.
pub async fn command(repo: &Path, args: &[&str]) -> Result<CommandOutput> {
    let executor = CommandExecutor::new()
        .cwd(repo)
        .with_env("GIT_TERMINAL_PROMPT", "0");
    executor
        .execute_command_os_unchecked(args.iter().map(OsString::from).collect())
        .await
        .with_context(|| format!("{}: cannot execute git {}", repo.display(), args.join(" ")))
}

pub fn check(repo: &Path, args: &[&str], output: CommandOutput) -> Result<Vec<u8>> {
    if !output.success {
        bail!(
            "{}: git {} failed ({:?}):\n{}",
            repo.display(),
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

pub async fn bytes(repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
    check(repo, args, command(repo, args).await?)
}

pub async fn git(repo: &Path, args: &[&str]) -> Result<String> {
    String::from_utf8(bytes(repo, args).await?)
        .with_context(|| format!("{}: non-UTF-8 Git output is not supported", repo.display()))
}

/// Exit 1 is a documented negative predicate, not a generic command failure.
pub async fn predicate(repo: &Path, args: &[&str]) -> Result<bool> {
    let output = command(repo, args).await?;
    match output.exit_code {
        0 => Ok(true),
        1 => Ok(false),
        _ => {
            check(repo, args, output)?;
            unreachable!("unexpected exit status passed the success check")
        }
    }
}

pub async fn head(repo: &Path) -> Result<String> {
    Ok(git(repo, &["rev-parse", "--verify", "HEAD"])
        .await?
        .trim()
        .into())
}
