use serde::Serialize;

/// Supported Rust symbol kinds for the MVP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Struct,
    Enum,
    Trait,
    Impl,
    Module,
    Const,
    Static,
    TypeAlias,
}

impl SymbolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Function => "fn",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Trait => "trait",
            Self::Impl => "impl",
            Self::Module => "mod",
            Self::Const => "const",
            Self::Static => "static",
            Self::TypeAlias => "type",
        }
    }
}

/// Indexed symbol metadata.
#[derive(Debug, Clone, Serialize)]
pub struct SymbolRecord {
    pub id: Option<i64>,
    pub file_id: Option<i64>,
    pub parent_symbol_id: Option<i64>,
    pub kind: SymbolKind,
    pub name: String,
    pub qualname: String,
    pub signature: Option<String>,
    pub docs: Option<String>,
    pub start_line: i64,
    pub end_line: i64,
    pub is_async: bool,
    pub is_test: bool,
    pub visibility: Option<String>,
    pub return_type: Option<String>,
    pub summary: String,
}
