# ui gaps

Known Signed features `ui` does not currently support. Recorded instead of
modifying it or reimplementing a parallel mechanism.

1. **Custom themes.** `Signed Light`/`Signed Dark`, theme registry names, font
   sizes, radii, focus ring, and shadows are removed. `ui` derives colors,
   radii, fonts, and chrome from the OS; `AppearanceMode` is preserved through
   `set_mode_override`/`follow_system`.
2. **Tab bar navigation.** The previous/next tab buttons and
   `tab_bar.hide_navigation_buttons` are removed; `ui` tab strip has none.
3. **Traffic-light placement.** Custom macOS traffic-light position and the 44px
   tab bar are removed. `ui` uses `theme.titlebar_height` and a fixed 78px
   traffic-light pad.
4. **Virtual list.** `ui` has no data-driven virtual list. Screens keep
   `gpui::uniform_list` for fixed-height lists and `ScrollArea` for the Inbox.
5. **Editable code field.** `ui` code view is read-only and supports four
   grammars. Signed only previews files read-only; other extensions render as
   plain text.
6. **Hunk-only diffs.** The `ui` `DiffViewState` needs the full old and new texts;
   `CommitDiff` carries hunks only. The custom hunk rows are retained.
7. **Tree data.** `ui` tree is flat with caller-managed expansion.
   Folder rows use `IconRole::Folder` and file rows use `IconRole::File`.
8. **Dock panels.** `ui` has no dock, tab-group, or view registry API: `Panel`
   covers one split row and `Resizable` stacked panes. Per-repository panels
   register a builder keyed by a unique id and stay in the builder map. No dock
   persistence, zoom, or bottom/right placement; splits only.
9. **Dialogs.** Dialogs are elements mounted at the window root; there is no
   `window.open_dialog`, dialog stack, or non-dismissible option. A single-slot
   `DialogHost` is used, and the passphrase dialog ignores dismiss requests.
10. **Notifications.** `ui` toast shows one toast at a time; backend errors
    queue but only the current toast is visible.
11. **Segmented badges.** `Segmented` has no badge slot; filter counts fold
    into labels.
12. **Combobox trigger.** `Combobox` has no custom trigger renderer; branch/tag
    selectors lose the leading git icon and custom caret.
