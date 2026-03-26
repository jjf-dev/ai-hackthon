use std::{
    fs,
    os::unix::fs::MetadataExt,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};

use crate::{
    cli::BuildArgs,
    db::{store, Database},
    extractor, git,
    indexer::derived,
    model::FileRecord,
    parser::rust,
    scanner,
};

#[derive(Debug, Clone)]
pub struct BuildReport {
    pub files_indexed: usize,
    pub symbols_indexed: usize,
    pub chunks_indexed: usize,
    pub edges_indexed: usize,
    pub skipped_files: usize,
}

pub(crate) struct PreparedFile {
    pub(crate) file_record: FileRecord,
    pub(crate) imports: Vec<crate::model::FileImportRecord>,
    pub(crate) symbols: Vec<crate::extractor::ExtractedSymbol>,
    pub(crate) chunks: Vec<crate::extractor::ExtractedChunk>,
    pub(crate) edges: Vec<crate::extractor::PendingEdge>,
}

pub fn run(args: BuildArgs) -> Result<()> {
    let workspace = scanner::discover(&args.location.path)?;
    let files = scanner::scan_rust_files(&workspace)?;
    let mut db = Database::open_for_workspace(&workspace.root, args.location.db.as_deref())?;
    let report = build_workspace(db.conn_mut(), &files)?;
    sync_git_metrics(db.conn_mut(), &workspace.root);
    sync_derived_relations(db.conn_mut());

    if args.location.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "workspace_root": workspace.root,
                "db_path": db.path(),
                "report": {
                    "files_indexed": report.files_indexed,
                    "symbols_indexed": report.symbols_indexed,
                    "chunks_indexed": report.chunks_indexed,
                    "edges_indexed": report.edges_indexed,
                    "skipped_files": report.skipped_files,
                }
            }))?
        );
    } else {
        println!("Indexed {} Rust files", report.files_indexed);
        println!("Symbols: {}", report.symbols_indexed);
        println!("Chunks: {}", report.chunks_indexed);
        println!("Edges: {}", report.edges_indexed);
        if report.skipped_files > 0 {
            println!("Skipped files: {}", report.skipped_files);
        }
        println!("Database: {}", db.path().display());
    }

    Ok(())
}

pub(crate) fn build_workspace(
    conn: &mut rusqlite::Connection,
    files: &[scanner::ScannedFile],
) -> Result<BuildReport> {
    store::clear_all(conn)?;
    let tx = conn
        .transaction()
        .context("Failed to start build transaction")?;

    let mut report = BuildReport {
        files_indexed: 0,
        symbols_indexed: 0,
        chunks_indexed: 0,
        edges_indexed: 0,
        skipped_files: 0,
    };

    for file in files {
        match index_one_file(&tx, file) {
            Ok((symbols, chunks, edges)) => {
                report.files_indexed += 1;
                report.symbols_indexed += symbols;
                report.chunks_indexed += chunks;
                report.edges_indexed += edges;
            }
            Err(error) => {
                report.skipped_files += 1;
                eprintln!(
                    "Warning: failed to index {}: {error:#}",
                    file.workspace_path
                );
            }
        }
    }

    tx.commit().context("Failed to commit build transaction")?;
    Ok(report)
}

pub(crate) fn index_one_file(
    tx: &rusqlite::Transaction<'_>,
    file: &scanner::ScannedFile,
) -> Result<(usize, usize, usize)> {
    let prepared = prepare_file_bundle(file)?;
    persist_prepared_file(tx, prepared)
}

pub(crate) fn prepare_file_bundle(file: &scanner::ScannedFile) -> Result<PreparedFile> {
    let source = fs::read_to_string(&file.absolute_path)
        .with_context(|| format!("Failed to read {}", file.absolute_path.display()))?;
    let metadata = fs::metadata(&file.absolute_path)
        .with_context(|| format!("Failed to stat {}", file.absolute_path.display()))?;
    let tree = rust::parse_source(&source)
        .with_context(|| format!("Failed to parse {}", file.absolute_path.display()))?;
    let extracted = extractor::extract_file(&tree, &source, file)
        .with_context(|| format!("Failed to extract {}", file.absolute_path.display()))?;

    let indexed_at = now_unix_seconds()?;
    let file_record = FileRecord {
        id: None,
        path: file.workspace_path.clone(),
        crate_name: file.crate_name.clone(),
        module_path: file.module_path.clone(),
        hash: blake3::hash(source.as_bytes()).to_hex().to_string(),
        size: metadata.len() as i64,
        mtime_ns: Some(metadata.mtime() * 1_000_000_000 + metadata.mtime_nsec()),
        line_count: source.lines().count() as i64,
        symbol_count: extracted.symbols.len() as i64,
        is_generated: file.is_generated,
        has_tests: extracted.symbols.iter().any(|symbol| symbol.record.is_test),
        summary: extracted.summary.clone(),
        indexed_at,
    };

    Ok(PreparedFile {
        file_record,
        imports: extracted.imports,
        symbols: extracted.symbols,
        chunks: extracted.chunks,
        edges: extracted.edges,
    })
}

pub(crate) fn persist_prepared_file(
    tx: &rusqlite::Transaction<'_>,
    prepared: PreparedFile,
) -> Result<(usize, usize, usize)> {
    store::insert_file_bundle(
        tx,
        &prepared.file_record,
        &prepared.imports,
        &prepared.symbols,
        &prepared.chunks,
        &prepared.edges,
    )?;
    Ok((
        prepared.symbols.len(),
        prepared.chunks.len(),
        prepared.edges.len(),
    ))
}

fn now_unix_seconds() -> Result<i64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("System time is before UNIX_EPOCH")?
        .as_secs() as i64)
}

pub(crate) fn sync_git_metrics(conn: &mut rusqlite::Connection, workspace_root: &std::path::Path) {
    match git::stats::collect_metrics(workspace_root) {
        Ok(metrics) => match store::load_file_id_map(conn)
            .and_then(|mapping| store::replace_git_metrics(conn, &mapping, &metrics))
        {
            Ok(()) => {}
            Err(error) => eprintln!("Warning: failed to persist git metrics: {error:#}"),
        },
        Err(error) => eprintln!("Warning: failed to collect git metrics: {error:#}"),
    }
}

pub(crate) fn sync_derived_relations(conn: &mut rusqlite::Connection) {
    if let Err(error) = derived::refresh(conn) {
        eprintln!("Warning: failed to refresh derived relations: {error:#}");
    }
}
