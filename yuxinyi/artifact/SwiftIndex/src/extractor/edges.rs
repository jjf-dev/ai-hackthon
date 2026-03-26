use crate::model::EdgeType;

/// Edge extracted before database ids are assigned.
#[derive(Debug, Clone)]
pub struct PendingEdge {
    pub from_local: Option<usize>,
    pub to_local: Option<usize>,
    pub edge_type: EdgeType,
    pub evidence: Option<String>,
}
