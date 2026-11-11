use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

pub struct TreeItemSeed {
    /// Path of the node, relative to the worktree root.
    pub id: String,
    pub label: String,
    pub children: Vec<TreeItemSeed>,
}

/// One visible row of the flattened tree.
#[derive(Clone)]
pub struct TreeRow {
    pub id: String,
    pub label: String,
    pub depth: usize,
    pub expandable: bool,
    pub expanded: bool,
}

/// Flattens the seeds into the rows a flat tree renders, honoring `expanded`.
pub fn flatten(seeds: &[TreeItemSeed], expanded: &HashSet<String>) -> Vec<TreeRow> {
    fn push(
        seed: &TreeItemSeed,
        depth: usize,
        expanded: &HashSet<String>,
        rows: &mut Vec<TreeRow>,
    ) {
        let is_expanded = expanded.contains(&seed.id);
        rows.push(TreeRow {
            id: seed.id.clone(),
            label: seed.label.clone(),
            depth,
            expandable: !seed.children.is_empty(),
            expanded: is_expanded,
        });

        if is_expanded {
            for child in &seed.children {
                push(child, depth + 1, expanded, rows);
            }
        }
    }

    let mut rows = Vec::new();
    for seed in seeds {
        push(seed, 0, expanded, &mut rows);
    }
    rows
}

/// Build nested tree items from a flat entry list sorted dirs-first.
pub fn build_tree_items(entries: &[PathBuf]) -> Vec<TreeItemSeed> {
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
pub fn sorted_worktree_paths(entries: &[PathBuf]) -> Vec<String> {
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

    fn ids(rows: &[TreeRow]) -> Vec<(&str, usize)> {
        rows.iter()
            .map(|row| (row.id.as_str(), row.depth))
            .collect()
    }

    #[test]
    fn collapsed_folders_hide_children() {
        let seeds = build_tree_items(&[
            PathBuf::from("src"),
            PathBuf::from("src/lib.rs"),
            PathBuf::from("README.md"),
        ]);

        let rows = flatten(&seeds, &HashSet::new());

        assert_eq!(ids(&rows), vec![("src", 0), ("README.md", 0)]);
        assert!(rows[0].expandable);
        assert!(!rows[0].expanded);
    }

    #[test]
    fn expanding_a_folder_shows_children_in_order() {
        let seeds = build_tree_items(&[
            PathBuf::from("src"),
            PathBuf::from("src/lib.rs"),
            PathBuf::from("src/views/mod.rs"),
            PathBuf::from("README.md"),
        ]);
        let expanded = HashSet::from(["src".to_owned(), "src/views".to_owned()]);

        let rows = flatten(&seeds, &expanded);

        assert_eq!(
            ids(&rows),
            vec![
                ("src", 0),
                ("src/lib.rs", 1),
                ("src/views", 1),
                ("src/views/mod.rs", 2),
                ("README.md", 0),
            ]
        );
    }

    #[test]
    fn indices_map_to_row_ids() {
        let seeds = build_tree_items(&[
            PathBuf::from("docs"),
            PathBuf::from("docs/guide.md"),
            PathBuf::from("README.md"),
        ]);
        let expanded = HashSet::from(["docs".to_owned()]);
        let rows = flatten(&seeds, &expanded);

        assert_eq!(rows[0].id, "docs");
        assert_eq!(rows[1].id, "docs/guide.md");
        assert_eq!(rows[2].id, "README.md");
    }
}
