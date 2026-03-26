mod build;
mod query;
mod stats;
mod update;

use std::path::PathBuf;

use anyhow::Result;
use clap::{ArgAction, Args, Parser, Subcommand};

/// Command-line interface for the local Rust repository indexer.
#[derive(Debug, Parser)]
#[command(
    name = "repo-index",
    version,
    about = "Lightweight local code indexer for Rust workspaces"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    Build(BuildArgs),
    Update(UpdateArgs),
    Stats(StatsArgs),
    Query(QueryArgs),
}

/// Shared index location arguments.
#[derive(Debug, Clone, Args)]
pub struct IndexLocationArgs {
    #[arg(long, default_value = ".")]
    pub path: PathBuf,
    #[arg(long)]
    pub db: Option<PathBuf>,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

/// Build a fresh index for a workspace.
#[derive(Debug, Clone, Args)]
pub struct BuildArgs {
    #[command(flatten)]
    pub location: IndexLocationArgs,
}

/// Incrementally update an existing index.
#[derive(Debug, Clone, Args)]
pub struct UpdateArgs {
    #[command(flatten)]
    pub location: IndexLocationArgs,
}

/// Print index statistics.
#[derive(Debug, Clone, Args)]
pub struct StatsArgs {
    #[command(flatten)]
    pub location: IndexLocationArgs,
}

/// Query the index.
#[derive(Debug, Clone, Args)]
pub struct QueryArgs {
    #[arg(long, default_value = ".")]
    pub workspace: PathBuf,
    #[arg(long)]
    pub db: Option<PathBuf>,
    #[arg(long, default_value_t = false)]
    pub json: bool,
    #[arg(long, default_value_t = true, action = ArgAction::Set)]
    pub compact: bool,
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u8).range(0..=2))]
    pub expand: u8,
    #[command(subcommand)]
    pub command: QueryCommand,
}

/// Query subcommands.
#[derive(Debug, Clone, Subcommand)]
pub enum QueryCommand {
    Symbol {
        query: String,
    },
    File {
        query: String,
    },
    Explain {
        query: String,
    },
    Entrypoints,
    Outline {
        path: PathBuf,
    },
    Neighbors {
        qualname: String,
    },
    ReadSymbol {
        qualname: String,
    },
    Snippet {
        #[arg(long)]
        path: PathBuf,
        #[arg(long)]
        start: usize,
        #[arg(long)]
        end: usize,
    },
    Suggest {
        query: String,
    },
}

pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Build(args) => build::handle(args),
        Command::Update(args) => update::handle(args),
        Command::Stats(args) => stats::handle(args),
        Command::Query(args) => query::handle(args),
    }
}
