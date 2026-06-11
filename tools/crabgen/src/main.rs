use std::path::PathBuf;

use clap::{Parser, Subcommand};
use crabgen::project::{self, Outcome};

#[derive(Parser)]
#[command(
    name = "crabgen",
    version,
    about = "WIT-driven guest project generator for crabcraft",
    after_help = "The repo root is found by walking up from the current directory to the \
                  first dir containing both guest/ and Cargo.toml; run crabgen from inside \
                  the crabcraft checkout."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scaffold guest/<name>/ from a starter WIT
    New {
        name: String,
        /// Target language: rust, go, cpp, ts
        #[arg(long)]
        lang: String,
    },
    /// Re-emit gen/ for one project (or every project with --all)
    Regen {
        /// Project dir, e.g. guest/hello-go (relative paths resolve against the repo root)
        #[arg(conflicts_with = "all", required_unless_present = "all")]
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
        Command::New { name, lang } => {
            let root = project::find_repo_root()?;
            let outcome = project::new_project(&root, &name, &lang)?;
            println!("created guest/{name} ({lang})");
            print_missing(&outcome);
        }
        Command::Regen { path, all } => {
            let root = project::find_repo_root()?;
            let projects = if all {
                project::discover(&root)?
            } else {
                vec![project::load_at(
                    &root,
                    &path.expect("clap enforces path xor --all"),
                )?]
            };
            for p in &projects {
                let outcome = project::regen(p)?;
                println!("regenerated {}/gen ({})", p.rel, p.manifest.lang);
                print_missing(&outcome);
            }
        }
        Command::Check => {
            let root = project::find_repo_root()?;
            let stale = project::stale_projects(&root)?;
            if !stale.is_empty() {
                for p in &stale {
                    eprintln!("stale: {} (WIT changed since gen/ was written)", p.rel);
                    eprintln!("  run: crabgen regen {}", p.rel);
                }
                std::process::exit(1);
            }
            // no stale projects (or none managed by crabgen at all): quiet success
        }
    }
    Ok(())
}

fn print_missing(outcome: &Outcome) {
    if outcome.missing_impls.is_empty() {
        return;
    }
    println!("add these to {}:", outcome.impl_file);
    for sig in &outcome.missing_impls {
        println!("  {sig}");
    }
}
