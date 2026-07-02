
use std::collections::HashMap;

use egui::{Color32, ColorImage, TextureHandle, TextureOptions};
use serde::Deserialize;

#[derive(Deserialize, Clone, Copy)]
struct Rect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[derive(Deserialize)]
struct AtlasMap {
    sprites: HashMap<String, Rect>,
}

pub fn assets_dir() -> String {
    let mut candidates = vec![std::path::PathBuf::from("assets")];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("assets"));
            candidates.push(dir.join("../Resources/assets"));
        }
    }
    for c in &candidates {
        if c.join("atlas_map.json").exists() {
            return c.to_string_lossy().into_owned();
        }
    }
    "assets".to_string()
}

pub const GLINT_FRAMES: usize = 24;
const GLINT_SIZE: u32 = 64;
const GLINT_C1: [f32; 3] = [122.0, 92.0, 255.0];
const GLINT_C2: [f32; 3] = [169.0, 113.0, 255.0];
const GLINT_STRENGTH: f32 = 0.5;

pub struct Atlas {
    map: HashMap<String, Rect>,
    image: image::RgbaImage,
    cache: HashMap<String, Option<TextureHandle>>,
    glint_src: Option<image::RgbaImage>,
    glint_cache: HashMap<String, Option<Vec<TextureHandle>>>,
}

impl Atlas {
    pub fn load(assets_dir: &str) -> Result<Atlas, String> {
        let json = std::fs::read_to_string(format!("{assets_dir}/atlas_map.json"))
            .map_err(|e| format!("atlas_map.json: {e}"))?;
        let map: AtlasMap = serde_json::from_str(&json).map_err(|e| format!("atlas_map: {e}"))?;

        let mut reader = image::ImageReader::open(format!("{assets_dir}/item_atlas.png"))
            .map_err(|e| format!("item_atlas.png: {e}"))?;
        reader.no_limits();
        let img = reader
            .decode()
            .map_err(|e| format!("item_atlas.png decode: {e}"))?
            .to_rgba8();

        let glint_src = image::open(format!("{assets_dir}/enchanted_glint_item.png"))
            .ok()
            .map(|i| i.to_rgba8());

        Ok(Atlas {
            map: map.sprites,
            image: img,
            cache: HashMap::new(),
            glint_src,
            glint_cache: HashMap::new(),
        })
    }

    fn crop_sprite(&self, r: Rect) -> image::RgbaImage {
        image::imageops::crop_imm(&self.image, r.x, r.y, r.width, r.height).to_image()
    }

    fn resolve(&self, id: &str) -> Option<Rect> {
        let clean = id.trim().trim_matches('"').to_lowercase();
        if let Some(r) = self.map.get(&clean) {
            return Some(*r);
        }
        if !clean.contains(':') {
            if let Some(r) = self.map.get(&format!("minecraft:{clean}")) {
                return Some(*r);
            }
        }
        if let Some(stripped) = clean.strip_prefix("minecraft:") {
            if let Some(r) = self.map.get(stripped) {
                return Some(*r);
            }
        }
        for (needle, key) in [
            ("shulker", "shulker_box"),
            ("chest", "chest"),
            ("barrel", "barrel"),
            ("backpack", "sophisticatedbackpacks:backpack"),
        ] {
            if clean.contains(needle) {
                if let Some(r) = self.map.get(key).or_else(|| self.map.get(&format!("minecraft:{key}"))) {
                    return Some(*r);
                }
            }
        }
        None
    }

    pub fn texture(&mut self, ctx: &egui::Context, id: &str) -> Option<TextureHandle> {
        let key = id.trim().trim_matches('"').to_lowercase();
        if let Some(cached) = self.cache.get(&key) {
            return cached.clone();
        }
        let tex = self.resolve(&key).map(|r| {
            let cropped =
                image::imageops::crop_imm(&self.image, r.x, r.y, r.width, r.height).to_image();
            let color = ColorImage::from_rgba_unmultiplied(
                [r.width as usize, r.height as usize],
                cropped.as_raw(),
            );
            ctx.load_texture(format!("item::{key}"), color, TextureOptions::NEAREST)
        });
        self.cache.insert(key, tex.clone());
        tex
    }

    pub fn glint_frame(
        &mut self,
        ctx: &egui::Context,
        id: &str,
        frame: usize,
    ) -> Option<TextureHandle> {
        if self.glint_src.is_none() {
            return None;
        }
        let key = id.trim().trim_matches('"').to_lowercase();
        if !self.glint_cache.contains_key(&key) {
            let built = self.build_glint_frames(ctx, &key);
            self.glint_cache.insert(key.clone(), built);
        }
        self.glint_cache
            .get(&key)?
            .as_ref()?
            .get(frame % GLINT_FRAMES)
            .cloned()
    }

    fn build_glint_frames(&self, ctx: &egui::Context, key: &str) -> Option<Vec<TextureHandle>> {
        let glint = self.glint_src.as_ref()?;
        let r = self.resolve(key)?;
        let item = image::imageops::resize(
            &self.crop_sprite(r),
            GLINT_SIZE,
            GLINT_SIZE,
            image::imageops::FilterType::Nearest,
        );
        let mut frames = Vec::with_capacity(GLINT_FRAMES);
        for f in 0..GLINT_FRAMES {
            let prog = f as f32 / GLINT_FRAMES as f32;
            let pixels: Vec<Color32> = glint_added(&item, glint, prog)
                .into_iter()
                .map(|[r, g, b]| Color32::from_rgba_premultiplied(r, g, b, 0))
                .collect();
            let color = ColorImage {
                size: [GLINT_SIZE as usize, GLINT_SIZE as usize],
                pixels,
            };
            frames.push(ctx.load_texture(format!("glint::{key}::{f}"), color, TextureOptions::LINEAR));
        }
        Some(frames)
    }

    pub fn glint_overlay(&self, id: &str, size: u32) -> Option<image::RgbaImage> {
        let glint = self.glint_src.as_ref()?;
        let r = self.resolve(&id.trim().trim_matches('"').to_lowercase())?;
        let item = image::imageops::resize(
            &self.crop_sprite(r),
            size,
            size,
            image::imageops::FilterType::Nearest,
        );
        let mut img = image::RgbaImage::new(size, size);
        for (i, [r, g, b]) in glint_added(&item, glint, 0.15).into_iter().enumerate() {
            let (x, y) = (i as u32 % size, i as u32 / size);
            img.put_pixel(x, y, image::Rgba([r, g, b, 255]));
        }
        Some(img)
    }

    pub fn sprite_scaled(&self, id: &str, size: u32) -> Option<image::RgbaImage> {
        let r = self.resolve(&id.trim().trim_matches('"').to_lowercase())?;
        let cropped = image::imageops::crop_imm(&self.image, r.x, r.y, r.width, r.height).to_image();
        Some(image::imageops::resize(
            &cropped,
            size,
            size,
            image::imageops::FilterType::Nearest,
        ))
    }
}

fn glint_added(item: &image::RgbaImage, glint: &image::RgbaImage, prog: f32) -> Vec<[u8; 3]> {
    let size = item.width();
    let (gw, gh) = glint.dimensions();
    let mut out = vec![[0u8; 3]; (size * size) as usize];
    for y in 0..size {
        for x in 0..size {
            let ia = item.get_pixel(x, y)[3];
            if ia == 0 {
                continue;
            }
            let mask = ia as f32 / 255.0;
            let i1 = sample_glint(glint, x, y, size, gw, gh, prog, 1.0);
            let i2 = sample_glint(glint, x, y, size, gw, gh, 0.3 - prog * 0.8, 1.7);
            let total = (i1 + i2).min(1.0);
            let mix = i2 / (i1 + i2 + 1e-4);
            let s = total * GLINT_STRENGTH * mask;
            let idx = (y * size + x) as usize;
            out[idx] = [
                ((GLINT_C1[0] + (GLINT_C2[0] - GLINT_C1[0]) * mix) * s).min(255.0) as u8,
                ((GLINT_C1[1] + (GLINT_C2[1] - GLINT_C1[1]) * mix) * s).min(255.0) as u8,
                ((GLINT_C1[2] + (GLINT_C2[2] - GLINT_C1[2]) * mix) * s).min(255.0) as u8,
            ];
        }
    }
    out
}

fn sample_glint(
    glint: &image::RgbaImage,
    x: u32,
    y: u32,
    size: u32,
    gw: u32,
    gh: u32,
    scroll: f32,
    scale: f32,
) -> f32 {
    let fx = (x as f32 * scale / size as f32 + scroll) * gw as f32;
    let fy = (y as f32 * scale / size as f32 + scroll) * gh as f32;
    let gx = (fx.rem_euclid(gw as f32)) as u32 % gw;
    let gy = (fy.rem_euclid(gh as f32)) as u32 % gh;
    let p = glint.get_pixel(gx, gy);
    let lum = (p[0] as f32 + p[1] as f32 + p[2] as f32) / (3.0 * 255.0);
    lum * (p[3] as f32 / 255.0)
}
