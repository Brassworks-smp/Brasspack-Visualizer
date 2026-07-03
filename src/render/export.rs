
use std::collections::HashMap;

use image::{Rgba, RgbaImage};

use crate::render::atlas::Atlas;
use crate::model::{format_count, Entry};

const SLOT: u32 = 44;
const ICON: u32 = 40;
const PAD: u32 = 18;
const COLS: u32 = 9;
const HEADER_ICON: u32 = 64;

pub struct McFont {
    glyphs: HashMap<char, (RgbaImage, u32)>,
    height: u32,
    space: u32,
}

impl McFont {
    pub fn load() -> Result<McFont, String> {
        let font_json = crate::assets::get("font.json").ok_or("embedded font.json missing")?;
        let cfg: serde_json::Value =
            serde_json::from_slice(font_json).map_err(|e| format!("font.json: {e}"))?;
        let rows: Vec<String> = cfg["providers"]
            .as_array()
            .and_then(|a| a.iter().find(|p| p["type"] == "bitmap"))
            .and_then(|p| p["chars"].as_array())
            .map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
            .ok_or("no bitmap provider in font.json")?;

        let ascii = crate::assets::get("ascii.png").ok_or("embedded ascii.png missing")?;
        let sheet = image::load_from_memory(ascii)
            .map_err(|e| format!("ascii.png: {e}"))?
            .to_rgba8();
        let (sw, sh) = sheet.dimensions();
        let cw = sw / 16;
        let ch = sh / 16;

        let mut glyphs = HashMap::new();
        for (r, row) in rows.iter().enumerate() {
            for (c, chr) in row.chars().enumerate() {
                if chr == '\0' {
                    continue;
                }
                let g = image::imageops::crop_imm(&sheet, c as u32 * cw, r as u32 * ch, cw, ch)
                    .to_image();
                let w = glyph_width(&g);
                glyphs.entry(chr).or_insert((g, w));
            }
        }
        Ok(McFont {
            glyphs,
            height: ch,
            space: (cw / 2).max(2),
        })
    }

    fn measure(&self, text: &str, scale: u32) -> u32 {
        let mut w = 0u32;
        for c in text.chars() {
            let gw = self.glyphs.get(&c).map(|g| g.1).unwrap_or(self.space);
            w += (gw + 1) * scale;
        }
        w
    }

    fn draw(
        &self,
        canvas: &mut RgbaImage,
        x: i64,
        y: i64,
        text: &str,
        color: [u8; 3],
        scale: u32,
        shadow: bool,
    ) -> u32 {
        let mut cx = x;
        let shadow_col = [
            (color[0] as f32 * 0.25) as u8,
            (color[1] as f32 * 0.25) as u8,
            (color[2] as f32 * 0.25) as u8,
        ];
        for c in text.chars() {
            let Some((glyph, w)) = self.glyphs.get(&c) else {
                cx += (self.space as i64 + 1) * scale as i64;
                continue;
            };
            for gy in 0..glyph.height() {
                for gx in 0..*w {
                    if glyph.get_pixel(gx, gy)[3] == 0 {
                        continue;
                    }
                    let px = cx + gx as i64 * scale as i64;
                    let py = y + gy as i64 * scale as i64;
                    if shadow {
                        fill_block(canvas, px + scale as i64, py + scale as i64, scale, shadow_col);
                    }
                    fill_block(canvas, px, py, scale, color);
                }
            }
            cx += (*w as i64 + 1) * scale as i64;
        }
        (cx - x).max(0) as u32
    }
}

fn glyph_width(g: &RgbaImage) -> u32 {
    let (w, h) = g.dimensions();
    for x in (0..w).rev() {
        for y in 0..h {
            if g.get_pixel(x, y)[3] > 0 {
                return x + 1;
            }
        }
    }
    w / 3
}

fn fill_block(canvas: &mut RgbaImage, x: i64, y: i64, size: u32, color: [u8; 3]) {
    let (w, h) = canvas.dimensions();
    for dy in 0..size as i64 {
        for dx in 0..size as i64 {
            let (px, py) = (x + dx, y + dy);
            if px >= 0 && py >= 0 && (px as u32) < w && (py as u32) < h {
                canvas.put_pixel(px as u32, py as u32, Rgba([color[0], color[1], color[2], 255]));
            }
        }
    }
}

fn blit(canvas: &mut RgbaImage, src: &RgbaImage, x: u32, y: u32) {
    let (cw, chh) = canvas.dimensions();
    for sy in 0..src.height() {
        for sx in 0..src.width() {
            let p = src.get_pixel(sx, sy);
            if p[3] == 0 {
                continue;
            }
            let (px, py) = (x + sx, y + sy);
            if px < cw && py < chh {
                let dst = canvas.get_pixel(px, py);
                let a = p[3] as f32 / 255.0;
                let mix = |s: u8, d: u8| ((s as f32 * a) + (d as f32 * (1.0 - a))) as u8;
                canvas.put_pixel(
                    px,
                    py,
                    Rgba([mix(p[0], dst[0]), mix(p[1], dst[1]), mix(p[2], dst[2]), 255]),
                );
            }
        }
    }
}

fn blit_add(canvas: &mut RgbaImage, src: &RgbaImage, x: u32, y: u32) {
    let (cw, chh) = canvas.dimensions();
    for sy in 0..src.height() {
        for sx in 0..src.width() {
            let s = src.get_pixel(sx, sy);
            let (px, py) = (x + sx, y + sy);
            if px < cw && py < chh {
                let d = canvas.get_pixel(px, py);
                let add = |a: u8, b: u8| (a as u16 + b as u16).min(255) as u8;
                canvas.put_pixel(
                    px,
                    py,
                    Rgba([add(d[0], s[0]), add(d[1], s[1]), add(d[2], s[2]), 255]),
                );
            }
        }
    }
}

fn draw_rect_px(canvas: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, color: [u8; 3]) {
    let (cw, ch) = canvas.dimensions();
    for dy in 0..h {
        for dx in 0..w {
            let (px, py) = (x + dx, y + dy);
            if px < cw && py < ch {
                canvas.put_pixel(px, py, Rgba([color[0], color[1], color[2], 255]));
            }
        }
    }
}

fn draw_missing(canvas: &mut RgbaImage, x: u32, y: u32, size: u32) {
    let (cw, ch) = canvas.dimensions();
    let cell = (size / 4).max(1);
    for dy in 0..size {
        for dx in 0..size {
            let (px, py) = (x + dx, y + dy);
            if px < cw && py < ch {
                let checker = ((dx / cell) + (dy / cell)) % 2 == 0;
                let c = if checker { [240, 0, 240] } else { [30, 30, 30] };
                canvas.put_pixel(px, py, Rgba([c[0], c[1], c[2], 255]));
            }
        }
    }
}

fn draw_bar(canvas: &mut RgbaImage, x: u32, y: u32, size: u32, bar: &crate::model::Bar) {
    let margin = (size as f32 * 0.12) as u32;
    let h = ((size as f32 * 0.06) as u32).clamp(2, 3);
    let full = size.saturating_sub(2 * margin);
    let by = y + size - h - (size as f32 * 0.08) as u32;
    let bx = x + margin;
    draw_rect_px(canvas, bx, by, full, h, [0, 0, 0]);
    let fw = (full as f32 * bar.frac.clamp(0.0, 1.0)) as u32;
    draw_rect_px(canvas, bx, by, fw, h.saturating_sub(1).max(1), bar.color);
}

fn draw_outline(canvas: &mut RgbaImage, x: u32, y: u32, size: u32, color: [u8; 3]) {
    let (cw, ch) = canvas.dimensions();
    for i in 0..size {
        for &(px, py) in &[(x + i, y), (x + i, y + size - 1), (x, y + i), (x + size - 1, y + i)] {
            if px < cw && py < ch {
                canvas.put_pixel(px, py, Rgba([color[0], color[1], color[2], 255]));
            }
        }
    }
}

fn draw_slot(canvas: &mut RgbaImage, x: u32, y: u32) {
    for dy in 0..SLOT {
        for dx in 0..SLOT {
            canvas.put_pixel(x + dx, y + dy, Rgba([139, 139, 139, 255]));
        }
    }
    for i in 0..SLOT {
        canvas.put_pixel(x + i, y, Rgba([55, 55, 55, 255]));
        canvas.put_pixel(x, y + i, Rgba([55, 55, 55, 255]));
        canvas.put_pixel(x + i, y + SLOT - 1, Rgba([255, 255, 255, 255]));
        canvas.put_pixel(x + SLOT - 1, y + i, Rgba([255, 255, 255, 255]));
    }
}

pub fn render_entry(entry: &Entry, atlas: &Atlas, font: &McFont) -> RgbaImage {
    let width = PAD * 2 + COLS * SLOT;
    let text_scale = 2u32;
    let line_h = font.height * text_scale + 8;
    let header_h = HEADER_ICON.max(entry.meta.len() as u32 * line_h) + PAD * 2;
    let grid_h = entry.rows as u32 * SLOT;
    let height = header_h + grid_h + PAD * 2;

    let mut canvas = RgbaImage::from_pixel(width, height, Rgba([198, 198, 198, 255]));

    if let Some(sprite) = atlas.sprite_scaled(&entry.header_icon, HEADER_ICON) {
        blit(&mut canvas, &sprite, PAD, PAD);
    }
    let text_x = (PAD + HEADER_ICON + 16) as i64;
    for (i, (label, value)) in entry.meta.iter().enumerate() {
        let y = PAD as i64 + i as i64 * line_h as i64;
        font.draw(
            &mut canvas,
            text_x,
            y,
            &format!("{label}: {value}"),
            [40, 40, 40],
            text_scale,
            false,
        );
    }

    let gx = PAD;
    let gy = header_h + PAD;
    let mut by_slot: HashMap<i32, &crate::model::Item> = HashMap::new();
    for it in &entry.items {
        by_slot.insert(it.slot, it);
    }
    for row in 0..entry.rows as u32 {
        for col in 0..COLS {
            let x = gx + col * SLOT;
            let y = gy + row * SLOT;
            draw_slot(&mut canvas, x, y);
            let idx = (row * COLS + col) as i32;
            if let Some(item) = by_slot.get(&idx) {
                let off = (SLOT - ICON) / 2;
                if let Some(sprite) = atlas.sprite_scaled(&item.id, ICON) {
                    blit(&mut canvas, &sprite, x + off, y + off);
                } else {
                    draw_missing(&mut canvas, x + off, y + off, ICON);
                }
                if !item.enchants.is_empty() {
                    if let Some(glint) = atlas.glint_overlay(&item.id, ICON) {
                        blit_add(&mut canvas, &glint, x + off, y + off);
                    }
                }
                if let Some(bar) = &item.bar {
                    draw_bar(&mut canvas, x + off, y + off, ICON, bar);
                }
                if let Some(oc) = item.outline {
                    draw_outline(&mut canvas, x, y, SLOT, oc);
                }
                if item.count > 1 {
                    let s = format_count(item.count);
                    let tw = font.measure(&s, text_scale);
                    font.draw(
                        &mut canvas,
                        (x + SLOT) as i64 - tw as i64 - 3,
                        (y + SLOT) as i64 - (font.height * text_scale) as i64 - 3,
                        &s,
                        [255, 255, 255],
                        text_scale,
                        true,
                    );
                }
            }
        }
    }

    canvas
}
