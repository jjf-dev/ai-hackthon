/// Score exact and prefix textual matches.
pub fn lexical_score(query: &str, candidate: &str) -> f64 {
    if candidate == query {
        100.0
    } else if candidate.starts_with(query) {
        60.0
    } else if candidate.contains(query) {
        30.0
    } else {
        0.0
    }
}
