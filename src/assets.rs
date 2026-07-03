// Assets from the `assets/` folder embedded into the binary at build time, so
// the program is self-contained and doesn't need the folder beside it at
// runtime. `img.png` is intentionally excluded - it only serves the README.
//
// The large item-sprite atlas (`brass_atlas.zip`) is NOT embedded: it's picked
// by the user at runtime (see the sidebar).

pub const FONT_JSON: &[u8] = include_bytes!("../assets/font.json");
pub const ASCII_PNG: &[u8] = include_bytes!("../assets/ascii.png");
pub const GLINT_PNG: &[u8] = include_bytes!("../assets/enchanted_glint_item.png");
pub const CONTAINER_9_SLICE_PNG: &[u8] = include_bytes!("../assets/container_9_slice.png");
pub const INVENTORY_SLOTS_PNG: &[u8] = include_bytes!("../assets/inventory_slots.png");
pub const SLOTS_BACKGROUND_PNG: &[u8] = include_bytes!("../assets/slots_background.png");

/// Look up an embedded asset by its original file name.
pub fn get(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "font.json" => FONT_JSON,
        "ascii.png" => ASCII_PNG,
        "enchanted_glint_item.png" => GLINT_PNG,
        "container_9_slice.png" => CONTAINER_9_SLICE_PNG,
        "inventory_slots.png" => INVENTORY_SLOTS_PNG,
        "slots_background.png" => SLOTS_BACKGROUND_PNG,
        _ => return None,
    })
}
