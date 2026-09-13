use gpui::prelude::*;
use gpui::{App, Window, div, px};
use gpui_component::list::ListItem;
use gpui_component::tree::TreeEntry;
use gpui_component::{Icon, IconName, Sizable, h_flex};

/// One row of a file tree, an icon and a name indented by depth.
/// Clicking a file runs `on_click`.
/// Folders expand and collapse via the tree itself.
pub fn tree_row<F>(ix: usize, entry: &TreeEntry, selected: bool, on_click: F) -> ListItem
where
    F: Fn(&mut Window, &mut App) + 'static,
{
    let item = entry.item();
    let is_folder = entry.is_folder();

    let icon = if is_folder {
        if entry.is_expanded() {
            IconName::FolderOpen
        } else {
            IconName::FolderClosed
        }
    } else {
        IconName::File
    };

    ListItem::new(ix)
        .pl(px(8.) + px(14.) * entry.depth() as f32)
        .selected(selected)
        .child(
            h_flex()
                .gap_2()
                .overflow_hidden()
                .child(Icon::new(icon).small())
                .child(div().text_sm().text_ellipsis().child(item.label.clone())),
        )
        .on_click(move |_event, window, cx| {
            if is_folder {
                return;
            }
            on_click(window, cx);
        })
}
