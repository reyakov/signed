# gpui-native-kit gaps

Vendored `crates/ui` shortcomings recorded here instead of patched in place.

- `PanelRow` (`src/layout/panel.rs`) seeds its split panes from `window.viewport_size()` during the first render frame. On some launches that size is not yet the window's final bounds, so the divider freezes at a wrong position (observed ~24% instead of the intended 50%) until a drag or a double-click reset. Seeding from the canvas-measured extent, or re-seeding while no user resize has happened, would fix it.
- `island()` (`src/layout/panel.rs`) rounds and paints `overflow_hidden`, but gpui's `ContentMask` is rectangular only: children are never clipped to the rounded corners. Full-bleed panel content (photos, backgrounds) paints square over the island chrome and must apply the matching `theme.radius.overlay` itself — see the signed-out banner in `signed_app`.
