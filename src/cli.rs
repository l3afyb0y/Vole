use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "vole")]
#[command(about = "Safe TUI/CLI cleanup utility for Linux", long_about = None)]
pub struct Cli {
    /// Path to a JSON config file.
    #[arg(long)]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Scan and clean using the CLI (or launch the clean TUI).
    Clean(CleanArgs),
}

#[derive(Args, Debug, Clone)]
pub struct CleanArgs {
    /// Launch the interactive TUI instead of the CLI flow.
    #[arg(long)]
    pub tui: bool,

    /// Preview cleanup without deleting (default is apply with confirmation).
    #[arg(long)]
    pub dry_run: bool,

    /// Include system rules that require sudo/root.
    #[arg(long)]
    pub sudo: bool,

    /// Internal: path to a saved TUI state file.
    #[arg(long, hide = true)]
    pub tui_state: Option<PathBuf>,

    /// Create a snapshot before cleaning (if supported).
    #[arg(long)]
    pub snapshot: bool,

    /// Skip the confirmation prompt when applying.
    #[arg(long)]
    pub yes: bool,

    /// Limit to specific rule IDs (repeatable).
    #[arg(long = "rule")]
    pub rules: Vec<String>,

    /// List available rules and exit.
    #[arg(long)]
    pub list_rules: bool,
}

impl CleanArgs {
    pub fn effective_dry_run(&self) -> bool {
        self.dry_run
    }
}
