use std::{
    collections::{BTreeSet, HashMap},
    path::Path,
    process::Command,
};

use anyhow::{bail, Context, Result};

/// Git-derived hotspot summary keyed by workspace-relative path.
#[derive(Debug, Clone)]
pub struct GitFileStat {
    pub path: String,
    pub commit_count: i64,
    pub last_modified: Option<i64>,
    pub last_author: Option<String>,
    pub authors: Vec<String>,
}

/// Pairwise file cochange metric keyed by workspace-relative paths.
#[derive(Debug, Clone)]
pub struct GitCochange {
    pub path_a: String,
    pub path_b: String,
    pub cochange_count: i64,
}

/// Combined git-derived metrics.
#[derive(Debug, Clone, Default)]
pub struct GitMetrics {
    pub file_stats: Vec<GitFileStat>,
    pub cochanges: Vec<GitCochange>,
}

#[derive(Debug, Clone)]
struct CommitMeta {
    timestamp: i64,
    author: String,
}

#[derive(Debug, Clone, Default)]
struct FileAccumulator {
    commit_count: i64,
    last_modified: Option<i64>,
    last_author: Option<String>,
    authors: BTreeSet<String>,
}

/// Collect git hotspot and cochange metrics from `git log`.
pub fn collect_metrics(workspace_root: &Path) -> Result<GitMetrics> {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .args([
            "log",
            "--format=COMMIT%x1f%ct%x1f%an",
            "--name-only",
            "--no-renames",
        ])
        .output()
        .with_context(|| format!("Failed to execute git log in {}", workspace_root.display()))?;
    if !output.status.success() {
        bail!(
            "git log failed in {}: {}",
            workspace_root.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8(output.stdout).context("git log produced invalid UTF-8")?;
    let mut current_commit: Option<CommitMeta> = None;
    let mut current_files = BTreeSet::new();
    let mut file_stats: HashMap<String, FileAccumulator> = HashMap::new();
    let mut cochanges: HashMap<(String, String), i64> = HashMap::new();

    for line in stdout.lines() {
        if let Some(meta) = line.strip_prefix("COMMIT\x1f") {
            flush_commit(&current_files, &mut cochanges);
            current_files.clear();

            let mut parts = meta.split('\x1f');
            let timestamp = parts
                .next()
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or_default();
            let author = parts.next().unwrap_or_default().to_string();
            current_commit = Some(CommitMeta { timestamp, author });
            continue;
        }

        if line.trim().is_empty() {
            continue;
        }

        let path = line.trim().replace('\\', "/");
        let Some(commit) = &current_commit else {
            continue;
        };
        let entry = file_stats.entry(path.clone()).or_default();
        entry.commit_count += 1;
        if entry.last_modified.is_none() {
            entry.last_modified = Some(commit.timestamp);
            entry.last_author = Some(commit.author.clone());
        }
        entry.authors.insert(commit.author.clone());
        current_files.insert(path);
    }
    flush_commit(&current_files, &mut cochanges);

    Ok(GitMetrics {
        file_stats: file_stats
            .into_iter()
            .map(|(path, acc)| GitFileStat {
                path,
                commit_count: acc.commit_count,
                last_modified: acc.last_modified,
                last_author: acc.last_author,
                authors: acc.authors.into_iter().collect(),
            })
            .collect(),
        cochanges: cochanges
            .into_iter()
            .map(|((path_a, path_b), cochange_count)| GitCochange {
                path_a,
                path_b,
                cochange_count,
            })
            .collect(),
    })
}

fn flush_commit(files: &BTreeSet<String>, cochanges: &mut HashMap<(String, String), i64>) {
    if files.len() < 2 {
        return;
    }

    let files = files.iter().cloned().collect::<Vec<_>>();
    for left_index in 0..files.len() {
        for right_index in (left_index + 1)..files.len() {
            let left = files[left_index].clone();
            let right = files[right_index].clone();
            *cochanges.entry((left, right)).or_insert(0) += 1;
        }
    }
}
