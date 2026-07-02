
use image::RgbaImage;

const YAW: f32 = 225.0 * std::f32::consts::PI / 180.0;
const PITCH: f32 = 30.0 * std::f32::consts::PI / 180.0;

const S: f32 = 4.0;
const HAT: f32 = S * 1.08;

struct Face {
    normal: [f32; 3],
    corner: [f32; 3],
    du: [f32; 3],
    dv: [f32; 3],
    tx: u32,
    ty: u32,
    shade: f32,
}

fn faces(s: f32, hat: bool) -> Vec<Face> {
    let o = if hat { 32 } else { 0 };
    let top_x = if hat { 40 } else { 8 };
    let bot_x = if hat { 48 } else { 16 };
    vec![
        Face {
            normal: [0.0, 1.0, 0.0],
            corner: [s, s, s],
            du: [-2.0 * s, 0.0, 0.0],
            dv: [0.0, 0.0, -2.0 * s],
            tx: top_x,
            ty: 0,
            shade: 1.0,
        },
        Face {
            normal: [0.0, 0.0, -1.0],
            corner: [s, s, -s],
            du: [-2.0 * s, 0.0, 0.0],
            dv: [0.0, -2.0 * s, 0.0],
            tx: 8 + o,
            ty: 8,
            shade: 0.86,
        },
        Face {
            normal: [1.0, 0.0, 0.0],
            corner: [s, s, s],
            du: [0.0, 0.0, -2.0 * s],
            dv: [0.0, -2.0 * s, 0.0],
            tx: o,
            ty: 8,
            shade: 0.70,
        },
        Face {
            normal: [-1.0, 0.0, 0.0],
            corner: [-s, s, -s],
            du: [0.0, 0.0, 2.0 * s],
            dv: [0.0, -2.0 * s, 0.0],
            tx: 16 + o,
            ty: 8,
            shade: 0.70,
        },
        Face {
            normal: [0.0, 0.0, 1.0],
            corner: [-s, s, s],
            du: [2.0 * s, 0.0, 0.0],
            dv: [0.0, -2.0 * s, 0.0],
            tx: 24 + o,
            ty: 8,
            shade: 0.86,
        },
        Face {
            normal: [0.0, -1.0, 0.0],
            corner: [-s, -s, -s],
            du: [2.0 * s, 0.0, 0.0],
            dv: [0.0, 0.0, 2.0 * s],
            tx: bot_x,
            ty: 0,
            shade: 0.55,
        },
    ]
}

fn rotate(p: [f32; 3]) -> [f32; 3] {
    let (sy, cy) = YAW.sin_cos();
    let (sp, cp) = PITCH.sin_cos();
    let x1 = p[0] * cy + p[2] * sy;
    let y1 = p[1];
    let z1 = -p[0] * sy + p[2] * cy;
    let x2 = x1;
    let y2 = y1 * cp - z1 * sp;
    let z2 = y1 * sp + z1 * cp;
    [x2, y2, z2]
}

pub fn render(skin: &RgbaImage, size: u32) -> Option<RgbaImage> {
    if skin.width() < 64 || skin.height() < 32 || size == 0 {
        return None;
    }
    let mut out = RgbaImage::new(size, size);
    let draw_hat = hat_visible(skin);

    let margin = size as f32 * 0.06;
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for &sx in &[-HAT, HAT] {
        for &sy in &[-HAT, HAT] {
            for &sz in &[-HAT, HAT] {
                let r = rotate([sx, sy, sz]);
                min_x = min_x.min(r[0]);
                max_x = max_x.max(r[0]);
                min_y = min_y.min(-r[1]);
                max_y = max_y.max(-r[1]);
            }
        }
    }
    let span = (max_x - min_x).max(max_y - min_y);
    let scale = (size as f32 - 2.0 * margin) / span;
    let cx = size as f32 / 2.0 - (min_x + max_x) / 2.0 * scale;
    let cy = size as f32 / 2.0 - (min_y + max_y) / 2.0 * scale;

    let mut all: Vec<(Face, bool)> = Vec::new();
    for f in faces(S, false) {
        all.push((f, false));
    }
    if draw_hat {
        for f in faces(HAT, true) {
            all.push((f, true));
        }
    }
    let mut vis: Vec<(Face, bool, f32)> = all
        .into_iter()
        .filter_map(|(f, is_hat)| {
            let n = rotate(f.normal);
            if n[2] <= 0.0 {
                return None;
            }
            let mid = [
                f.corner[0] + (f.du[0] + f.dv[0]) * 0.5,
                f.corner[1] + (f.du[1] + f.dv[1]) * 0.5,
                f.corner[2] + (f.du[2] + f.dv[2]) * 0.5,
            ];
            Some((f, is_hat, rotate(mid)[2]))
        })
        .collect();
    vis.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));

    let n = (scale * 2.0 * S / 8.0).ceil().max(1.0) as u32 * 3;
    for (f, is_hat, _) in &vis {
        for iu in 0..(8 * n) {
            for iv in 0..(8 * n) {
                let u = (iu as f32 + 0.5) / (8 * n) as f32;
                let v = (iv as f32 + 0.5) / (8 * n) as f32;
                let tex_u = (iu / n).min(7);
                let tex_v = (iv / n).min(7);
                let px = skin.get_pixel(f.tx + tex_u, f.ty + tex_v);
                let a = px[3];
                if *is_hat && a == 0 {
                    continue;
                }
                if a == 0 {
                    continue;
                }
                let p = [
                    f.corner[0] + f.du[0] * u + f.dv[0] * v,
                    f.corner[1] + f.du[1] * u + f.dv[1] * v,
                    f.corner[2] + f.du[2] * u + f.dv[2] * v,
                ];
                let r = rotate(p);
                let sxp = (r[0] * scale + cx).round();
                let syp = (-r[1] * scale + cy).round();
                if sxp < 0.0 || syp < 0.0 {
                    continue;
                }
                let (bx, by) = (sxp as u32, syp as u32);
                let col = image::Rgba([
                    (px[0] as f32 * f.shade) as u8,
                    (px[1] as f32 * f.shade) as u8,
                    (px[2] as f32 * f.shade) as u8,
                    a,
                ]);
                for dx in 0..2 {
                    for dy in 0..2 {
                        let (x, y) = (bx + dx, by + dy);
                        if x < size && y < size {
                            out.put_pixel(x, y, col);
                        }
                    }
                }
            }
        }
    }
    Some(out)
}

fn hat_visible(skin: &RgbaImage) -> bool {
    if skin.height() >= 64 {
        return true;
    }
    for f in faces(HAT, true) {
        for j in 0..8 {
            for i in 0..8 {
                if skin.get_pixel(f.tx + i, f.ty + j)[3] < 255 {
                    return true;
                }
            }
        }
    }
    false
}

pub fn render_from_bytes(bytes: &[u8], size: u32) -> Option<RgbaImage> {
    let skin = image::load_from_memory(bytes).ok()?.to_rgba8();
    render(&skin, size)
}
