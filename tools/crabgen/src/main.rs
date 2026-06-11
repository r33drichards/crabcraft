use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "crabgen", version, about = "WIT-driven guest project generator for crabcraft")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scaffold guest/<name>/ from a starter WIT
    New {
        name: String,
        #[arg(long, value_parser = ["rust", "go", "cpp", "ts"])]
        lang: String,
    },
    /// Re-emit gen/ for one project (or every project with --all)
    Regen {
        path: Option<PathBuf>,
        #[arg(long)]
        all: bool,
    },
    /// Verify every project's gen/ is fresh against its WIT
    Check,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::New { .. } => anyhow::bail!("unimplemented"),
        Command::Regen { .. } => anyhow::bail!("unimplemented"),
        Command::Check => anyhow::bail!("unimplemented"),
    }
}
