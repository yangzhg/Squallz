use std::cmp::Reverse;

use serde::Deserialize;
use squallz_core::api::{unix_seconds, EntryMeta, EntryType};
use squallz_core::ArchivePathSearchRank;

#[derive(Debug, Deserialize)]
pub struct BrowseRequest {
    pub location: BrowseLocation,
    pub sort: Option<BrowseSort>,
    pub generation: u64,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowseLocation {
    Directory {
        prefix: String,
        filter: Option<String>,
    },
    Search {
        query: String,
    },
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Deserialize)]
pub struct BrowseSort {
    key: Column,
    direction: Direction,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Column {
    Name,
    Size,
    Modified,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Direction {
    Asc,
    Desc,
}

#[derive(Eq, PartialEq, Ord, PartialOrd)]
pub(super) enum ColumnOrder {
    Relevance(ArchivePathSearchRank),
    Name(String),
    ReverseName(Reverse<String>),
    Number(i128),
    Missing,
}

impl BrowseSort {
    pub(super) fn column_order(self, name: &str, entry: Option<&EntryMeta>) -> ColumnOrder {
        let value = match self.key {
            Column::Name => {
                return match self.direction {
                    Direction::Asc => ColumnOrder::Name(name.to_lowercase()),
                    Direction::Desc => ColumnOrder::ReverseName(Reverse(name.to_lowercase())),
                }
            }
            Column::Size => entry
                .filter(|meta| !matches!(meta.entry_type, EntryType::Dir))
                .map(|meta| i128::from(meta.size)),
            Column::Modified => entry
                .and_then(|meta| meta.modified)
                .and_then(unix_seconds)
                .map(i128::from),
        };
        value.map_or(ColumnOrder::Missing, |value| {
            ColumnOrder::Number(if self.direction == Direction::Desc {
                -value
            } else {
                value
            })
        })
    }
}
