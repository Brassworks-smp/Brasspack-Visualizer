use eframe::egui::Color32;

// Compile-time hex -> color helpers, so the palette reads as `rgb(0x56ab60)`
// instead of `Color32::from_rgb(86, 171, 96)`.
pub(crate) const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

pub(crate) const fn rgb3(hex: u32) -> [u8; 3] {
    [(hex >> 16) as u8, (hex >> 8) as u8, hex as u8]
}

pub(crate) const fn rgb_bytes(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}
