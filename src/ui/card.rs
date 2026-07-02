use eframe::egui::{
    self, Align, Color32, FontId, Id, Layout, Pos2, Rect, Rounding, Sense, Stroke, Vec2,
};

use super::{Action, ACCENT, GOLD};
use crate::model::{format_count, prettify_id, Entry, EntryKind, Item};
use crate::profiles::Profiles;
use crate::render::atlas::Atlas;

type BpIndex = std::collections::HashMap<String, Vec<Item>>;

fn resolve_nested<'a>(item: &'a Item, bp: &'a BpIndex) -> &'a [Item] {
    if !item.contents.is_empty() {
        &item.contents
    } else if let Some(u) = &item.storage_uuid {
        bp.get(u).map(|v| v.as_slice()).unwrap_or(&[])
    } else {
        &[]
    }
}

fn has_openable(item: &Item, bp: &BpIndex) -> bool {
    !resolve_nested(item, bp).is_empty()
}

pub(crate) fn card_height(e: &Entry, slot: f32) -> f32 {
    let header = 56.0_f32.max(26.0 + e.meta.len() as f32 * 16.0);
    let upgrades = if e.upgrades.is_empty() { 0.0 } else { 4.0 + slot };
    12.0 + header + 8.0 + upgrades + 6.0 + e.rows.max(1) as f32 * slot + 12.0 + 6.0
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_card(
    ui: &mut egui::Ui,
    atlas: &mut Atlas,
    entry: &Entry,
    si: usize,
    ei: usize,
    slot: f32,
    gframe: usize,
    bp: &BpIndex,
    profiles: &mut Profiles,
    actions: &mut Vec<Action>,
    animating: &mut bool,
) {
    egui::Frame::none()
        .fill(Color32::from_rgb(33, 35, 43))
        .stroke(Stroke::new(1.0, Color32::from_rgb(52, 55, 66)))
        .rounding(Rounding::same(10.0))
        .inner_margin(egui::Margin::same(12.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(48.0), Sense::hover());
                paint_slot_bg(ui, rect, false);
                let drew_head = entry.kind == EntryKind::Player
                    && !entry.uuid.is_empty()
                    && profiles
                        .head(ui.ctx(), &entry.uuid)
                        .map(|tex| {
                            ui.painter().image(tex.id(), rect.shrink(2.0), full_uv(), Color32::WHITE);
                        })
                        .is_some();
                if !drew_head {
                    paint_texture(ui, atlas, rect.shrink(4.0), &entry.header_icon);
                }
                if entry.kind == EntryKind::Backpack
                    && !entry.uuid.is_empty()
                    && (entry.owner.is_empty() || entry.owner == "unknown")
                {
                    profiles.request(&entry.uuid);
                }

                ui.vertical(|ui| {
                    let title_col = match entry.kind {
                        EntryKind::Backpack => Color32::from_rgb(190, 150, 230),
                        EntryKind::Player => Color32::from_rgb(110, 190, 240),
                        EntryKind::Container => GOLD,
                    };
                    ui.label(egui::RichText::new(&entry.title).color(title_col).strong());
                    for (label, value) in &entry.meta {
                        ui.label(
                            egui::RichText::new(format!("{label}: {value}"))
                                .weak()
                                .size(12.0),
                        );
                    }
                });

                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    if ui.button("🖼 Export").on_hover_text("Save this as a PNG").clicked() {
                        actions.push(Action::Export(si, ei));
                    }
                    if ui.button("📋 Copy img").clicked() {
                        actions.push(Action::CopyImg(si, ei));
                    }
                    for c in &entry.copies {
                        if ui.button(&c.label).clicked() {
                            actions.push(Action::Copy(c.value.clone()));
                        }
                    }
                });
            });

            if !entry.upgrades.is_empty() {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Upgrades:").weak().size(12.0));
                    for (idx, up) in entry.upgrades.iter().enumerate() {
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(slot), Sense::hover());
                        paint_slot_bg(ui, rect, false);
                        if paint_icon(ui, atlas, profiles, rect.shrink(3.0), up, gframe) {
                            *animating = true;
                        }
                        let resp = ui.interact(rect, Id::new(("up", si, ei, idx)), Sense::hover());
                        if resp.hovered() {
                            paint_hover_ring(ui, rect);
                        }
                        let up = up.clone();
                        resp.on_hover_ui(|ui| item_tooltip(ui, atlas, &up, gframe, bp, profiles));
                    }
                });
            }

            ui.add_space(6.0);

            let cols = entry.cols.max(1);
            let rows = entry.rows.max(1);
            let (grid_rect, _) = ui.allocate_exact_size(
                Vec2::new(cols as f32 * slot, rows as f32 * slot),
                Sense::hover(),
            );
            let mut by_slot: std::collections::HashMap<i32, &Item> =
                std::collections::HashMap::new();
            for it in &entry.items {
                by_slot.insert(it.slot, it);
            }
            for r in 0..rows {
                for c in 0..cols {
                    let min = grid_rect.min + Vec2::new(c as f32 * slot, r as f32 * slot);
                    let srect = Rect::from_min_size(min, Vec2::splat(slot));
                    paint_slot_bg(ui, srect, true);
                    let sidx = (r * cols + c) as i32;
                    if let Some(item) = by_slot.get(&sidx) {
                        let icon = srect.shrink(slot * 0.1);
                        if paint_icon(ui, atlas, profiles, icon, item, gframe) {
                            *animating = true;
                        }
                        paint_count(ui, srect, item.count);
                        let openable = has_openable(item, bp);
                        if openable {
                            paint_nested_badge(ui, srect);
                        }
                        let sense = if openable { Sense::click() } else { Sense::hover() };
                        let resp = ui.interact(srect, Id::new(("slot", si, ei, sidx)), sense);
                        if resp.hovered() {
                            paint_hover_ring(ui, srect);
                        }
                        if openable && resp.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if openable && resp.clicked() {
                            let items = resolve_nested(item, bp).to_vec();
                            actions.push(Action::OpenNested(item.display_name(), items));
                        }
                        let item = (*item).clone();
                        resp.on_hover_ui(|ui| item_tooltip(ui, atlas, &item, gframe, bp, profiles));
                    }
                }
            }
        });
}

fn paint_slot_bg(ui: &egui::Ui, rect: Rect, filled: bool) {
    let painter = ui.painter();
    let bg = if filled {
        Color32::from_rgb(43, 45, 54)
    } else {
        Color32::from_rgb(38, 40, 48)
    };
    painter.rect_filled(rect, Rounding::same(3.0), bg);
    painter.rect_stroke(
        rect.shrink(0.5),
        Rounding::same(3.0),
        Stroke::new(1.0, Color32::from_rgb(24, 25, 30)),
    );
}

fn paint_hover_ring(ui: &egui::Ui, rect: Rect) {
    ui.painter()
        .rect_stroke(rect.shrink(1.0), Rounding::same(3.0), Stroke::new(1.5, ACCENT));
}

fn paint_nested_badge(ui: &egui::Ui, rect: Rect) {
    let s = (rect.width() * 0.26).clamp(6.0, 12.0);
    let min = rect.min + Vec2::new(2.0, 2.0);
    let br = Rect::from_min_size(min, Vec2::splat(s));
    let painter = ui.painter();
    painter.rect_filled(br, Rounding::same(2.0), Color32::from_rgba_unmultiplied(20, 24, 20, 190));
    painter.rect_stroke(br, Rounding::same(2.0), Stroke::new(1.0, ACCENT));
    let y = br.min.y + s * 0.34;
    painter.line_segment(
        [Pos2::new(br.min.x + 1.5, y), Pos2::new(br.max.x - 1.5, y)],
        Stroke::new(1.0, ACCENT),
    );
}

fn full_uv() -> Rect {
    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0))
}

fn paint_texture(ui: &egui::Ui, atlas: &mut Atlas, rect: Rect, id: &str) {
    if let Some(tex) = atlas.texture(ui.ctx(), id) {
        ui.painter().image(tex.id(), rect, full_uv(), Color32::WHITE);
    }
}

fn paint_icon(
    ui: &egui::Ui,
    atlas: &mut Atlas,
    profiles: &mut Profiles,
    rect: Rect,
    item: &Item,
    gframe: usize,
) -> bool {
    if let Some(key) = item.head_key() {
        if let Some(tex) = profiles.head(ui.ctx(), key) {
            ui.painter().image(tex.id(), rect.expand(rect.width() * 0.06), full_uv(), Color32::WHITE);
            return false;
        }
    }
    paint_texture(ui, atlas, rect, &item.id);
    !item.enchants.is_empty() && paint_glint(ui, atlas, rect, &item.id, gframe)
}

fn paint_glint(ui: &egui::Ui, atlas: &mut Atlas, rect: Rect, id: &str, gframe: usize) -> bool {
    if let Some(g) = atlas.glint_frame(ui.ctx(), id, gframe) {
        ui.painter().image(g.id(), rect, full_uv(), Color32::WHITE);
        true
    } else {
        false
    }
}

fn paint_count(ui: &egui::Ui, rect: Rect, count: i64) {
    if count <= 1 {
        return;
    }
    let s = format_count(count);
    let font = FontId::proportional((rect.height() * 0.34).max(10.0));
    let pos = rect.max - Vec2::new(3.0, 2.0);
    let painter = ui.painter();
    painter.text(
        pos + Vec2::new(1.0, 1.0),
        egui::Align2::RIGHT_BOTTOM,
        &s,
        font.clone(),
        Color32::from_black_alpha(200),
    );
    painter.text(pos, egui::Align2::RIGHT_BOTTOM, &s, font, Color32::WHITE);
}

fn item_tooltip(
    ui: &mut egui::Ui,
    atlas: &mut Atlas,
    item: &Item,
    gframe: usize,
    bp: &BpIndex,
    profiles: &mut Profiles,
) {
    ui.set_max_width(340.0);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::hover());
        paint_icon(ui, atlas, profiles, rect, item, gframe);
        ui.vertical(|ui| {
            let name_col = if item.custom_name.is_some() {
                Color32::from_rgb(255, 240, 160)
            } else {
                Color32::WHITE
            };
            let mut name = egui::RichText::new(item.display_name()).color(name_col).strong();
            if item.custom_name.is_some() {
                name = name.italics();
            }
            ui.label(name);
            ui.label(egui::RichText::new(item.id.clone()).weak().size(11.0).monospace());
        });
    });

    if let Some(dmg) = item.damage {
        let dur = match item.max_damage {
            Some(max) if max > 0 => format!("Durability: {} / {} used", dmg, max),
            _ => format!("Damage: {dmg}"),
        };
        ui.label(egui::RichText::new(dur).color(Color32::from_rgb(150, 200, 150)).size(12.0));
    }
    if let Some(p) = &item.potion {
        ui.label(egui::RichText::new(format!("Potion: {p}")).color(Color32::from_rgb(200, 120, 220)));
    }

    if !item.enchants.is_empty() {
        ui.add_space(2.0);
        for (id, lvl) in &item.enchants {
            let name = format!("{} {}", prettify_id(id), roman(*lvl));
            ui.label(egui::RichText::new(name).color(enchant_color(*lvl)));
        }
    }

    if !item.lore.is_empty() {
        ui.add_space(2.0);
        for line in &item.lore {
            ui.label(
                egui::RichText::new(line)
                    .italics()
                    .color(Color32::from_rgb(160, 130, 210))
                    .size(12.0),
            );
        }
    }

    let contents = resolve_nested(item, bp);
    if !contents.is_empty() {
        ui.add_space(4.0);
        ui.separator();
        let heading = if item.storage_uuid.is_some() && item.contents.is_empty() {
            format!("Backpack contents ({})", contents.len())
        } else {
            format!("Contents ({})", contents.len())
        };
        ui.label(egui::RichText::new(heading).color(ACCENT).strong());
        let cell = 26.0;
        let cols = 9usize;
        let shown = contents.len().min(54);
        let rows = shown.div_ceil(cols);
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(cols as f32 * cell, rows as f32 * cell), Sense::hover());
        for (i, c) in contents.iter().take(shown).enumerate() {
            let min = rect.min + Vec2::new((i % cols) as f32 * cell, (i / cols) as f32 * cell);
            let srect = Rect::from_min_size(min, Vec2::splat(cell));
            paint_slot_bg(ui, srect, true);
            paint_icon(ui, atlas, profiles, srect.shrink(2.0), c, gframe);
            if has_openable(c, bp) {
                paint_nested_badge(ui, srect);
            }
            paint_count(ui, srect, c.count);
        }
        for c in contents.iter().take(12) {
            ui.label(
                egui::RichText::new(format!("• {}× {}", c.count, c.display_name()))
                    .size(11.0)
                    .weak(),
            );
        }
        if contents.len() > 12 {
            ui.label(
                egui::RichText::new(format!("… +{} more · click to open", contents.len() - 12))
                    .size(11.0)
                    .color(ACCENT),
            );
        } else {
            ui.label(egui::RichText::new("Click slot to open").size(11.0).color(ACCENT));
        }
    } else if let Some(u) = &item.storage_uuid {
        ui.add_space(4.0);
        ui.separator();
        ui.label(
            egui::RichText::new("Backpack - load the backpacks .dat to view contents")
                .size(11.0)
                .color(GOLD),
        );
        ui.label(egui::RichText::new(u.clone()).size(10.0).weak().monospace());
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn nested_grid(
    ui: &mut egui::Ui,
    atlas: &mut Atlas,
    items: &[Item],
    bp: &BpIndex,
    profiles: &mut Profiles,
    gframe: usize,
    slot: f32,
    animating: &mut bool,
) -> Option<(String, Vec<Item>)> {
    let cols = 9usize;
    let rows = items.len().max(1).div_ceil(cols);
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(cols as f32 * slot, rows as f32 * slot),
        Sense::hover(),
    );
    let mut drill = None;
    for (i, it) in items.iter().enumerate() {
        let min = rect.min + Vec2::new((i % cols) as f32 * slot, (i / cols) as f32 * slot);
        let srect = Rect::from_min_size(min, Vec2::splat(slot));
        paint_slot_bg(ui, srect, true);
        if paint_icon(ui, atlas, profiles, srect.shrink(slot * 0.1), it, gframe) {
            *animating = true;
        }
        paint_count(ui, srect, it.count);
        let openable = has_openable(it, bp);
        if openable {
            paint_nested_badge(ui, srect);
        }
        let sense = if openable { Sense::click() } else { Sense::hover() };
        let resp = ui.interact(srect, Id::new(("nested-slot", i)), sense);
        if resp.hovered() {
            paint_hover_ring(ui, srect);
        }
        if openable && resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if openable && resp.clicked() {
            drill = Some((it.display_name(), resolve_nested(it, bp).to_vec()));
        }
        let it2 = it.clone();
        resp.on_hover_ui(|ui| item_tooltip(ui, atlas, &it2, gframe, bp, profiles));
    }
    drill
}

fn enchant_color(level: i32) -> Color32 {
    if level > 10 {
        Color32::from_rgb(240, 90, 80)
    } else if level > 5 {
        GOLD
    } else {
        Color32::from_rgb(170, 170, 235)
    }
}

fn roman(n: i32) -> String {
    if !(1..=10).contains(&n) {
        return n.to_string();
    }
    ["I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX", "X"][(n - 1) as usize].to_string()
}
