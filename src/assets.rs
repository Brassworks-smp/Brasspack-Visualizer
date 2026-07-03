
pub const FONT_JSON: &[u8] = include_bytes!("../assets/font.json");
pub const ASCII_PNG: &[u8] = include_bytes!("../assets/ascii.png");
pub const GLINT_PNG: &[u8] = include_bytes!("../assets/enchanted_glint_item.png");
pub const CONTAINER_9_SLICE_PNG: &[u8] = include_bytes!("../assets/container_9_slice.png");
pub const INVENTORY_SLOTS_PNG: &[u8] = include_bytes!("../assets/inventory_slots.png");
pub const SLOTS_BACKGROUND_PNG: &[u8] = include_bytes!("../assets/slots_background.png");

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
