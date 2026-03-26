use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use cargo_metadata::{Metadata, MetadataCommand, Package, PackageId, Target};
use ignore::WalkBuilder;

/// Cargo workspace view used by the indexer.
#[derive(Debug, Clone)]
pub struct WorkspaceInfo {
    pub root: PathBuf,
    pub members: Vec<CrateInfo>,
}

/// Crate/package information extracted from cargo metadata.
#[derive(Debug, Clone)]
pub struct CrateInfo {
    pub package_id: PackageId,
    pub package_name: String,
    pub manifest_path: PathBuf,
    pub root_dir: PathBuf,
    pub default_namespace: String,
    pub explicit_targets: Vec<TargetNamespace>,
}

/// Cargo target namespace used to disambiguate package-level sources.
#[derive(Debug, Clone)]
pub struct TargetNamespace {
    pub namespace: String,
    pub source_path: PathBuf,
    pub module_dir: Option<PathBuf>,
}

/// A Rust source file selected for indexing.
#[derive(Debug, Clone)]
pub struct ScannedFile {
    pub absolute_path: PathBuf,
    pub workspace_path: String,
    pub crate_name: Option<String>,
    pub module_path: Option<String>,
    pub is_generated: bool,
}

/// Resolve cargo workspace metadata for the target path.
pub fn discover(path: &Path) -> Result<WorkspaceInfo> {
    let metadata = MetadataCommand::new()
        .current_dir(path)
        .exec()
        .with_context(|| format!("Failed to run cargo metadata in {}", path.display()))?;

    let root = metadata.workspace_root.as_std_path().to_path_buf();
    let members = collect_members(&metadata)?;
    Ok(WorkspaceInfo { root, members })
}

fn collect_members(metadata: &Metadata) -> Result<Vec<CrateInfo>> {
    let mut members = Vec::new();
    for member_id in &metadata.workspace_members {
        let package = metadata
            .packages
            .iter()
            .find(|package| &package.id == member_id)
            .with_context(|| format!("Workspace member {member_id} missing from metadata"))?;
        members.push(package_to_crate_info(package));
    }
    Ok(members)
}

fn package_to_crate_info(package: &Package) -> CrateInfo {
    let manifest_path = package.manifest_path.as_std_path().to_path_buf();
    let root_dir = manifest_path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let package_name = sanitize_ident(&package.name);
    let has_lib = package
        .targets
        .iter()
        .any(|target| target.kind.iter().any(|kind| kind == "lib"));
    let default_namespace = package
        .targets
        .iter()
        .find(|target| target.kind.iter().any(|kind| kind == "lib"))
        .map(target_namespace_name)
        .or_else(|| {
            package
                .targets
                .iter()
                .find(|target| {
                    target.kind.iter().any(|kind| kind == "bin")
                        && target.src_path.as_std_path().ends_with("src/main.rs")
                })
                .map(target_namespace_name)
        })
        .unwrap_or_else(|| package_name.clone());
    let explicit_targets = package
        .targets
        .iter()
        .filter_map(|target| build_explicit_target(target, &package_name, has_lib))
        .collect();

    CrateInfo {
        package_id: package.id.clone(),
        package_name,
        manifest_path,
        root_dir,
        default_namespace,
        explicit_targets,
    }
}

/// Scan the workspace for Rust source files while honoring ignore rules.
pub fn scan_rust_files(workspace: &WorkspaceInfo) -> Result<Vec<ScannedFile>> {
    let mut builder = WalkBuilder::new(&workspace.root);
    builder.standard_filters(true);
    builder.hidden(false);
    builder.git_ignore(true);
    builder.git_exclude(true);
    builder.parents(true);

    let mut files = Vec::new();
    for result in builder.build() {
        let entry = result.with_context(|| {
            format!(
                "Failed while scanning workspace {}",
                workspace.root.display()
            )
        })?;
        let path = entry.path();
        if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        if !should_index_path(path) {
            continue;
        }

        let absolute_path = path.to_path_buf();
        let relative_path = absolute_path
            .strip_prefix(&workspace.root)
            .with_context(|| {
                format!(
                    "Failed to relativize {} against workspace root {}",
                    absolute_path.display(),
                    workspace.root.display()
                )
            })?
            .to_path_buf();
        let workspace_path = relative_path.to_string_lossy().replace('\\', "/");
        let crate_info = classify_crate(&workspace.members, &absolute_path);

        files.push(ScannedFile {
            absolute_path: absolute_path.clone(),
            workspace_path: workspace_path.clone(),
            crate_name: crate_info.map(|info| infer_namespace(info, &absolute_path)),
            module_path: crate_info.and_then(|info| infer_module_path(info, &absolute_path)),
            is_generated: is_generated_path(&workspace_path),
        });
    }

    files.sort_by(|left, right| left.workspace_path.cmp(&right.workspace_path));
    Ok(files)
}

fn should_index_path(path: &Path) -> bool {
    if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
        return false;
    }

    for component in path.components() {
        if let Component::Normal(value) = component {
            let name = value.to_string_lossy();
            if matches!(
                name.as_ref(),
                "target" | ".git" | "node_modules" | "dist" | "vendor"
            ) {
                return false;
            }
        }
    }

    true
}

fn classify_crate<'a>(members: &'a [CrateInfo], path: &Path) -> Option<&'a CrateInfo> {
    members
        .iter()
        .filter(|member| path.starts_with(&member.root_dir))
        .max_by_key(|member| member.root_dir.components().count())
}

fn infer_namespace(crate_info: &CrateInfo, absolute_path: &Path) -> String {
    crate_info
        .explicit_targets
        .iter()
        .find_map(|target| {
            if absolute_path == target.source_path
                || target
                    .module_dir
                    .as_ref()
                    .map(|module_dir| absolute_path.starts_with(module_dir))
                    .unwrap_or(false)
            {
                Some(target.namespace.clone())
            } else {
                None
            }
        })
        .unwrap_or_else(|| {
            if let Ok(relative) = absolute_path.strip_prefix(&crate_info.root_dir) {
                let first = relative
                    .components()
                    .next()
                    .and_then(|component| match component {
                        Component::Normal(value) => Some(value.to_string_lossy().to_string()),
                        _ => None,
                    });
                match first.as_deref() {
                    Some("tests") => format!("{}__tests", crate_info.package_name),
                    Some("examples") => format!("{}__examples", crate_info.package_name),
                    Some("benches") => format!("{}__benches", crate_info.package_name),
                    _ => crate_info.default_namespace.clone(),
                }
            } else {
                crate_info.default_namespace.clone()
            }
        })
}

fn infer_module_path(crate_info: &CrateInfo, absolute_path: &Path) -> Option<String> {
    if let Some(target) = crate_info.explicit_targets.iter().find(|target| {
        absolute_path == target.source_path
            || target
                .module_dir
                .as_ref()
                .map(|module_dir| absolute_path.starts_with(module_dir))
                .unwrap_or(false)
    }) {
        if absolute_path == target.source_path {
            return None;
        }
        if let Some(module_dir) = &target.module_dir {
            if let Ok(relative) = absolute_path.strip_prefix(module_dir) {
                return path_to_module(relative);
            }
        }
    }

    let relative = absolute_path.strip_prefix(&crate_info.root_dir).ok()?;
    let mut components: Vec<_> = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().to_string()),
            _ => None,
        })
        .collect();

    if components.is_empty() {
        return None;
    }

    match components.first().map(String::as_str) {
        Some("src") => {
            components.remove(0);
            if components.first().map(String::as_str) == Some("bin") {
                return None;
            }
            module_parts_to_path(components)
        }
        Some("tests") | Some("examples") | Some("benches") => {
            let root = components.remove(0);
            let mut parts = vec![root];
            if let Some(module) = module_parts_to_path(components) {
                parts.push(module);
            }
            Some(parts.join("::"))
        }
        _ => module_parts_to_path(components),
    }
}

fn module_parts_to_path(mut components: Vec<String>) -> Option<String> {
    let file_name = components.pop()?;
    let stem = file_name.strip_suffix(".rs").unwrap_or(&file_name);
    if stem != "lib" && stem != "main" && stem != "mod" {
        components.push(stem.to_string());
    }
    if components.is_empty() {
        None
    } else {
        Some(components.join("::"))
    }
}

fn path_to_module(relative: &Path) -> Option<String> {
    let components = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    module_parts_to_path(components)
}

fn build_explicit_target(
    target: &Target,
    package_name: &str,
    has_lib: bool,
) -> Option<TargetNamespace> {
    let kind = target.kind.first()?.as_str();
    let source_path = target.src_path.as_std_path().to_path_buf();
    let namespace = explicit_namespace(kind, target, package_name);
    let module_dir = sibling_module_dir(&source_path);

    match kind {
        "bin" => {
            if source_path.ends_with("src/main.rs") && !has_lib {
                None
            } else {
                Some(TargetNamespace {
                    namespace,
                    source_path,
                    module_dir,
                })
            }
        }
        "test" | "example" | "bench" => Some(TargetNamespace {
            namespace,
            source_path,
            module_dir,
        }),
        _ => None,
    }
}

fn explicit_namespace(kind: &str, target: &Target, package_name: &str) -> String {
    let target_name = sanitize_ident(&target.name);
    match kind {
        "bin" => format!("{package_name}__bin__{target_name}"),
        "test" => format!("{package_name}__test__{target_name}"),
        "example" => format!("{package_name}__example__{target_name}"),
        "bench" => format!("{package_name}__bench__{target_name}"),
        _ => target_namespace_name(target),
    }
}

fn target_namespace_name(target: &Target) -> String {
    sanitize_ident(&target.name)
}

fn sanitize_ident(value: &str) -> String {
    value.replace('-', "_")
}

fn sibling_module_dir(source_path: &Path) -> Option<PathBuf> {
    let parent = source_path.parent()?;
    let stem = source_path.file_stem()?.to_string_lossy();
    if stem == "lib" || stem == "main" || stem == "mod" {
        None
    } else {
        Some(parent.join(stem.as_ref()))
    }
}

fn is_generated_path(path: &str) -> bool {
    path.ends_with(".generated.rs") || path.ends_with("/bindings.rs") || path == "bindings.rs"
}
