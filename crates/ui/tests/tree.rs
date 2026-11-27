//! Flattening a path tree honors expansion and nests children in order.

use std::collections::HashSet;
use std::path::PathBuf;

use ui::tree::{TreeItem, TreeItemSeed};

fn ids(rows: &[TreeItem]) -> Vec<(String, usize)> {
    rows.iter()
        .map(|row| (row.key().to_string(), row.level()))
        .collect()
}

#[test]
fn collapsed_folders_hide_children() {
    let seeds = TreeItemSeed::from_paths(&[
        PathBuf::from("src"),
        PathBuf::from("src/lib.rs"),
        PathBuf::from("README.md"),
    ]);

    let rows = TreeItemSeed::flatten_all(&seeds, &HashSet::new());

    assert_eq!(
        ids(&rows),
        vec![("src".to_owned(), 0), ("README.md".to_owned(), 0)]
    );
    assert!(rows[0].is_expandable());
    assert!(!rows[0].is_expanded());
}

#[test]
fn expanding_a_folder_shows_children_in_order() {
    let seeds = TreeItemSeed::from_paths(&[
        PathBuf::from("src"),
        PathBuf::from("src/lib.rs"),
        PathBuf::from("src/views/mod.rs"),
        PathBuf::from("README.md"),
    ]);
    let expanded = HashSet::from(["src".to_owned(), "src/views".to_owned()]);

    let rows = TreeItemSeed::flatten_all(&seeds, &expanded);

    assert_eq!(
        ids(&rows),
        vec![
            ("src".to_owned(), 0),
            ("src/lib.rs".to_owned(), 1),
            ("src/views".to_owned(), 1),
            ("src/views/mod.rs".to_owned(), 2),
            ("README.md".to_owned(), 0),
        ]
    );
}

#[test]
fn indices_map_to_row_ids() {
    let seeds = TreeItemSeed::from_paths(&[
        PathBuf::from("docs"),
        PathBuf::from("docs/guide.md"),
        PathBuf::from("README.md"),
    ]);
    let expanded = HashSet::from(["docs".to_owned()]);
    let rows = TreeItemSeed::flatten_all(&seeds, &expanded);

    assert_eq!(rows[0].key().to_string(), "docs");
    assert_eq!(rows[1].key().to_string(), "docs/guide.md");
    assert_eq!(rows[2].key().to_string(), "README.md");
}
