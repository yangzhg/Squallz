//! Shared literal path-search normalization and ranking.

/// Match quality for an archive path search, ordered from strongest to weakest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ArchivePathSearchRank {
    /// The query equals the final path component.
    ExactName,
    /// The final path component starts with the query.
    NamePrefix,
    /// The final path component contains the query.
    NameContains,
    /// The full archive path contains the query.
    PathContains,
}

/// Trims and folds a user query for literal archive-path matching.
///
/// Backslashes are treated as archive separators so callers behave the same
/// across desktop platforms. An all-whitespace query folds to an empty string.
pub fn fold_archive_search_query(query: &str) -> String {
    fold_archive_search_path(query.trim())
}

/// Normalizes separators and folds an archive path for case-insensitive search.
pub fn fold_archive_search_path(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

/// Ranks an already-folded archive path against an already-folded query.
///
/// Both arguments should come from [`fold_archive_search_path`] or
/// [`fold_archive_search_query`]. Matching is literal rather than glob or
/// regular-expression based. Empty queries never match.
pub fn rank_folded_archive_path(
    folded_path: &str,
    folded_query: &str,
) -> Option<ArchivePathSearchRank> {
    if folded_query.is_empty() {
        return None;
    }
    let path = folded_path.trim_end_matches('/');
    let name = path.rsplit('/').next().unwrap_or(path);
    if name == folded_query {
        Some(ArchivePathSearchRank::ExactName)
    } else if name.starts_with(folded_query) {
        Some(ArchivePathSearchRank::NamePrefix)
    } else if name.contains(folded_query) {
        Some(ArchivePathSearchRank::NameContains)
    } else if path.contains(folded_query) {
        Some(ArchivePathSearchRank::PathContains)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_folding_normalizes_query_whitespace_case_and_separators() {
        assert_eq!(
            fold_archive_search_query("  RÉSUMÉ\\Final  "),
            "résumé/final"
        );
        assert_eq!(fold_archive_search_query("  \t "), "");
        assert_eq!(
            fold_archive_search_path("Docs\\Summary.PDF"),
            "docs/summary.pdf"
        );
    }

    #[test]
    fn folded_path_ranking_prefers_names_before_parent_paths() {
        let query = fold_archive_search_query("sum");
        assert_eq!(
            rank_folded_archive_path("reports/sum", &query),
            Some(ArchivePathSearchRank::ExactName)
        );
        assert_eq!(
            rank_folded_archive_path("reports/summary.pdf", &query),
            Some(ArchivePathSearchRank::NamePrefix)
        );
        assert_eq!(
            rank_folded_archive_path("reports/annual-summary.pdf", &query),
            Some(ArchivePathSearchRank::NameContains)
        );
        assert_eq!(
            rank_folded_archive_path("summary/report.pdf", &query),
            Some(ArchivePathSearchRank::PathContains)
        );
        assert_eq!(rank_folded_archive_path("reports/final.pdf", &query), None);
    }

    #[test]
    fn folded_directory_path_uses_its_final_component_as_the_name() {
        assert_eq!(
            rank_folded_archive_path("reports/quarter/", "quarter"),
            Some(ArchivePathSearchRank::ExactName)
        );
        assert_eq!(rank_folded_archive_path("reports/quarter/", ""), None);
    }
}
