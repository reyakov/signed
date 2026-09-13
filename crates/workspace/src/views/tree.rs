use std::collections::HashMap;
use std::path::PathBuf;

use gpui_component::tree::TreeItem;

pub(crate) struct TreeItemSeed {
    /// Path of the node, relative to the worktree root.
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) children: Vec<TreeItemSeed>,
}

pub(crate) fn tree_items(seeds: Vec<TreeItemSeed>, expand_folders: bool) -> Vec<TreeItem> {
    fn convert(seed: TreeItemSeed, expand_folders: bool) -> TreeItem {
        let mut item = TreeItem::new(seed.id, seed.label);
        if expand_folders && !seed.children.is_empty() {
            item = item.expanded(true);
        }
        item.children = seed
            .children
            .into_iter()
            .map(|seed| convert(seed, expand_folders))
            .collect();
        item
    }

    seeds
        .into_iter()
        .map(|seed| convert(seed, expand_folders))
        .collect()
}

/// Build nested tree items from a flat entry list sorted dirs-first.
pub(crate) fn build_tree_items(entries: &[PathBuf]) -> Vec<TreeItemSeed> {
    // Node indices by full path, so parents resolve in constant time while inserting.
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut nodes: Vec<(String, String, Vec<usize>)> = Vec::new();
    let mut roots: Vec<usize> = Vec::new();

    for entry in entries {
        let mut parent: Option<usize> = None;
        let mut path = String::new();
        for part in entry.components() {
            let label = part.as_os_str().to_string_lossy().into_owned();
            path = if path.is_empty() {
                label.clone()
            } else {
                format!("{path}/{label}")
            };
            let ix = *index.entry(path.clone()).or_insert_with(|| {
                let ix = nodes.len();
                nodes.push((path.clone(), label.clone(), Vec::new()));
                match parent {
                    Some(parent) => nodes[parent].2.push(ix),
                    None => roots.push(ix),
                }
                ix
            });
            parent = Some(ix);
        }
    }

    fn assemble(ix: usize, nodes: &[(String, String, Vec<usize>)]) -> TreeItemSeed {
        let (id, label, children) = &nodes[ix];
        TreeItemSeed {
            id: id.clone(),
            label: label.clone(),
            children: children
                .iter()
                .map(|child| assemble(*child, nodes))
                .collect(),
        }
    }

    roots.iter().map(|root| assemble(*root, &nodes)).collect()
}

/// Sorted relative paths of a worktree snapshot.
///
/// Compared against the `worktree_paths` of a repository panel to skip
/// rebuilding the explorer when a refresh left the worktree unchanged.
pub(crate) fn sorted_worktree_paths(entries: &[PathBuf]) -> Vec<String> {
    let mut paths: Vec<String> = entries
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    paths.sort();
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_nested_tree_from_flat_entries() {
        let entries = vec![
            PathBuf::from("src"),
            PathBuf::from("src/lib.rs"),
            PathBuf::from("README.md"),
            PathBuf::from("docs/guide.md"),
        ];

        let items = build_tree_items(&entries);

        // Input order is preserved, dirs-first as produced by worktree_entries.
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].label, "src");
        assert_eq!(items[0].id, "src");
        assert_eq!(items[0].children.len(), 1);
        assert_eq!(items[0].children[0].label, "lib.rs");
        assert_eq!(items[0].children[0].id, "src/lib.rs");

        assert_eq!(items[1].label, "README.md");
        assert_eq!(items[1].id, "README.md");

        assert_eq!(items[2].label, "docs");
        assert_eq!(items[2].children[0].label, "guide.md");
        assert_eq!(items[2].children[0].id, "docs/guide.md");
    }

    #[test]
    fn tree_builder_handles_deep_nesting() {
        let entries = vec![
            PathBuf::from("a"),
            PathBuf::from("a/b"),
            PathBuf::from("a/b/c.txt"),
        ];

        let items = build_tree_items(&entries);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].children[0].id, "a/b");
        assert_eq!(items[0].children[0].children[0].id, "a/b/c.txt");
    }

    #[test]
    fn tree_builder_merges_shared_prefixes() {
        // File children of a directory arrive after other directories' entries.
        // The worktree list is dirs-first globally.
        // The shared prefix must still resolve to one node.
        let entries = vec![
            PathBuf::from("a/x.txt"),
            PathBuf::from("b/y.txt"),
            PathBuf::from("a/z.txt"),
        ];

        let items = build_tree_items(&entries);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "a");
        assert_eq!(items[0].children.len(), 2);
        assert_eq!(items[1].label, "b");
    }
}
