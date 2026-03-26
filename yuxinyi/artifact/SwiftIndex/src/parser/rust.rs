use anyhow::{Context, Result};
use tree_sitter::{Node, Parser, Tree};

/// Create a Rust parser backed by tree-sitter-rust.
pub fn create_parser() -> Result<Parser> {
    let mut parser = Parser::new();
    let language = tree_sitter_rust::language();
    parser
        .set_language(&language)
        .context("Failed to load tree-sitter Rust grammar")?;
    Ok(parser)
}

/// Parse a Rust source string into a tree-sitter tree.
pub fn parse_source(source: &str) -> Result<Tree> {
    let mut parser = create_parser()?;
    parser
        .parse(source, None)
        .context("tree-sitter returned no parse tree")
}

/// Return 1-based line numbers for a node.
pub fn line_range(node: Node<'_>) -> (usize, usize) {
    (node.start_position().row + 1, node.end_position().row + 1)
}

/// Borrow the source text covered by the node.
pub fn node_text<'a>(node: Node<'a>, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}
