use anyhow::{Context, Result};

use crate::{cli::StatsArgs, db::Database, model::StatsResult};

pub fn run(args: StatsArgs) -> Result<()> {
    let db = Database::open_for_workspace(&args.location.path, args.location.db.as_deref())
        .context("Failed to open database for stats")?;
    let stats = collect(db.conn())?;
    if args.location.json {
        println!("{}", serde_json::to_string_pretty(&stats)?);
    } else {
        println!("Files: {}", stats.files);
        println!("Symbols: {}", stats.symbols);
        println!("Chunks: {}", stats.chunks);
        println!("Edges: {}", stats.edges);
        println!("Git stats files: {}", stats.git_stats_files);
        println!("Git cochange pairs: {}", stats.git_cochange_pairs);
        println!("Database: {}", db.path().display());
    }
    Ok(())
}

pub fn collect(conn: &rusqlite::Connection) -> Result<StatsResult> {
    Ok(StatsResult {
        files: count(conn, "files")?,
        symbols: count(conn, "symbols")?,
        chunks: count(conn, "chunks")?,
        edges: count(conn, "symbol_edges")?,
        git_stats_files: count(conn, "git_file_stats")?,
        git_cochange_pairs: count(conn, "git_cochange")?,
    })
}

fn count(conn: &rusqlite::Connection, table: &str) -> Result<usize> {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    let value: i64 = conn
        .query_row(&sql, [], |row| row.get(0))
        .with_context(|| format!("Failed to count rows in {table}"))?;
    Ok(value as usize)
}
