use anyhow::{Context, Result, ensure};
use bstr::ByteSlice;
use git_spawn::parse::{StatusEntry, parse_status};
use std::collections::BTreeMap;

/// Parse Git's NUL-delimited tree/index records, preserving path whitespace.
pub fn gitlinks(output: &[u8], index: bool) -> Result<BTreeMap<String, String>> {
    ensure!(
        output.is_empty() || output.ends_with(b"\0"),
        "Git records must be NUL-terminated"
    );
    let mut entries = BTreeMap::new();
    for record in output.split_str(b"\0").filter(|r| !r.is_empty()) {
        let (metadata, path) = record
            .split_once_str(b"\t")
            .context("Git record is missing its path separator")?;
        let path = path
            .to_str()
            .context("Non-UTF-8 Git paths are not supported")?;
        ensure!(!path.is_empty(), "Git record has an empty path");
        let fields: Vec<_> = metadata.to_str()?.split(' ').collect();
        ensure!(fields.len() == 3, "Malformed Git record for {path:?}");
        if index {
            ensure!(fields[2] == "0", "unresolved conflict at {path:?}");
        }
        if fields[0] == "160000" {
            let hash = fields[if index { 1 } else { 2 }];
            ensure!(
                (hash.len() == 40 || hash.len() == 64)
                    && hash.bytes().all(|b| b.is_ascii_hexdigit()),
                "Invalid object ID for {path:?}"
            );
            ensure!(
                entries.insert(path.to_owned(), hash.to_owned()).is_none(),
                "Duplicate submodule path {path:?}"
            );
        }
    }
    Ok(entries)
}

pub fn status(output: &[u8]) -> Result<Vec<StatusEntry>> {
    ensure!(
        output.is_empty() || output.ends_with(b"\0"),
        "Status records must be NUL-terminated"
    );
    // Never use stdout_str(): its lossy conversion could change file names.
    let text = output
        .to_str()
        .context("Non-UTF-8 Git paths are not supported")?;
    let entries = parse_status(text).context("Cannot parse Git porcelain-v1 status")?;
    for entry in &entries {
        ensure!(!entry.path.is_empty(), "Status record has an empty path");
        use git_spawn::parse::StatusKind;
        if matches!(entry.index, StatusKind::Renamed | StatusKind::Copied) {
            ensure!(
                entry.original_path.as_ref().is_some_and(|p| !p.is_empty()),
                "Rename/copy record is missing its original path"
            );
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_unusual_paths_and_consumes_renames() {
        let entries = status(b"R  new name\0old\tname\0??  leading\ntrailing \0").unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "new name");
        assert_eq!(entries[0].original_path.as_deref(), Some("old\tname"));
        assert_eq!(entries[1].path, " leading\ntrailing ");
    }

    #[test]
    fn rejects_malformed_status_and_non_utf8_without_lossy_conversion() {
        for output in [
            b"MM file".as_slice(),
            b"R  file\0",
            b"MMfile\0",
            b"?? \xff\0",
            b"?? \0",
        ] {
            assert!(status(output).is_err(), "accepted {output:?}");
        }
    }

    #[test]
    fn parses_tree_and_index_gitlinks() {
        let hash = "a".repeat(40);
        for (record, index) in [
            (format!("160000 commit {hash}\t leading\tname \0"), false),
            (format!("160000 {hash} 0\t leading\tname \0"), true),
        ] {
            assert_eq!(
                gitlinks(record.as_bytes(), index).unwrap()[" leading\tname "],
                hash
            );
        }
    }

    #[test]
    fn rejects_conflicts_and_malformed_gitlinks() {
        let hash = "a".repeat(40);
        assert!(gitlinks(format!("160000 {hash} 2\tmodule\0").as_bytes(), true).is_err());
        for output in [
            b"160000\tmodule\0".as_slice(),
            b"160000 commit invalid\tmodule\0",
            b"160000 commit missing-path\0",
        ] {
            assert!(gitlinks(output, false).is_err());
        }
    }
}
