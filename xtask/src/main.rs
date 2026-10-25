use anyhow::{Context, Result, bail, ensure};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};
use toml_edit::{DocumentMut, value};

const NAME: &str = "git-sync-submodule-refs";
const USAGE: &str = "Usage: cargo xtask deploy <patch|minor|major>";

fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    env::set_current_dir(root)?;
    let args: Vec<_> = env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["deploy", bump @ ("patch" | "minor" | "major")] => deploy(root, bump),
        ["help" | "--help" | "-h"] => {
            println!("{USAGE}");
            Ok(())
        }
        _ => bail!("{USAGE}"),
    }
}

fn run(program: &str, args: &[&str]) -> Result<()> {
    println!("+ {program} {}", args.join(" "));
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("Could not run {program}; is it installed and on PATH?"))?;
    ensure!(status.success(), "{program} failed: {status}");
    Ok(())
}

fn output(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program).args(args).output()?;
    ensure!(
        output.status.success(),
        "{program} {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn manifest() -> Result<DocumentMut> {
    fs::read_to_string("Cargo.toml")?
        .parse()
        .context("Invalid Cargo.toml")
}

fn package_version(manifest: &DocumentMut) -> Result<&str> {
    manifest["package"]["version"]
        .as_str()
        .context("Missing package.version")
}

fn next_version(current: &str, bump: &str) -> Result<String> {
    let parts = current
        .split('.')
        .map(str::parse::<u64>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let [major, minor, patch] = parts.as_slice() else {
        bail!("Expected a stable major.minor.patch version, got {current}");
    };
    let increment = |n: u64| n.checked_add(1).context("Version component overflow");
    Ok(match bump {
        "major" => format!("{}.0.0", increment(*major)?),
        "minor" => format!("{major}.{}.0", increment(*minor)?),
        "patch" => format!("{major}.{minor}.{}", increment(*patch)?),
        _ => bail!("Unknown version bump: {bump}"),
    })
}

fn clean_tree() -> Result<()> {
    ensure!(
        output("git", &["status", "--porcelain", "--untracked-files=all"])?.is_empty(),
        "Working tree must be clean. Commit changes (including regenerated Lean files) first."
    );
    Ok(())
}

fn github_repository(url: &str) -> Result<&str> {
    let repository = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("git@github.com:"))
        .context("Expected an HTTPS or SSH github.com repository URL")?;
    Ok(repository.trim_end_matches('/').trim_end_matches(".git"))
}

fn deploy(root: &Path, bump: &str) -> Result<()> {
    clean_tree()?;
    let mut manifest = manifest()?;
    let version = next_version(package_version(&manifest)?, bump)?;
    let tag = format!("v{version}");
    let repository = github_repository(
        manifest["package"]["repository"]
            .as_str()
            .context("Missing package.repository")?,
    )?
    .to_owned();
    let origin = output("git", &["remote", "get-url", "--push", "origin"])?;
    ensure!(
        github_repository(&origin)? == repository,
        "origin push URL must match package.repository"
    );
    ensure!(
        !output("git", &["branch", "--show-current"])?.is_empty(),
        "Deploy requires a checked-out branch, not detached HEAD"
    );
    ensure!(
        output("git", &["tag", "--list", &tag])?.is_empty(),
        "Tag {tag} already exists locally"
    );
    ensure!(
        output(
            "git",
            &["ls-remote", "--tags", "origin", &format!("refs/tags/{tag}")]
        )?
        .is_empty(),
        "Tag {tag} already exists on origin"
    );
    run("gh", &["auth", "status"])?;
    run(
        "gh",
        &["repo", "view", &repository, "--json", "nameWithOwner"],
    )?;
    // Fail before lengthy checks if any verification tool is missing.
    for (program, args) in [
        ("make", vec!["--version"]),
        ("aeneas", vec!["-version"]),
        ("charon", vec!["version"]),
        ("lake", vec!["--version"]),
    ] {
        run(program, &args)?;
    }
    println!("Preparing {tag}; all checks must pass before publishing.");
    run("cargo", &["fmt", "--all", "--", "--check"])?;
    run(
        "cargo",
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--locked",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    run("cargo", &["test", "--workspace", "--locked"])?;
    run("make", &["verify-lean"])?;
    clean_tree()?;

    manifest["package"]["version"] = value(&version);
    fs::write("Cargo.toml", manifest.to_string())?;
    // Update only workspace versions; do not upgrade registry dependencies.
    run("cargo", &["update", "--workspace", "--offline"])?;
    let asset = build_rpm(root).context(
        "RPM build failed before publishing. The version bump is left uncommitted; fix the build and review Cargo.toml/Cargo.lock before retrying.",
    )?;
    let asset = asset.to_str().context("RPM path is not UTF-8")?;
    run("git", &["add", "--", "Cargo.toml", "Cargo.lock"])?;
    run("git", &["commit", "-m", &format!("Release {tag}")])?;
    clean_tree()?;
    run("git", &["tag", "-a", &tag, "-m", &format!("Release {tag}")])?;
    run(
        "git",
        &[
            "push",
            "--atomic",
            "origin",
            "HEAD",
            &format!("refs/tags/{tag}"),
        ],
    )
    .with_context(|| format!("Release commit and tag exist locally. Retry the push with: git push --atomic origin HEAD refs/tags/{tag}. Then create the release with: gh release create {tag} '{asset}' --repo {repository} --verify-tag --title {tag} --generate-notes"))?;
    run("gh", &["release", "create", &tag, asset, "--repo", &repository, "--verify-tag", "--title", &tag, "--generate-notes"])
        .with_context(|| format!("Tag was pushed. Check gh release view {tag} --repo {repository}. If no release exists, retry: gh release create {tag} '{asset}' --repo {repository} --verify-tag --title {tag} --generate-notes. If it exists, upload with: gh release upload {tag} '{asset}' --repo {repository} --clobber"))?;
    println!("Published {tag} with its RPM attached.");
    Ok(())
}

fn build_rpm(root: &Path) -> Result<PathBuf> {
    ensure!(env::consts::OS == "linux", "RPM builds require Linux");
    let (target, arch) = match env::consts::ARCH {
        "x86_64" => ("x86_64-unknown-linux-musl", "x86_64"),
        "aarch64" => ("aarch64-unknown-linux-musl", "aarch64"),
        arch => bail!("Unsupported RPM architecture: {arch}"),
    };
    // Keep build and packaging outputs together even with a custom Cargo target directory.
    let cargo_target = root.join("target");
    run(
        "cargo",
        &[
            "build",
            "--release",
            "--locked",
            "--package",
            NAME,
            "--target",
            target,
            "--target-dir",
            cargo_target
                .to_str()
                .context("Target directory is not UTF-8")?,
        ],
    )?;

    let output_dir = cargo_target.join("rpm");
    fs::create_dir_all(&output_dir)?;
    let asset = output_dir.join(format!("{NAME}.{arch}.rpm"));
    package_rpm(
        &manifest()?,
        &cargo_target.join(target).join("release").join(NAME),
        &root.join("LICENSE"),
        arch,
        &asset,
    )?;
    println!("RPM created: {}", asset.display());
    Ok(asset)
}

fn package_rpm(
    manifest: &DocumentMut,
    binary: &Path,
    license: &Path,
    arch: &str,
    destination: &Path,
) -> Result<()> {
    let field = |name: &str| {
        manifest["package"][name]
            .as_str()
            .with_context(|| format!("Missing package.{name}"))
    };
    let mut builder = rpm::PackageBuilder::new(
        field("name")?,
        package_version(manifest)?,
        field("license")?,
        arch,
        field("description")?,
    );
    builder
        .using_config(rpm::BuildConfig::v4().compression(rpm::CompressionType::Gzip))
        .release("1")
        .url(field("repository")?)
        // Static musl binaries need no shared-library dependencies; Git runs externally.
        .requires(rpm::Dependency::any("git"))
        .with_file(
            binary,
            rpm::FileOptions::new(format!("/usr/bin/{NAME}")).permissions(0o755),
        )?
        .with_file(
            license,
            rpm::FileOptions::new(format!("/usr/share/licenses/{NAME}/LICENSE"))
                .permissions(0o644)
                .license(),
        )?;
    builder.build()?.write_file(destination)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bumps_increment_and_reset_lower_components() {
        assert_eq!(next_version("1.2.3", "patch").unwrap(), "1.2.4");
        assert_eq!(next_version("1.2.3", "minor").unwrap(), "1.3.0");
        assert_eq!(next_version("1.2.3", "major").unwrap(), "2.0.0");
        assert_eq!(next_version("0.1.0", "major").unwrap(), "1.0.0");
    }

    #[test]
    fn invalid_versions_and_bumps_fail() {
        for version in [
            "1.2",
            "1.2.3.4",
            "1.2.3-rc.1",
            "one.2.3",
            "1.2.18446744073709551615",
        ] {
            assert!(next_version(version, "patch").is_err());
        }
        assert!(next_version("1.2.3", "breaking").is_err());
    }

    #[test]
    fn github_urls_normalize() {
        assert_eq!(
            github_repository("git@github.com:owner/repo.git").unwrap(),
            "owner/repo"
        );
        assert_eq!(
            github_repository("https://github.com/owner/repo").unwrap(),
            "owner/repo"
        );
        assert!(github_repository("https://example.com/owner/repo").is_err());
    }

    #[test]
    fn version_edit_preserves_other_manifest_fields() {
        let source = "[package]\nname = \"example\"\nversion = \"0.1.0\"\n\n# Keep dependencies unchanged\n[dependencies]\nanyhow = \"1\"\n";
        let mut manifest: DocumentMut = source.parse().unwrap();
        let version = next_version(package_version(&manifest).unwrap(), "minor").unwrap();
        manifest["package"]["version"] = value(version);
        assert_eq!(
            manifest.to_string(),
            source.replace("version = \"0.1.0\"", "version = \"0.2.0\"")
        );
    }

    #[test]
    fn rpm_contains_binary_license_and_git_dependency() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let binary = directory.path().join(NAME);
        let license = directory.path().join("LICENSE");
        let destination = directory.path().join("package.rpm");
        fs::write(&binary, b"test binary")?;
        fs::write(&license, b"MIT license")?;
        let manifest = include_str!("../../Cargo.toml").parse::<DocumentMut>()?;
        package_rpm(&manifest, &binary, &license, "x86_64", &destination)?;

        let package = rpm::Package::open(destination)?;
        package.verify_digests()?;
        assert_eq!(package.metadata.get_name()?, NAME);
        assert_eq!(package.metadata.get_version()?, package_version(&manifest)?);
        assert_eq!(package.metadata.get_arch()?, "x86_64");
        assert_eq!(package.metadata.get_license()?, "MIT");
        assert!(
            package
                .metadata
                .get_requires()?
                .iter()
                .any(|dep| dep.name == "git")
        );
        let entries = package.metadata.get_file_entries()?;
        assert_eq!(entries.len(), 2);
        for (path, permissions) in [
            (format!("/usr/bin/{NAME}"), 0o755),
            (format!("/usr/share/licenses/{NAME}/LICENSE"), 0o644),
        ] {
            let entry = entries
                .iter()
                .find(|entry| entry.path == Path::new(&path))
                .unwrap();
            assert_eq!(entry.mode.permissions(), permissions);
        }
        let files = package
            .files()?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        assert!(files.iter().any(|file| file.content == b"test binary"));
        assert!(files.iter().any(|file| file.content == b"MIT license"));
        Ok(())
    }
}
