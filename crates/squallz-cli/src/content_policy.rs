use squallz_core::CreateContentPolicy;

pub(crate) fn resolve_create_excludes(
    policy: Option<CreateContentPolicy>,
    explicit: Vec<String>,
) -> Vec<String> {
    match policy {
        Some(policy) => policy.resolve_excludes(&explicit),
        None => explicit,
    }
}

#[cfg(test)]
mod tests {
    use squallz_core::CreateContentPolicy;

    use super::resolve_create_excludes;

    #[test]
    fn omitted_policy_preserves_the_legacy_explicit_list() {
        let explicit = vec!["*.tmp".to_owned(), "*.tmp".to_owned()];

        assert_eq!(resolve_create_excludes(None, explicit.clone()), explicit);
    }

    #[test]
    fn selected_policy_uses_the_shared_resolver() {
        let explicit = vec!["*.tmp".to_owned(), ".DS_Store".to_owned()];
        let resolved =
            resolve_create_excludes(Some(CreateContentPolicy::CrossPlatformClean), explicit);

        assert_eq!(resolved, [".DS_Store", "._*", "__MACOSX", "*.tmp"]);
    }
}
