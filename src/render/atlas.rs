
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::PathBuf;
use std::sync::Arc;

use egui::{Color32, ColorImage, TextureHandle, TextureOptions};

pub fn find_zip() -> Option<PathBuf> {
    let mut candidates = vec![
        PathBuf::from("brass_atlas.zip"),
        PathBuf::from("assets").join("brass_atlas.zip"),
    ];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("brass_atlas.zip"));
            candidates.push(dir.join("../Resources/brass_atlas.zip"));
        }
    }
    candidates.into_iter().find(|p| p.exists())
}

pub const GLINT_FRAMES: usize = 24;
const GLINT_SIZE: u32 = 64;
const GLINT_C1: [f32; 3] = [122.0, 92.0, 255.0];
const GLINT_C2: [f32; 3] = [169.0, 113.0, 255.0];
const GLINT_STRENGTH: f32 = 0.5;

#[derive(Clone, Copy)]
struct SpriteMeta {
    #[allow(dead_code)]
    w: u32,
    #[allow(dead_code)]
    h: u32,
    #[allow(dead_code)]
    is3d: bool,
}

type Zip = zip::ZipArchive<Cursor<Vec<u8>>>;

pub struct Atlas {
    manifest: HashMap<String, SpriteMeta>,
    zip: RefCell<Zip>,
    sprites: RefCell<HashMap<String, Option<Arc<image::RgbaImage>>>>,
    cache: HashMap<String, Option<TextureHandle>>,
    glint_src: Option<image::RgbaImage>,
    glint_cache: HashMap<String, Option<Vec<TextureHandle>>>,
}

impl Atlas {
    pub fn load(zip_path: &str) -> Result<Atlas, String> {
        let bytes = std::fs::read(zip_path).map_err(|e| format!("brass_atlas.zip: {e}"))?;
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("zip open: {e}"))?;

        let manifest = {
            let mut f = zip.by_name("atlas.nbt").map_err(|e| format!("atlas.nbt: {e}"))?;
            let mut raw = Vec::new();
            f.read_to_end(&mut raw).map_err(|e| format!("atlas.nbt read: {e}"))?;
            parse_manifest(&raw)?
        };

        let glint_src = crate::assets::get("enchanted_glint_item.png")
            .and_then(|b| image::load_from_memory(b).ok())
            .map(|i| i.to_rgba8());

        Ok(Atlas {
            manifest,
            zip: RefCell::new(zip),
            sprites: RefCell::new(HashMap::new()),
            cache: HashMap::new(),
            glint_src,
            glint_cache: HashMap::new(),
        })
    }

    fn resolve(&self, id: &str) -> Option<String> {
        let clean = id.trim().trim_matches('"').to_lowercase();
        if self.manifest.contains_key(&clean) {
            return Some(clean);
        }
        if !clean.contains(':') {
            let k = format!("minecraft:{clean}");
            if self.manifest.contains_key(&k) {
                return Some(k);
            }
        }
        if let Some(stripped) = clean.strip_prefix("minecraft:") {
            if self.manifest.contains_key(stripped) {
                return Some(stripped.to_string());
            }
        }
        for (needle, key) in [
            ("shulker", "shulker_box"),
            ("chest", "chest"),
            ("barrel", "barrel"),
            ("backpack", "sophisticatedbackpacks:backpack"),
        ] {
            if clean.contains(needle) {
                for cand in [key.to_string(), format!("minecraft:{key}")] {
                    if self.manifest.contains_key(&cand) {
                        return Some(cand);
                    }
                }
            }
        }
        None
    }

    fn sprite(&self, id: &str) -> Option<Arc<image::RgbaImage>> {
        let key = self.resolve(id)?;
        if let Some(cached) = self.sprites.borrow().get(&key) {
            return cached.clone();
        }
        let img = self.load_sprite(&key);
        self.sprites.borrow_mut().insert(key, img.clone());
        img
    }

    fn load_sprite(&self, key: &str) -> Option<Arc<image::RgbaImage>> {
        let name = format!("sprites/{}.png", key.replacen(':', "_", 1).replace('/', "_"));
        let mut zip = self.zip.borrow_mut();
        let mut file = zip.by_name(&name).ok()?;
        let mut buf = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut buf).ok()?;
        drop(file);
        let img = image::load_from_memory(&buf).ok()?.to_rgba8();
        Some(Arc::new(img))
    }

    pub fn texture(&mut self, ctx: &egui::Context, id: &str) -> Option<TextureHandle> {
        let key = id.trim().trim_matches('"').to_lowercase();
        if let Some(cached) = self.cache.get(&key) {
            return cached.clone();
        }
        let tex = self.sprite(&key).map(|img| {
            let color = ColorImage::from_rgba_unmultiplied(
                [img.width() as usize, img.height() as usize],
                img.as_raw(),
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
        let base = self.sprite(key)?;
        let item = image::imageops::resize(
            base.as_ref(),
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
        let base = self.sprite(id)?;
        let item = image::imageops::resize(
            base.as_ref(),
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
        let base = self.sprite(id)?;
        Some(image::imageops::resize(
            base.as_ref(),
            size,
            size,
            image::imageops::FilterType::Nearest,
        ))
    }
}

fn parse_manifest(raw: &[u8]) -> Result<HashMap<String, SpriteMeta>, String> {
    let bytes = decompress(raw);
    let root: fastnbt::Value =
        fastnbt::from_bytes(&bytes).map_err(|e| format!("manifest nbt: {e}"))?;
    let sprites = match &root {
        fastnbt::Value::Compound(m) => m.get("sprites"),
        _ => None,
    }
    .ok_or("manifest missing 'sprites'")?;
    let map = match sprites {
        fastnbt::Value::Compound(m) => m,
        _ => return Err("manifest 'sprites' not a compound".into()),
    };
    let mut out = HashMap::with_capacity(map.len());
    for (id, v) in map {
        if let fastnbt::Value::Compound(e) = v {
            let w = nbt_u32(e.get("w")).unwrap_or(16);
            let h = nbt_u32(e.get("h")).unwrap_or(w);
            let is3d = matches!(e.get("d"), Some(fastnbt::Value::Byte(b)) if *b != 0);
            out.insert(id.clone(), SpriteMeta { w, h, is3d });
        }
    }
    Ok(out)
}

fn nbt_u32(v: Option<&fastnbt::Value>) -> Option<u32> {
    match v? {
        fastnbt::Value::Byte(x) => Some(*x as u32),
        fastnbt::Value::Short(x) => Some(*x as u32),
        fastnbt::Value::Int(x) => Some(*x as u32),
        fastnbt::Value::Long(x) => Some(*x as u32),
        _ => None,
    }
}

fn decompress(raw: &[u8]) -> Vec<u8> {
    if raw.first() == Some(&0x1f) {
        let mut d = flate2::read::MultiGzDecoder::new(raw);
        let mut out = Vec::new();
        if d.read_to_end(&mut out).is_ok() {
            return out;
        }
    }
    if raw.first() == Some(&0x78) {
        let mut d = flate2::read::ZlibDecoder::new(raw);
        let mut out = Vec::new();
        if d.read_to_end(&mut out).is_ok() {
            return out;
        }
    }
    raw.to_vec()
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
