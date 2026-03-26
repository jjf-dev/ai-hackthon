use serde::Serialize;

/// Supported lightweight relationship types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeType {
    Contains,
    Imports,
    DeclaresMod,
    Implements,
    Tests,
    Calls,
}

impl EdgeType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Contains => "contains",
            Self::Imports => "imports",
            Self::DeclaresMod => "declares_mod",
            Self::Implements => "implements",
            Self::Tests => "tests",
            Self::Calls => "calls",
        }
    }
}

/// Relationship edge persisted in SQLite.
#[derive(Debug, Clone, Serialize)]
pub struct EdgeRecord {
    pub id: Option<i64>,
    pub from_symbol_id: Option<i64>,
    pub to_symbol_id: Option<i64>,
    pub edge_type: EdgeType,
    pub evidence: Option<String>,
}
