use std::collections::HashMap;

use anyhow::Result;
use serde_json::json;
use tree_sitter::{Node, Tree};

use crate::{
    extractor::{
        chunk_for_symbol,
        edges::PendingEdge,
        summary::{summarize_file, summarize_symbol},
    },
    model::{ChunkRecord, EdgeType, FileImportRecord, SymbolKind, SymbolRecord},
    parser::rust::{line_range, node_text},
    scanner::ScannedFile,
};

/// Symbol plus local parent link before persistence.
#[derive(Debug, Clone)]
pub struct ExtractedSymbol {
    pub record: SymbolRecord,
    pub parent_local: Option<usize>,
}

/// Chunk plus local symbol link before persistence.
#[derive(Debug, Clone)]
pub struct ExtractedChunk {
    pub record: ChunkRecord,
    pub symbol_local: Option<usize>,
}

/// All extracted artifacts for a single Rust file.
#[derive(Debug, Clone)]
pub struct ExtractedFile {
    pub symbols: Vec<ExtractedSymbol>,
    pub chunks: Vec<ExtractedChunk>,
    pub imports: Vec<FileImportRecord>,
    pub edges: Vec<PendingEdge>,
    pub summary: String,
}

#[derive(Debug, Clone)]
struct Scope {
    qual_prefix: Vec<String>,
    parent_local: Option<usize>,
}

#[derive(Debug, Clone)]
struct CallHint {
    caller_local: usize,
    raw_target: String,
    normalized_targets: Vec<String>,
    line: usize,
}

#[derive(Debug, Clone)]
struct TestHint {
    caller_local: usize,
    target_hint: String,
}

#[derive(Debug, Clone)]
struct ImplHint {
    impl_local: usize,
    trait_name: String,
}

struct Extractor<'a> {
    source: &'a str,
    symbols: Vec<ExtractedSymbol>,
    chunks: Vec<ExtractedChunk>,
    imports: Vec<FileImportRecord>,
    edges: Vec<PendingEdge>,
    call_hints: Vec<CallHint>,
    test_hints: Vec<TestHint>,
    impl_hints: Vec<ImplHint>,
    qualname_counts: HashMap<String, usize>,
}

/// Extract symbols, imports, chunks, and lightweight relations from a parsed file.
pub fn extract_file(tree: &Tree, source: &str, file: &ScannedFile) -> Result<ExtractedFile> {
    let base_prefix = base_prefix(file);
    let root = tree.root_node();
    let mut extractor = Extractor {
        source,
        symbols: Vec::new(),
        chunks: Vec::new(),
        imports: Vec::new(),
        edges: Vec::new(),
        call_hints: Vec::new(),
        test_hints: Vec::new(),
        impl_hints: Vec::new(),
        qualname_counts: HashMap::new(),
    };

    extractor.walk_items(
        root,
        &Scope {
            qual_prefix: base_prefix,
            parent_local: None,
        },
    );
    extractor.resolve_hints();

    let symbol_records = extractor
        .symbols
        .iter()
        .map(|symbol| symbol.record.clone())
        .collect::<Vec<_>>();
    let summary = summarize_file(
        file.crate_name.as_deref(),
        file.module_path.as_deref(),
        &symbol_records,
    );

    Ok(ExtractedFile {
        symbols: extractor.symbols,
        chunks: extractor.chunks,
        imports: extractor.imports,
        edges: extractor.edges,
        summary,
    })
}

impl<'a> Extractor<'a> {
    fn walk_items(&mut self, node: Node<'_>, scope: &Scope) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            match child.kind() {
                "function_item" => self.capture_function(child, scope),
                "struct_item" => self.capture_simple_symbol(child, scope, SymbolKind::Struct),
                "enum_item" => self.capture_simple_symbol(child, scope, SymbolKind::Enum),
                "trait_item" => self.capture_trait(child, scope),
                "impl_item" => self.capture_impl(child, scope),
                "mod_item" => self.capture_mod(child, scope),
                "const_item" => self.capture_simple_symbol(child, scope, SymbolKind::Const),
                "static_item" => self.capture_simple_symbol(child, scope, SymbolKind::Static),
                "type_item" => self.capture_simple_symbol(child, scope, SymbolKind::TypeAlias),
                "use_declaration" => self.capture_import(child, scope),
                "declaration_list" | "source_file" => self.walk_items(child, scope),
                _ => {}
            }
        }
    }

    fn capture_import(&mut self, node: Node<'_>, scope: &Scope) {
        let import_text = collapse_whitespace(node_text(node, self.source));
        let alias = import_text
            .split(" as ")
            .nth(1)
            .map(|value| value.trim_end_matches(';').trim().to_string());
        let is_glob = import_text.contains('*');
        let import_path = import_text
            .strip_prefix("use ")
            .unwrap_or(import_text.as_str())
            .trim_end_matches(';')
            .trim()
            .to_string();

        self.imports.push(FileImportRecord {
            id: None,
            file_id: None,
            import_path: import_path.clone(),
            alias,
            is_glob,
        });
        self.edges.push(PendingEdge {
            from_local: scope.parent_local,
            to_local: None,
            edge_type: EdgeType::Imports,
            evidence: Some(json!({ "import": import_path }).to_string()),
        });
    }

    fn capture_function(&mut self, node: Node<'_>, scope: &Scope) {
        let name = field_text(node, "name", self.source).unwrap_or_else(|| "anonymous".to_string());
        let body = node.child_by_field_name("body");
        let signature = body
            .map(|body| collapse_whitespace(&self.source[node.start_byte()..body.start_byte()]))
            .or_else(|| Some(collapse_whitespace(node_text(node, self.source))));
        let docs = extract_docs_before(self.source, node.start_position().row);
        let return_type = field_text(node, "return_type", self.source);
        let is_async = signature
            .as_deref()
            .map(|value| value.contains("async fn"))
            .unwrap_or(false);
        let is_test = signature
            .as_deref()
            .map(|value| value.contains("#[test]") || value.contains("::test"))
            .unwrap_or(false);
        let visibility = find_visibility(node, self.source);
        let (start_line, end_line) = line_range(node);

        let qualname = self.next_qualname(scope.qual_prefix.clone(), &name, start_line);
        let mut record = SymbolRecord {
            id: None,
            file_id: None,
            parent_symbol_id: None,
            kind: SymbolKind::Function,
            name: name.clone(),
            qualname,
            signature,
            docs,
            start_line: start_line as i64,
            end_line: end_line as i64,
            is_async,
            is_test,
            visibility,
            return_type,
            summary: String::new(),
        };
        record.summary = summarize_symbol(&record);
        let local = self.push_symbol(record, scope.parent_local, node);

        if let Some(body) = body {
            self.collect_call_hints(body, local);
        }
        if is_test {
            self.test_hints.extend(test_hints_for_name(local, &name));
        }
    }

    fn capture_simple_symbol(&mut self, node: Node<'_>, scope: &Scope, kind: SymbolKind) {
        let name =
            field_text(node, "name", self.source).unwrap_or_else(|| kind.as_str().to_string());
        let docs = extract_docs_before(self.source, node.start_position().row);
        let visibility = find_visibility(node, self.source);
        let (start_line, end_line) = line_range(node);
        let qualname = self.next_qualname(scope.qual_prefix.clone(), &name, start_line);
        let signature = Some(collapse_whitespace(
            node_text(node, self.source)
                .lines()
                .next()
                .unwrap_or_default(),
        ));

        let mut record = SymbolRecord {
            id: None,
            file_id: None,
            parent_symbol_id: None,
            kind,
            name,
            qualname,
            signature,
            docs,
            start_line: start_line as i64,
            end_line: end_line as i64,
            is_async: false,
            is_test: false,
            visibility,
            return_type: None,
            summary: String::new(),
        };
        record.summary = summarize_symbol(&record);
        self.push_symbol(record, scope.parent_local, node);
    }

    fn capture_trait(&mut self, node: Node<'_>, scope: &Scope) {
        let name = field_text(node, "name", self.source).unwrap_or_else(|| "Trait".to_string());
        let docs = extract_docs_before(self.source, node.start_position().row);
        let visibility = find_visibility(node, self.source);
        let (start_line, end_line) = line_range(node);
        let qualname = self.next_qualname(scope.qual_prefix.clone(), &name, start_line);
        let mut record = SymbolRecord {
            id: None,
            file_id: None,
            parent_symbol_id: None,
            kind: SymbolKind::Trait,
            name: name.clone(),
            qualname,
            signature: Some(collapse_whitespace(
                node_text(node, self.source)
                    .lines()
                    .next()
                    .unwrap_or_default(),
            )),
            docs,
            start_line: start_line as i64,
            end_line: end_line as i64,
            is_async: false,
            is_test: false,
            visibility,
            return_type: None,
            summary: String::new(),
        };
        record.summary = summarize_symbol(&record);
        let local = self.push_symbol(record, scope.parent_local, node);

        if let Some(body) = node.child_by_field_name("body") {
            let mut prefix = scope.qual_prefix.clone();
            prefix.push(name);
            self.walk_items(
                body,
                &Scope {
                    qual_prefix: prefix,
                    parent_local: Some(local),
                },
            );
        }
    }

    fn capture_mod(&mut self, node: Node<'_>, scope: &Scope) {
        let name = field_text(node, "name", self.source).unwrap_or_else(|| "mod".to_string());
        let docs = extract_docs_before(self.source, node.start_position().row);
        let visibility = find_visibility(node, self.source);
        let (start_line, end_line) = line_range(node);
        let qualname = self.next_qualname(scope.qual_prefix.clone(), &name, start_line);
        let mut record = SymbolRecord {
            id: None,
            file_id: None,
            parent_symbol_id: None,
            kind: SymbolKind::Module,
            name: name.clone(),
            qualname,
            signature: Some(collapse_whitespace(
                node_text(node, self.source)
                    .lines()
                    .next()
                    .unwrap_or_default(),
            )),
            docs,
            start_line: start_line as i64,
            end_line: end_line as i64,
            is_async: false,
            is_test: false,
            visibility,
            return_type: None,
            summary: String::new(),
        };
        record.summary = summarize_symbol(&record);
        let local = self.push_symbol(record, scope.parent_local, node);

        self.edges.push(PendingEdge {
            from_local: scope.parent_local,
            to_local: Some(local),
            edge_type: EdgeType::DeclaresMod,
            evidence: None,
        });

        if let Some(body) = node.child_by_field_name("body") {
            let mut prefix = scope.qual_prefix.clone();
            prefix.push(name);
            self.walk_items(
                body,
                &Scope {
                    qual_prefix: prefix,
                    parent_local: Some(local),
                },
            );
        }
    }

    fn capture_impl(&mut self, node: Node<'_>, scope: &Scope) {
        let type_name = field_text(node, "type", self.source).unwrap_or_else(|| "Type".to_string());
        let trait_name = field_text(node, "trait", self.source);
        let impl_name = match &trait_name {
            Some(trait_name) => format!("impl {trait_name} for {type_name}"),
            None => format!("impl {type_name}"),
        };
        let docs = extract_docs_before(self.source, node.start_position().row);
        let visibility = find_visibility(node, self.source);
        let (start_line, end_line) = line_range(node);
        let mut prefix = scope.qual_prefix.clone();
        prefix.push(sanitize_namespace_segment(&type_name));
        if let Some(trait_name) = &trait_name {
            prefix.push(format!("impl[{}]", sanitize_namespace_segment(trait_name)));
        } else {
            prefix.push("impl".to_string());
        }
        let qualname = self.next_qualname(prefix, "__impl__", start_line);
        let mut record = SymbolRecord {
            id: None,
            file_id: None,
            parent_symbol_id: None,
            kind: SymbolKind::Impl,
            name: impl_name,
            qualname,
            signature: Some(collapse_whitespace(
                node_text(node, self.source)
                    .lines()
                    .next()
                    .unwrap_or_default(),
            )),
            docs,
            start_line: start_line as i64,
            end_line: end_line as i64,
            is_async: false,
            is_test: false,
            visibility,
            return_type: None,
            summary: String::new(),
        };
        record.summary = summarize_symbol(&record);
        let local = self.push_symbol(record, scope.parent_local, node);

        if let Some(trait_name) = &trait_name {
            self.impl_hints.push(ImplHint {
                impl_local: local,
                trait_name: strip_generic_suffix(trait_name),
            });
        }

        if let Some(body) = node.child_by_field_name("body") {
            let mut method_prefix = scope.qual_prefix.clone();
            method_prefix.push(sanitize_namespace_segment(&type_name));
            if let Some(trait_name) = &trait_name {
                method_prefix.push(sanitize_namespace_segment(trait_name));
            }
            self.walk_items(
                body,
                &Scope {
                    qual_prefix: method_prefix,
                    parent_local: Some(local),
                },
            );
        }
    }

    fn push_symbol(
        &mut self,
        record: SymbolRecord,
        parent_local: Option<usize>,
        node: Node<'_>,
    ) -> usize {
        let local = self.symbols.len();
        self.symbols.push(ExtractedSymbol {
            record: record.clone(),
            parent_local,
        });
        self.chunks.push(ExtractedChunk {
            record: chunk_for_symbol(
                record.kind.as_str(),
                record.start_line,
                record.end_line,
                record.summary.clone(),
                node_text(node, self.source).to_string(),
            ),
            symbol_local: Some(local),
        });
        if parent_local.is_some() {
            self.edges.push(PendingEdge {
                from_local: parent_local,
                to_local: Some(local),
                edge_type: EdgeType::Contains,
                evidence: None,
            });
        }
        local
    }

    fn collect_call_hints(&mut self, node: Node<'_>, caller_local: usize) {
        let mut stack = vec![node];
        while let Some(current) = stack.pop() {
            if current.kind() == "call_expression" {
                if let Some(function_node) = current.child_by_field_name("function") {
                    let raw_target = collapse_whitespace(node_text(function_node, self.source));
                    let normalized_targets = normalize_call_target(&raw_target);
                    self.call_hints.push(CallHint {
                        caller_local,
                        raw_target,
                        normalized_targets,
                        line: current.start_position().row + 1,
                    });
                }
            }

            let mut cursor = current.walk();
            for child in current.named_children(&mut cursor) {
                stack.push(child);
            }
        }
    }

    fn resolve_hints(&mut self) {
        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_qualname: HashMap<String, usize> = HashMap::new();
        for (index, symbol) in self.symbols.iter().enumerate() {
            by_name
                .entry(symbol.record.name.clone())
                .or_default()
                .push(index);
            by_qualname.insert(symbol.record.qualname.clone(), index);
        }

        for hint in &self.call_hints {
            let resolved = resolve_call_target(hint, &self.symbols, &by_name, &by_qualname);
            self.edges.push(PendingEdge {
                from_local: Some(hint.caller_local),
                to_local: resolved,
                edge_type: EdgeType::Calls,
                evidence: Some(
                    json!({
                        "callee": hint.raw_target,
                        "line": hint.line,
                        "resolved": resolved.is_some(),
                    })
                    .to_string(),
                ),
            });
        }

        for hint in &self.test_hints {
            let resolved = by_name.get(&hint.target_hint).and_then(|entries| {
                entries
                    .iter()
                    .copied()
                    .find(|index| !self.symbols[*index].record.is_test)
            });
            self.edges.push(PendingEdge {
                from_local: Some(hint.caller_local),
                to_local: resolved,
                edge_type: EdgeType::Tests,
                evidence: Some(
                    json!({
                        "target_hint": hint.target_hint,
                        "resolved": resolved.is_some(),
                    })
                    .to_string(),
                ),
            });
        }

        for hint in &self.impl_hints {
            let resolved = by_name.get(&hint.trait_name).and_then(|entries| {
                entries
                    .iter()
                    .copied()
                    .find(|index| self.symbols[*index].record.kind == SymbolKind::Trait)
            });
            self.edges.push(PendingEdge {
                from_local: Some(hint.impl_local),
                to_local: resolved,
                edge_type: EdgeType::Implements,
                evidence: Some(
                    json!({
                        "trait": hint.trait_name,
                        "resolved": resolved.is_some(),
                    })
                    .to_string(),
                ),
            });
        }
    }

    fn next_qualname(&mut self, mut prefix: Vec<String>, name: &str, line: usize) -> String {
        prefix.push(name.to_string());
        let base = prefix.join("::");
        let count = self.qualname_counts.entry(base.clone()).or_insert(0);
        let qualname = if *count == 0 {
            base
        } else {
            format!("{base}#L{line}")
        };
        *count += 1;
        qualname
    }
}

fn base_prefix(file: &ScannedFile) -> Vec<String> {
    let mut prefix = Vec::new();
    if let Some(crate_name) = &file.crate_name {
        prefix.push(crate_name.clone());
    }
    if let Some(module_path) = &file.module_path {
        prefix.extend(module_path.split("::").map(ToString::to_string));
    }
    prefix
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn field_text(node: Node<'_>, field_name: &str, source: &str) -> Option<String> {
    node.child_by_field_name(field_name)
        .map(|child| collapse_whitespace(node_text(child, source)))
}

fn find_visibility(node: Node<'_>, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    let visibility = node
        .children(&mut cursor)
        .find(|child| child.kind() == "visibility_modifier")
        .map(|child| collapse_whitespace(node_text(child, source)));
    visibility
}

fn extract_docs_before(source: &str, start_row: usize) -> Option<String> {
    let lines = source.lines().collect::<Vec<_>>();
    if start_row == 0 || start_row > lines.len() {
        return None;
    }

    let mut docs = Vec::new();
    let mut index = start_row;
    while index > 0 {
        index -= 1;
        let line = lines[index].trim();
        if line.starts_with("///") || line.starts_with("//!") {
            docs.push(
                line.trim_start_matches("///")
                    .trim_start_matches("//!")
                    .trim()
                    .to_string(),
            );
        } else if line.is_empty() {
            continue;
        } else {
            break;
        }
    }

    if docs.is_empty() {
        None
    } else {
        docs.reverse();
        Some(docs.join("\n"))
    }
}

fn test_hints_for_name(local: usize, name: &str) -> Vec<TestHint> {
    let mut hints = Vec::new();
    let raw = name
        .trim_start_matches("test_")
        .trim_start_matches("should_");
    if !raw.is_empty() && raw != name {
        hints.push(TestHint {
            caller_local: local,
            target_hint: raw.to_string(),
        });
    }
    if let Some(stripped) = raw.strip_prefix("it_") {
        hints.push(TestHint {
            caller_local: local,
            target_hint: stripped.to_string(),
        });
    }
    hints
}

fn normalize_call_target(raw: &str) -> Vec<String> {
    let compact = raw.replace(' ', "");
    let stripped = strip_generic_suffix(&compact);
    let mut candidates = vec![stripped.clone()];
    if let Some(last) = stripped.rsplit("::").next() {
        candidates.push(last.to_string());
    }
    if let Some(last) = stripped.rsplit('.').next() {
        candidates.push(last.to_string());
    }
    candidates.sort();
    candidates.dedup();
    candidates
}

fn resolve_call_target(
    hint: &CallHint,
    symbols: &[ExtractedSymbol],
    by_name: &HashMap<String, Vec<usize>>,
    by_qualname: &HashMap<String, usize>,
) -> Option<usize> {
    for target in &hint.normalized_targets {
        if let Some(index) = by_qualname.get(target) {
            return Some(*index);
        }
        if target.contains("::") {
            if let Some((index, _)) = symbols
                .iter()
                .enumerate()
                .filter(|(_, symbol)| symbol.record.qualname.ends_with(target))
                .max_by_key(|(_, symbol)| {
                    shared_prefix_segments(
                        &symbols[hint.caller_local].record.qualname,
                        &symbol.record.qualname,
                    )
                })
            {
                return Some(index);
            }
        }
        if let Some(entries) = by_name.get(target) {
            if let Some(best) = entries
                .iter()
                .copied()
                .filter(|index| *index != hint.caller_local)
                .max_by_key(|index| {
                    shared_prefix_segments(
                        &symbols[hint.caller_local].record.qualname,
                        &symbols[*index].record.qualname,
                    )
                })
            {
                return Some(best);
            }
        }
    }
    None
}

fn shared_prefix_segments(left: &str, right: &str) -> usize {
    left.split("::")
        .zip(right.split("::"))
        .take_while(|(left, right)| left == right)
        .count()
}

fn sanitize_namespace_segment(value: &str) -> String {
    strip_generic_suffix(value)
        .trim_start_matches('&')
        .trim_start_matches("mut ")
        .trim()
        .replace("::", "_")
        .replace(['<', '>', '(', ')', '[', ']', ',', ' '], "_")
}

fn strip_generic_suffix(value: &str) -> String {
    if let Some(index) = value.find('<') {
        value[..index].to_string()
    } else {
        value.to_string()
    }
}
