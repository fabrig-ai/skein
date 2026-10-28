//! Capacities derived from 64 notes and the record ceilings (ui.md, 15).
#[must_use]
pub fn limits() -> skein_ui::Limits {
    skein_ui::Limits {
        nodes: 600,
        depth: 6,
        text_per_node: 512,
        text_bytes: 24_000,
        drawing_bytes: 64,
        styles_per_node: 1,
        patches: 1200,
        bindings: 81,
        slots: 0,
        markdown_depth: 0,
        scene_marks: 6,
        scene_bytes: 256,
    }
}
