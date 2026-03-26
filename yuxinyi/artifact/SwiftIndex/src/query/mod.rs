pub mod common;
pub mod compactor;
pub mod entrypoints;
pub mod explain;
pub mod files;
pub mod neighbors;
pub mod outline;
pub mod stats;
pub mod suggest;
pub mod symbols;

use anyhow::Result;

use crate::cli::{QueryArgs, QueryCommand};

pub fn dispatch(args: QueryArgs) -> Result<()> {
    let options = common::QueryOptions::new(args.compact, args.expand);
    match args.command {
        QueryCommand::Symbol { query } => symbols::run(
            &args.workspace,
            args.db.as_deref(),
            &query,
            args.json,
            options,
        ),
        QueryCommand::File { query } => files::run(
            &args.workspace,
            args.db.as_deref(),
            &query,
            args.json,
            options,
        ),
        QueryCommand::Explain { query } => explain::run(
            &args.workspace,
            args.db.as_deref(),
            &query,
            args.json,
            options,
        ),
        QueryCommand::Entrypoints => {
            entrypoints::run(&args.workspace, args.db.as_deref(), args.json)
        }
        QueryCommand::Outline { path } => outline::run(
            &args.workspace,
            args.db.as_deref(),
            &path,
            args.json,
            options,
        ),
        QueryCommand::Neighbors { qualname } => neighbors::run(
            &args.workspace,
            args.db.as_deref(),
            &qualname,
            args.json,
            options,
        ),
        QueryCommand::ReadSymbol { qualname } => {
            symbols::read_symbol(&args.workspace, args.db.as_deref(), &qualname, args.json)
        }
        QueryCommand::Snippet { path, start, end } => outline::snippet(
            &args.workspace,
            args.db.as_deref(),
            &path,
            start,
            end,
            args.json,
        ),
        QueryCommand::Suggest { query } => suggest::run(
            &args.workspace,
            args.db.as_deref(),
            &query,
            args.json,
            options,
        ),
    }
}
