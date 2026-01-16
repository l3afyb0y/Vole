use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

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
    pub error_messages: Vec<String>,
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
        } else if scan.rule.kind == RuleKind::Downloads {
            let summary_dirs = summarize_download_dirs(&scan.dirs);
            let mut suppressed = 0usize;
            if summary_dirs.is_empty() {
                for path in &scan.files {
                    let _ = writeln!(details, "  file: {}", path.display());
                }
                for path in &scan.dirs {
                    let _ = writeln!(details, "  dir: {}", path.display());
                }
            } else {
                for path in &scan.files {
                    if path_is_under_any(path, &summary_dirs) {
                        suppressed += 1;
                        continue;
                    }
                    let _ = writeln!(details, "  file: {}", path.display());
                }
                for path in &summary_dirs {
                    let _ = writeln!(details, "  dir: {}", path.display());
                }
                if suppressed > 0 {
                    let _ = writeln!(details, "  (contents omitted for Downloads folders)");
                }
            }
        } else {
            for path in &scan.files {
                let _ = writeln!(details, "  file: {}", path.display());
            }
            for path in &scan.dirs {
                let _ = writeln!(details, "  dir: {}", path.display());
            }
        }
        if !scan.error_messages.is_empty() {
            let _ = writeln!(details, "  errors: {}", scan.errors);
            for message in &scan.error_messages {
                let _ = writeln!(details, "  error: {}", message);
            }
        }
        report.files_listed += scan.files.len();
        report.dirs_listed += scan.dirs.len();
        report.bytes_listed += scan.bytes;
        report.errors += scan.errors;
    }

    DryRunOutput { report, details }
}

pub fn dry_run_report_path(home: &Path) -> PathBuf {
    let mut path = home.to_path_buf();
    path.push("vole-dry-run.txt");
    path
}

pub fn write_dry_run_report(home: &Path, details: &str) -> Result<PathBuf> {
    let path = dry_run_report_path(home);
    std::fs::write(&path, details)
        .with_context(|| format!("could not write {}", path.display()))?;
    Ok(path)
}

pub fn remove_dry_run_report(home: &Path) {
    let path = dry_run_report_path(home);
    let _ = std::fs::remove_file(path);
}

pub fn scan_rule(rule: &Rule, options: &ScanOptions) -> RuleScan {
    match rule.kind {
        RuleKind::Paths => scan_paths_rule(rule),
        RuleKind::Downloads => scan_downloads_rule(rule, options.downloads_choice),
        RuleKind::Logs => scan_logs_rule(rule),
    }
}

fn scan_logs_rule(rule: &Rule) -> RuleScan {
    let mut scan = RuleScan {
        rule: rule.clone(),
        bytes: 0,
        entries: 0,
        files: Vec::new(),
        dirs: Vec::new(),
        errors: 0,
        error_messages: Vec::new(),
    };

    let (exclude_set, exclude_errors) = build_globset(&rule.exclude_globs);
    for message in exclude_errors {
        record_error(&mut scan, message);
    }

    let cutoff = rule.older_than_days.and_then(cutoff_from_days);

    for root in rule.expanded_paths() {
        if !root.exists() {
            continue;
        }

        if root.is_file() || root.is_symlink() {
            let base = root.parent().unwrap_or(&root);
            scan_log_path(&root, base, exclude_set.as_ref(), cutoff, &mut scan);
            continue;
        }

        let mut iter = WalkDir::new(&root)
            .follow_links(false)
            .same_file_system(true)
            .into_iter();

        while let Some(next) = iter.next() {
            match next {
                Ok(entry) => {
                    if entry.path() == root {
                        continue;
                    }
                    if entry.file_type().is_dir() {
                        continue;
                    }
                    if entry.file_type().is_symlink() {
                        continue;
                    }
                    if is_excluded(entry.path(), &root, exclude_set.as_ref()) {
                        continue;
                    }
                    if !is_log_file_name(entry.path()) {
                        continue;
                    }
                    let meta = match entry.metadata() {
                        Ok(meta) => meta,
                        Err(err) => {
                            record_error(
                                &mut scan,
                                format!(
                                    "Failed to read metadata for {}: {}",
                                    entry.path().display(),
                                    err
                                ),
                            );
                            continue;
                        }
                    };
                    if !meta.is_file() {
                        continue;
                    }
                    if !is_older_than(&meta, cutoff, entry.path(), &mut scan) {
                        continue;
                    }
                    scan.bytes += meta.len();
                    scan.entries += 1;
                    scan.files.push(entry.path().to_path_buf());
                }
                Err(err) => {
                    if let Some(path) = err.path() {
                        record_error(
                            &mut scan,
                            format!("Failed to read entry {}: {}", path.display(), err),
                        );
                    } else {
                        record_error(
                            &mut scan,
                            format!("Failed to read entry under {}: {}", root.display(), err),
                        );
                    }
                }
            }
        }
    }

    scan
}

fn scan_log_path(
    path: &Path,
    root: &Path,
    exclude: Option<&GlobSet>,
    cutoff: Option<SystemTime>,
    scan: &mut RuleScan,
) {
    if is_excluded(path, root, exclude) {
        return;
    }
    if !is_log_file_name(path) {
        return;
    }
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) => {
            record_error(
                scan,
                format!("Failed to read metadata for {}: {}", path.display(), err),
            );
            return;
        }
    };
    if meta.file_type().is_symlink() || !meta.is_file() {
        return;
    }
    if !is_older_than(&meta, cutoff, path, scan) {
        return;
    }
    scan.bytes += meta.len();
    scan.entries += 1;
    scan.files.push(path.to_path_buf());
}

fn scan_paths_rule(rule: &Rule) -> RuleScan {
    let mut scan = RuleScan {
        rule: rule.clone(),
        bytes: 0,
        entries: 0,
        files: Vec::new(),
        dirs: Vec::new(),
        errors: 0,
        error_messages: Vec::new(),
    };

    let (exclude_set, exclude_errors) = build_globset(&rule.exclude_globs);
    for message in exclude_errors {
        record_error(&mut scan, message);
    }

    for root in rule.expanded_paths() {
        if !root.exists() {
            continue;
        }
        for message in scan_root(&root, exclude_set.as_ref(), &mut scan) {
            record_error(&mut scan, message);
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
        error_messages: Vec::new(),
    };

    let Some(choice) = choice else {
        return scan;
    };

    for root in rule.expanded_paths() {
        let meta = match fs::symlink_metadata(&root) {
            Ok(meta) => meta,
            Err(err) => {
                record_error(
                    &mut scan,
                    format!("Failed to read metadata for {}: {}", root.display(), err),
                );
                continue;
            }
        };
        if meta.file_type().is_symlink() || !meta.is_dir() {
            continue;
        }

        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(err) => {
                record_error(
                    &mut scan,
                    format!("Failed to list {}: {}", root.display(), err),
                );
                continue;
            }
        };

        let mut archives: Vec<(String, PathBuf, u64)> = Vec::new();
        let mut folders: HashMap<String, PathBuf> = HashMap::new();

        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    record_error(
                        &mut scan,
                        format!(
                            "Failed to read directory entry in {}: {}",
                            root.display(),
                            err
                        ),
                    );
                    continue;
                }
            };
            let path = entry.path();
            let name = match entry.file_name().to_str() {
                Some(name) => name.to_string(),
                None => {
                    record_error(
                        &mut scan,
                        format!("Non-utf8 filename in {}", path.display()),
                    );
                    continue;
                }
            };
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(err) => {
                    record_error(
                        &mut scan,
                        format!("Failed to read file type for {}: {}", path.display(), err),
                    );
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
                Err(err) => {
                    record_error(
                        &mut scan,
                        format!("Failed to read metadata for {}: {}", path.display(), err),
                    );
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
                        for message in scan_root(dir_path, None, &mut scan) {
                            record_error(&mut scan, message);
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

fn scan_root(root: &Path, exclude: Option<&GlobSet>, scan: &mut RuleScan) -> Vec<String> {
    if root.is_file() || root.is_symlink() {
        if !is_excluded(root, root, exclude) {
            if let Ok(meta) = fs::symlink_metadata(root) {
                scan.bytes += meta.len();
            }
            scan.entries += 1;
            scan.files.push(root.to_path_buf());
        }
        return Vec::new();
    }

    let mut errors = Vec::new();
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
            Err(err) => {
                if let Some(path) = err.path() {
                    errors.push(format!("Failed to read entry {}: {}", path.display(), err));
                } else {
                    errors.push(format!(
                        "Failed to read entry under {}: {}",
                        root.display(),
                        err
                    ));
                }
            }
        }
    }

    errors
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

fn is_log_file_name(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    if lower == "xsession-errors" || lower.starts_with("xsession-errors.") {
        return true;
    }
    if lower.ends_with(".log") || lower.contains(".log.") {
        return true;
    }
    lower.ends_with(".err") || lower.ends_with(".error")
}

fn cutoff_from_days(days: u64) -> Option<SystemTime> {
    let secs = days.saturating_mul(24 * 60 * 60);
    SystemTime::now().checked_sub(Duration::from_secs(secs))
}

fn is_older_than(
    meta: &fs::Metadata,
    cutoff: Option<SystemTime>,
    path: &Path,
    scan: &mut RuleScan,
) -> bool {
    let Some(cutoff) = cutoff else {
        return true;
    };
    match meta.modified() {
        Ok(modified) => modified <= cutoff,
        Err(err) => {
            record_error(
                scan,
                format!(
                    "Failed to read modified time for {}: {}",
                    path.display(),
                    err
                ),
            );
            false
        }
    }
}

fn summarize_download_dirs(dirs: &[PathBuf]) -> Vec<PathBuf> {
    if dirs.is_empty() {
        return Vec::new();
    }
    let mut sorted = dirs.to_vec();
    sorted.sort_by_key(|path| path.components().count());
    let mut top = Vec::new();
    for dir in sorted {
        if top.iter().any(|root: &PathBuf| dir.starts_with(root)) {
            continue;
        }
        top.push(dir);
    }
    top
}

fn path_is_under_any(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| path.starts_with(root))
}

fn build_globset(patterns: &[String]) -> (Option<GlobSet>, Vec<String>) {
    if patterns.is_empty() {
        return (None, Vec::new());
    }

    let mut builder = GlobSetBuilder::new();
    let mut errors = Vec::new();

    for pattern in patterns {
        match Glob::new(pattern) {
            Ok(glob) => {
                builder.add(glob);
            }
            Err(err) => {
                errors.push(format!("Invalid exclude glob '{}': {}", pattern, err));
            }
        }
    }

    match builder.build() {
        Ok(set) => (Some(set), errors),
        Err(err) => {
            errors.push(format!("Failed to build exclude globs: {}", err));
            (None, errors)
        }
    }
}

fn record_error(scan: &mut RuleScan, message: String) {
    scan.errors += 1;
    scan.error_messages.push(message);
}
