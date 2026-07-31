//! Reusable policies for selecting archive input content.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

const CROSS_PLATFORM_CLEAN_EXCLUDES: [&str; 3] = [".DS_Store", "._*", "__MACOSX"];

/// Named content policy resolved into the exclude globs consumed by archive
/// creation. `KeepAllFiles` and `Custom` differ in product intent; both add no
/// implicit rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreateContentPolicy {
    /// Remove common macOS helper files from archives shared across platforms.
    CrossPlatformClean,
    /// Keep every selected file unless an explicit rule excludes it.
    KeepAllFiles,
    /// Use only explicit exclude rules.
    Custom,
}

impl CreateContentPolicy {
    /// Resolves this policy and caller-supplied rules into one stable,
    /// de-duplicated exclude list.
    pub fn resolve_excludes(self, explicit: &[String]) -> Vec<String> {
        let implicit: &[&str] = match self {
            Self::CrossPlatformClean => &CROSS_PLATFORM_CLEAN_EXCLUDES,
            Self::KeepAllFiles | Self::Custom => &[],
        };
        let mut seen = HashSet::with_capacity(implicit.len().saturating_add(explicit.len()));
        let mut resolved = Vec::with_capacity(implicit.len().saturating_add(explicit.len()));
        for rule in implicit {
            if seen.insert(*rule) {
                resolved.push((*rule).to_owned());
            }
        }
        for rule in explicit {
            if seen.insert(rule.as_str()) {
                resolved.push(rule.clone());
            }
        }
        resolved
    }
}

#[cfg(test)]
mod tests {
    use super::CreateContentPolicy;

    #[test]
    fn cross_platform_policy_prepends_noise_rules_and_stably_deduplicates() {
        let explicit = vec![
            "*.tmp".to_owned(),
            ".DS_Store".to_owned(),
            "*.tmp".to_owned(),
            ".git".to_owned(),
        ];

        assert_eq!(
            CreateContentPolicy::CrossPlatformClean.resolve_excludes(&explicit),
            [".DS_Store", "._*", "__MACOSX", "*.tmp", ".git"]
        );
    }

    #[test]
    fn keep_all_and_custom_preserve_explicit_rule_order() {
        let explicit = vec!["*.tmp".to_owned(), ".git".to_owned(), "*.tmp".to_owned()];
        let expected = vec!["*.tmp".to_owned(), ".git".to_owned()];

        assert_eq!(
            CreateContentPolicy::KeepAllFiles.resolve_excludes(&explicit),
            expected
        );
        assert_eq!(
            CreateContentPolicy::Custom.resolve_excludes(&explicit),
            expected
        );
    }
}
