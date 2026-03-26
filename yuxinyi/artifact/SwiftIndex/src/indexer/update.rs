use std::{
    collections::{HashMap, HashSet},
    fs,
    os::unix::fs::MetadataExt,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};

use crate::{
    cli::UpdateArgs,
    db::{store, Database},
    indexer::build,
    scanner,
};

#[derive(Debug, Clone)]
struct UpdateReport {
    added: usize,
    updated: usize,
    deleted: usize,
    unchanged: usize,
    skipped: usize,
}

pub fn run(args: UpdateArgs) -> Result<()> {
    let workspace = scanner::discover(&args.location.path)?;
    let files = scanner::scan_rust_files(&workspace)?;
    let mut db = Database::open_for_workspace(&workspace.root, args.location.db.as_deref())?;
    let report = update_workspace(db.conn_mut(), &files)?;
    build::sync_git_metrics(db.conn_mut(), &workspace.root);
    build::sync_derived_relations(db.conn_mut());

    if args.location.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "workspace_root": workspace.root,
                "db_path": db.path(),
                "report": {
                    "added": report.added,
                    "updated": report.updated,
                    "deleted": report.deleted,
                    "unchanged": report.unchanged,
                    "skipped": report.skipped,
                }
            }))?
        );
    } else {
        println!("Added: {}", report.added);
        println!("Updated: {}", report.updated);
        println!("Deleted: {}", report.deleted);
        println!("Unchanged: {}", report.unchanged);
        if report.skipped > 0 {
            println!("Skipped: {}", report.skipped);
        }
        println!("Database: {}", db.path().display());
    }
    Ok(())
}

fn update_workspace(
    conn: &mut rusqlite::Connection,
    files: &[scanner::ScannedFile],
) -> Result<UpdateReport> {
    let existing = store::load_file_states(conn)?;
    if existing.is_empty() {
        let report = build::build_workspace(conn, files)?;
        return Ok(UpdateReport {
            added: report.files_indexed,
            updated: 0,
            deleted: 0,
            unchanged: 0,
            skipped: report.skipped_files,
        });
    }

    let existing_map = existing
        .into_iter()
        .map(|state| (state.path.clone(), state))
        .collect::<HashMap<_, _>>();
    let current_paths = files
        .iter()
        .map(|file| file.workspace_path.clone())
        .collect::<HashSet<_>>();

    let tx = conn
        .transaction()
        .context("Failed to start update transaction")?;

    let mut report = UpdateReport {
        added: 0,
        updated: 0,
        deleted: 0,
        unchanged: 0,
        skipped: 0,
    };

    for (path, state) in &existing_map {
        if !current_paths.contains(path) {
            store::delete_file_bundle(&tx, state.file_id)?;
            report.deleted += 1;
        }
    }

    for file in files {
        let metadata = fs::metadata(&file.absolute_path)
            .with_context(|| format!("Failed to stat {}", file.absolute_path.display()))?;
        let current_size = metadata.len() as i64;
        let current_mtime_ns = Some(file_mtime_ns(&metadata));
        match existing_map.get(&file.workspace_path) {
            None => match build::prepare_file_bundle(file)
                .and_then(|prepared| build::persist_prepared_file(&tx, prepared))
            {
                Ok(_) => report.added += 1,
                Err(error) => {
                    report.skipped += 1;
                    eprintln!("Warning: failed to add {}: {error:#}", file.workspace_path);
                }
            },
            Some(state) if state.size == current_size && state.mtime_ns == current_mtime_ns => {
                report.unchanged += 1;
            }
            Some(state) => {
                let prepared = match build::prepare_file_bundle(file) {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        report.skipped += 1;
                        eprintln!(
                            "Warning: failed to prepare {}: {error:#}",
                            file.workspace_path
                        );
                        continue;
                    }
                };
                if prepared.file_record.hash == state.hash {
                    store::touch_file_metadata(
                        &tx,
                        state.file_id,
                        current_size,
                        current_mtime_ns,
                        now_unix_seconds()?,
                    )?;
                    report.unchanged += 1;
                    continue;
                }

                store::delete_file_bundle(&tx, state.file_id)?;
                match build::persist_prepared_file(&tx, prepared) {
                    Ok(_) => report.updated += 1,
                    Err(error) => {
                        report.skipped += 1;
                        eprintln!(
                            "Warning: failed to update {}: {error:#}",
                            file.workspace_path
                        );
                    }
                }
            }
        }
    }

    tx.commit().context("Failed to commit update transaction")?;
    Ok(report)
}

fn file_mtime_ns(metadata: &fs::Metadata) -> i64 {
    metadata.mtime() * 1_000_000_000 + metadata.mtime_nsec()
}

fn now_unix_seconds() -> Result<i64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("System time is before UNIX_EPOCH")?
        .as_secs() as i64)
}
