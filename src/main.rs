mod clean;
mod cli;
mod config;
mod distro;
mod options;
mod snapshot;
mod tui;

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Parser;
use humansize::{format_size, BINARY};

use crate::clean::scan_rules;
use crate::cli::{CleanArgs, Cli, Commands};
use crate::config::{Config, RuleKind};
use crate::distro::Distro;
use crate::options::{DownloadsChoice, ScanOptions};
use crate::snapshot::SnapshotSupport;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let is_root = is_root();
    let user_home = match &cli.command {
        Some(Commands::Clean(args)) => args.user_home.as_deref(),
        None => None,
    };
    let home = resolve_home(is_root, user_home).context("Failed to resolve home directory")?;
    std::env::set_var("HOME", &home);
    let config = Config::load(cli.config.as_deref())?;
    let distro = distro::detect();
    let snapshot_support = if is_root {
        snapshot::detect(&home)
    } else {
        None
    };

    match &cli.command {
        Some(Commands::Clean(args)) => {
            if args.sudo && !is_root {
                let sudo_args = build_sudo_args(&cli, args, &home)?;
                return reexec_with_sudo(&sudo_args);
            }
            if args.tui {
                let sudo_reexec = build_tui_sudo_reexec(&cli, &home)?;
                let tui_state = load_tui_state(args.tui_state.as_deref())?;
                return handle_tui(tui::run(
                    config.available_rules(&distro),
                    snapshot_support,
                    is_root,
                    args.sudo,
                    args.dry_run,
                    sudo_reexec,
                    tui_state,
                    home.clone(),
                )?);
            }
            run_clean_cli(&config, &distro, args, snapshot_support, is_root, &home)
        }
        None => {
            let sudo_reexec = build_tui_sudo_reexec(&cli, &home)?;
            handle_tui(tui::run(
                config.available_rules(&distro),
                snapshot_support,
                is_root,
                false,
                false,
                sudo_reexec,
                None,
                home.clone(),
            )?)
        }
    }
}

fn run_clean_cli(
    config: &Config,
    distro: &Distro,
    args: &CleanArgs,
    snapshot_support: Option<SnapshotSupport>,
    is_root: bool,
    home: &Path,
) -> Result<()> {
    let available_rules = config.available_rules(distro);

    if args.list_rules {
        print_rules(&available_rules);
        return Ok(());
    }

    let mut rules = available_rules.clone();
    if !args.rules.is_empty() {
        let selected = args
            .rules
            .iter()
            .map(|id| id.to_lowercase())
            .collect::<Vec<_>>();
        rules.retain(|rule| selected.iter().any(|id| id == &rule.id.to_lowercase()));

        let unknown = selected
            .iter()
            .filter(|id| {
                !available_rules
                    .iter()
                    .any(|rule| rule.id.to_lowercase() == **id)
            })
            .cloned()
            .collect::<Vec<_>>();
        if !unknown.is_empty() {
            eprintln!("Unknown rule ids: {}", unknown.join(", "));
        }
    } else {
        rules.retain(|rule| rule.enabled_by_default);
    }

    if !args.sudo {
        rules.retain(|rule| !rule.requires_sudo);
    } else if !is_root {
        bail!("--sudo requires running as root (try: sudo vole clean --sudo)");
    }

    if args.snapshot && !is_root && !args.effective_dry_run() {
        bail!("--snapshot requires root (try: sudo vole clean --sudo --snapshot)");
    }

    if rules.is_empty() {
        println!("No rules selected.");
        return Ok(());
    }

    let downloads_choice = resolve_downloads_choice(&rules, args)?;
    let scan_options = ScanOptions { downloads_choice };
    let scans = scan_rules(&rules, &scan_options);
    print_plan(&scans);

    if args.effective_dry_run() {
        emit_dry_run(&scans, home, args.snapshot)?;
        return Ok(());
    }

    if args.snapshot {
        let support =
            snapshot_support.context("Snapshot requested but no supported provider detected")?;
        let outcome = snapshot::create_snapshot(&support)?;
        println!("{}", outcome.display());
    }

    if !args.yes && !confirm(args.sudo)? {
        println!("Canceled.");
        return Ok(());
    }

    let report = clean::apply(&scans);
    println!(
        "Removed {} files and {} directories",
        report.files_removed, report.dirs_removed
    );
    println!("Freed {}", format_size(report.bytes_freed, BINARY));
    if report.errors > 0 {
        println!("Errors encountered: {}", report.errors);
    }

    Ok(())
}

fn print_rules(rules: &[crate::config::Rule]) {
    println!("Available rules:");
    for rule in rules {
        let sudo = if rule.requires_sudo { " (sudo)" } else { "" };
        let enabled = if rule.enabled_by_default {
            " [default]"
        } else {
            ""
        };
        println!("- {}{}{}", rule.id, sudo, enabled);
        if let Some(desc) = &rule.description {
            println!("  {}", desc);
        }
    }
}

fn print_plan(scans: &[crate::clean::RuleScan]) {
    println!("Cleanup plan:");
    let mut total_bytes = 0;
    let mut total_entries = 0;
    for scan in scans {
        total_bytes += scan.bytes;
        total_entries += scan.entries;
        println!(
            "- {}: {} ({} items)",
            scan.rule.label,
            format_size(scan.bytes, BINARY),
            scan.entries
        );
    }
    println!(
        "Total: {} across {} items",
        format_size(total_bytes, BINARY),
        total_entries
    );
}

fn resolve_downloads_choice(
    rules: &[crate::config::Rule],
    args: &CleanArgs,
) -> Result<Option<DownloadsChoice>> {
    let has_downloads = rules.iter().any(|rule| rule.kind == RuleKind::Downloads);
    if !has_downloads {
        return Ok(None);
    }
    if let Some(choice) = args.downloads_remove {
        return Ok(Some(choice.into()));
    }
    if args.yes {
        bail!("Downloads cleanup requires --downloads-remove when using --yes");
    }
    Ok(Some(prompt_downloads_choice()?))
}

fn emit_dry_run(
    scans: &[crate::clean::RuleScan],
    home: &Path,
    snapshot_requested: bool,
) -> Result<()> {
    let output = clean::dry_run_output(scans);
    print!("{}", output.details);

    match clean::write_dry_run_report(home, &output.details) {
        Ok(path) => println!("Dry-run report saved to {}", path.display()),
        Err(err) => eprintln!("Failed to write dry-run report: {err}"),
    }

    let report = output.report;
    println!(
        "Dry-run listed {} files and {} directories",
        report.files_listed, report.dirs_listed
    );
    println!("Would free {}", format_size(report.bytes_listed, BINARY));
    if snapshot_requested {
        println!("Snapshot skipped in dry-run.");
    }
    if report.errors > 0 {
        println!("Errors encountered: {}", report.errors);
    }
    Ok(())
}

fn confirm(requires_sudo: bool) -> Result<bool> {
    if requires_sudo {
        print!("Sudo mode: type DELETE to confirm: ");
        io::stdout().flush().ok();
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim().to_lowercase();
        return Ok(input == "delete");
    }

    print!("Proceed with deletion? [y/N]: ");
    io::stdout().flush().ok();
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim().to_lowercase();
    Ok(input == "y" || input == "yes")
}

fn prompt_downloads_choice() -> Result<DownloadsChoice> {
    loop {
        print!("Downloads cleanup: remove archives or folders? [a/f]: ");
        io::stdout().flush().ok();
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim().to_ascii_lowercase();
        match input.as_str() {
            "a" | "archive" | "archives" => return Ok(DownloadsChoice::Archives),
            "f" | "folder" | "folders" => return Ok(DownloadsChoice::Folders),
            "" => continue,
            _ => println!("Please enter 'a' for archives or 'f' for folders."),
        }
    }
}

fn handle_tui(exit: tui::TuiExit) -> Result<()> {
    match exit {
        tui::TuiExit::Quit => Ok(()),
        tui::TuiExit::ReexecSudo { args } => reexec_with_sudo(&args),
        tui::TuiExit::Apply {
            rules,
            snapshot,
            downloads_choice,
        } => {
            let scan_options = ScanOptions { downloads_choice };
            let scans = rules
                .iter()
                .map(|rule| clean::scan_rule(rule, &scan_options))
                .collect::<Vec<_>>();
            if let Some(support) = snapshot {
                let outcome = snapshot::create_snapshot(&support)?;
                println!("{}", outcome.display());
            }

            let report = clean::apply(&scans);
            println!(
                "Removed {} files and {} directories",
                report.files_removed, report.dirs_removed
            );
            println!("Freed {}", format_size(report.bytes_freed, BINARY));
            if report.errors > 0 {
                println!("Errors encountered: {}", report.errors);
            }
            Ok(())
        }
    }
}

fn build_sudo_args(cli: &Cli, args: &CleanArgs, home: &Path) -> Result<Vec<String>> {
    let exe = std::env::current_exe()
        .context("Failed to resolve current executable path")?
        .to_string_lossy()
        .to_string();
    let mut sudo_args = vec![exe];
    if let Some(config) = &cli.config {
        sudo_args.push("--config".to_string());
        sudo_args.push(config.to_string_lossy().to_string());
    }
    sudo_args.push("clean".to_string());
    if args.tui {
        sudo_args.push("--tui".to_string());
    }
    sudo_args.push("--sudo".to_string());
    sudo_args.push("--user-home".to_string());
    sudo_args.push(home.to_string_lossy().to_string());
    if args.dry_run {
        sudo_args.push("--dry-run".to_string());
    }
    if let Some(choice) = args.downloads_remove {
        sudo_args.push("--downloads-remove".to_string());
        sudo_args.push(DownloadsChoice::from(choice).to_string());
    }
    if args.snapshot {
        sudo_args.push("--snapshot".to_string());
    }
    if args.yes {
        sudo_args.push("--yes".to_string());
    }
    for rule in &args.rules {
        sudo_args.push("--rule".to_string());
        sudo_args.push(rule.clone());
    }
    if args.list_rules {
        sudo_args.push("--list-rules".to_string());
    }
    Ok(sudo_args)
}

fn build_tui_sudo_reexec(cli: &Cli, home: &Path) -> Result<Option<Vec<String>>> {
    let exe = std::env::current_exe()
        .context("Failed to resolve current executable path")?
        .to_string_lossy()
        .to_string();
    let mut sudo_args = vec![exe];
    if let Some(config) = &cli.config {
        sudo_args.push("--config".to_string());
        sudo_args.push(config.to_string_lossy().to_string());
    }
    sudo_args.push("clean".to_string());
    sudo_args.push("--tui".to_string());
    sudo_args.push("--sudo".to_string());
    sudo_args.push("--user-home".to_string());
    sudo_args.push(home.to_string_lossy().to_string());
    Ok(Some(sudo_args))
}

fn reexec_with_sudo(args: &[String]) -> Result<()> {
    let status = std::process::Command::new("sudo")
        .args(args)
        .status()
        .context("Failed to invoke sudo")?;
    std::process::exit(status.code().unwrap_or(1));
}

fn load_tui_state(path: Option<&Path>) -> Result<Option<tui::PersistedState>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let state = tui::load_state(path)?;
    std::fs::remove_file(path).ok();
    Ok(Some(state))
}

fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

fn resolve_home(is_root: bool, override_home: Option<&Path>) -> Option<PathBuf> {
    if let Some(home) = override_home {
        return Some(home.to_path_buf());
    }
    if is_root {
        if let Some(home) = home_from_sudo_user() {
            return Some(home);
        }
    }
    std::env::var_os("HOME").map(PathBuf::from)
}

fn home_from_sudo_user() -> Option<PathBuf> {
    let user = std::env::var("SUDO_USER").ok()?;
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    let prefix = format!("{user}:");
    for line in passwd.lines() {
        if !line.starts_with(&prefix) {
            continue;
        }
        let mut parts = line.split(':');
        let _name = parts.next();
        let _password = parts.next();
        let _uid = parts.next();
        let _gid = parts.next();
        let _gecos = parts.next();
        let home = parts.next()?;
        return Some(PathBuf::from(home));
    }
    None
}
