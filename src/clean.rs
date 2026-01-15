use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use walkdir::{DirEntry, WalkDir};

use crate::config::{Rule, RuleKind};
use crate::options::{DownloadsChoice, ScanOptions};

#[derive(Debug, Clone)]
pub struct RuleScan {
    pub rule: Rule,
    pub bytes: u64,
    pub entries: usize,
    pub files: Vec<PathBuf>,
    pub dirs: Vec<PathBuf>,
    pub errors: usize,
}

#[derive(Debug, Default)]
pub struct CleanReport {
    pub files_removed: usize,
    pub dirs_removed: usize,
    pub bytes_freed: u64,
    pub errors: usize,
}

#[derive(Debug, Default)]
pub struct DryRunReport {
    pub files_listed: usize,
    pub dirs_listed: usize,
    pub bytes_listed: u64,
    pub errors: usize,
}

#[derive(Debug)]
pub struct DryRunOutput {
    pub report: DryRunReport,
    pub details: String,
}

const ARCHIVE_EXTENSIONS: [&str; 7] = [
    ".tar.gz", ".tgz", ".tar.xz", ".tar.zst", ".zip", ".7z", ".rar",
];

pub fn scan_rules(rules: &[Rule], options: &ScanOptions) -> Vec<RuleScan> {
    rules.iter().map(|rule| scan_rule(rule, options)).collect()
}

pub fn apply(scans: &[RuleScan]) -> CleanReport {
    let mut report = CleanReport::default();

    for scan in scans {
        for path in &scan.files {
            match fs::symlink_metadata(path) {
                Ok(meta) => {
                    let size = meta.len();
                    if fs::remove_file(path).is_ok() {
                        report.bytes_freed += size;
                        report.files_removed += 1;
                    } else {
                        report.errors += 1;
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    continue;
                }
                Err(_) => {
                    report.errors += 1;
                }
            }
        }

        let mut dirs = scan.dirs.clone();
        dirs.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
        for dir in dirs {
            match fs::remove_dir(&dir) {
                Ok(_) => report.dirs_removed += 1,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    continue;
                }
                Err(_) => {
                    report.errors += 1;
                }
            }
        }
    }

    report
}

pub fn dry_run_output(scans: &[RuleScan]) -> DryRunOutput {
    let mut report = DryRunReport::default();
    let mut details = String::new();

    let _ = writeln!(details, "Dry-run details (no files will be deleted):");
    let _ = writeln!(
        details,
        "Note: directories are only removed if empty after file removal."
    );
    for scan in scans {
        let _ = writeln!(details, "Rule: {} ({})", scan.rule.label, scan.rule.id);
        if scan.files.is_empty() && scan.dirs.is_empty() {
            let _ = writeln!(details, "  (no entries)");
        } else {
            for path in &scan.files {
                let _ = writeln!(details, "  file: {}", path.display());
            }
            for path in &scan.dirs {
                let _ = writeln!(details, "  dir: {}", path.display());
            }
        }
        report.files_listed += scan.files.len();
        report.dirs_listed += scan.dirs.len();
        report.bytes_listed += scan.bytes;
        report.errors += scan.errors;
    }

    DryRunOutput { report, details }
}

pub fn write_dry_run_report(home: &Path, details: &str) -> Result<PathBuf> {
    let mut path = home.to_path_buf();
    path.push("vole-dry-run.txt");
    std::fs::write(&path, details)
        .with_context(|| format!("could not write {}", path.display()))?;
    Ok(path)
}

pub fn scan_rule(rule: &Rule, options: &ScanOptions) -> RuleScan {
    match rule.kind {
        RuleKind::Paths => scan_paths_rule(rule),
        RuleKind::Downloads => scan_downloads_rule(rule, options.downloads_choice),
    }
}

fn scan_paths_rule(rule: &Rule) -> RuleScan {
    let mut scan = RuleScan {
        rule: rule.clone(),
        bytes: 0,
        entries: 0,
        files: Vec::new(),
        dirs: Vec::new(),
        errors: 0,
    };

    let (exclude_set, exclude_errors) = build_globset(&rule.exclude_globs);
    scan.errors += exclude_errors;

    for root in rule.expanded_paths() {
        if !root.exists() {
            continue;
        }
        if let Err(errors) = scan_root(&root, exclude_set.as_ref(), &mut scan) {
            scan.errors += errors;
        }
    }

    scan
}

fn scan_downloads_rule(rule: &Rule, choice: Option<DownloadsChoice>) -> RuleScan {
    let mut scan = RuleScan {
        rule: rule.clone(),
        bytes: 0,
        entries: 0,
        files: Vec::new(),
        dirs: Vec::new(),
        errors: 0,
    };

    let Some(choice) = choice else {
        return scan;
    };

    for root in rule.expanded_paths() {
        let meta = match fs::symlink_metadata(&root) {
            Ok(meta) => meta,
            Err(_) => {
                scan.errors += 1;
                continue;
            }
        };
        if meta.file_type().is_symlink() || !meta.is_dir() {
            continue;
        }

        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(_) => {
                scan.errors += 1;
                continue;
            }
        };

        let mut archives: Vec<(String, PathBuf, u64)> = Vec::new();
        let mut folders: HashMap<String, PathBuf> = HashMap::new();

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    scan.errors += 1;
                    continue;
                }
            };
            let path = entry.path();
            let name = match entry.file_name().to_str() {
                Some(name) => name.to_string(),
                None => {
                    scan.errors += 1;
                    continue;
                }
            };
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => {
                    scan.errors += 1;
                    continue;
                }
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                folders.insert(name, path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let Some(base) = archive_base_name(&name) else {
                continue;
            };
            let size = match entry.metadata() {
                Ok(meta) => meta.len(),
                Err(_) => {
                    scan.errors += 1;
                    continue;
                }
            };
            archives.push((base, path, size));
        }

        let mut seen_dirs: HashSet<PathBuf> = HashSet::new();

        for (base, archive_path, size) in archives {
            let Some(dir_path) = folders.get(&base) else {
                continue;
            };
            match choice {
                DownloadsChoice::Archives => {
                    scan.entries += 1;
                    scan.bytes += size;
                    scan.files.push(archive_path);
                }
                DownloadsChoice::Folders => {
                    if seen_dirs.insert(dir_path.clone()) {
                        if let Err(errors) = scan_root(dir_path, None, &mut scan) {
                            scan.errors += errors;
                        }
                        scan.dirs.push(dir_path.clone());
                    }
                }
            }
        }
    }

    scan
}

fn archive_base_name(file_name: &str) -> Option<String> {
    let lower = file_name.to_ascii_lowercase();
    for ext in ARCHIVE_EXTENSIONS {
        if lower.ends_with(ext) {
            let base_len = file_name.len().saturating_sub(ext.len());
            if base_len == 0 {
                return None;
            }
            return Some(file_name[..base_len].to_string());
        }
    }
    None
}

fn scan_root(root: &Path, exclude: Option<&GlobSet>, scan: &mut RuleScan) -> Result<(), usize> {
    if root.is_file() || root.is_symlink() {
        if !is_excluded(root, root, exclude) {
            if let Ok(meta) = fs::symlink_metadata(root) {
                scan.bytes += meta.len();
            }
            scan.entries += 1;
            scan.files.push(root.to_path_buf());
        }
        return Ok(());
    }

    let mut errors = 0;
    let mut iter = WalkDir::new(root)
        .follow_links(false)
        .same_file_system(true)
        .into_iter()
        .filter_entry(|entry| filter_entry(entry, root, exclude));

    while let Some(next) = iter.next() {
        match next {
            Ok(entry) => {
                if entry.path() == root {
                    continue;
                }
                if is_excluded(entry.path(), root, exclude) {
                    continue;
                }
                if entry.file_type().is_dir() {
                    scan.dirs.push(entry.path().to_path_buf());
                } else {
                    if let Ok(meta) = entry.metadata() {
                        scan.bytes += meta.len();
                    }
                    scan.entries += 1;
                    scan.files.push(entry.path().to_path_buf());
                }
            }
            Err(_) => {
                errors += 1;
            }
        }
    }

    if errors == 0 {
        Ok(())
    } else {
        Err(errors)
    }
}

fn filter_entry(entry: &DirEntry, root: &Path, exclude: Option<&GlobSet>) -> bool {
    if entry.path() == root {
        return true;
    }
    !is_excluded(entry.path(), root, exclude)
}

fn is_excluded(path: &Path, root: &Path, exclude: Option<&GlobSet>) -> bool {
    let Some(exclude) = exclude else {
        return false;
    };
    let rel = path.strip_prefix(root).unwrap_or(path);
    exclude.is_match(rel)
}

fn build_globset(patterns: &[String]) -> (Option<GlobSet>, usize) {
    if patterns.is_empty() {
        return (None, 0);
    }

    let mut builder = GlobSetBuilder::new();
    let mut errors = 0;

    for pattern in patterns {
        if let Ok(glob) = Glob::new(pattern) {
            builder.add(glob);
        } else {
            errors += 1;
        }
    }

    match builder.build() {
        Ok(set) => (Some(set), errors),
        Err(_) => (None, errors + 1),
    }
}
