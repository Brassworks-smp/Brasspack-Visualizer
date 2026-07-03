use eframe::egui::{
    self, Color32, FontId, Id, Pos2, Rect, Rounding, Sense, Stroke, Vec2,
};

use super::{Action, ACCENT, GOLD};
use crate::model::{format_count, prettify_id, Bar, Entry, EntryKind, Item};
use crate::profiles::Profiles;
use crate::render::atlas::Atlas;
use crate::search::Highlight;

type BpIndex = std::collections::HashMap<String, Vec<Item>>;

// A drill-down request from a nested slot: (title, items, backpack uuid if any).
pub(crate) type Drill = (String, Vec<Item>, Option<String>);

const MATCH: Color32 = Color32::from_rgb(255, 206, 70);

// Does this item or anything nested inside it satisfy the search?
fn matches_deep(item: &Item, hl: &Highlight, bp: &BpIndex) -> bool {
    hl.item_matches(item) || resolve_nested(item, bp).iter().any(|c| matches_deep(c, hl, bp))
}

// 0 = no match, 1 = a nested descendant matches (container marker),
// 2 = the item itself matches (direct marker).
fn match_level(item: &Item, hl: Option<&Highlight>, bp: &BpIndex) -> u8 {
    let Some(hl) = hl else { return 0 };
    if hl.item_matches(item) {
        2
    } else if resolve_nested(item, bp).iter().any(|c| matches_deep(c, hl, bp)) {
        1
    } else {
        0
    }
}

// Gold marker for a slot that matched the search: a corner tab plus an outline.
// `strong` marks the item itself; otherwise it marks a container whose nested
// contents matched (drawn a touch dimmer).
fn paint_match(ui: &egui::Ui, rect: Rect, strong: bool) {
    let painter = ui.painter();
    let col = if strong { MATCH } else { MATCH.gamma_multiply(0.65) };
    // top-right corner tab (the nested badge lives top-left, count bottom-right)
    let s = (rect.width() * 0.3).clamp(6.0, 11.0);
    let tr = Pos2::new(rect.max.x - 1.5, rect.min.y + 1.5);
    painter.add(egui::Shape::convex_polygon(
        vec![tr, tr - Vec2::new(s, 0.0), tr + Vec2::new(0.0, s)],
        col,
        Stroke::NONE,
    ));
    painter.rect_stroke(rect.shrink(1.0), Rounding::same(3.0), Stroke::new(1.5, col));
}

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

pub(crate) fn card_height(m: &crate::store::EntryMeta, slot: f32) -> f32 {
    let header = 52.0_f32.max(24.0 + m.meta_len as f32 * 16.0);
    let actions = 34.0;
    let upgrades = if m.has_upgrades() { 12.0 + slot } else { 0.0 };
    let rows = (m.rows as usize).max(1) as f32;
    // frame margins (12*2) + header + gap + actions + gap + upgrades + gap + grid + pad
    24.0 + header + 8.0 + actions + 10.0 + upgrades + rows * slot + 14.0
}

// Fixed on-screen width of a card at the given zoom. Wide enough that the
// 9-column grid and the compact action toolbar never need to wrap.
pub(crate) fn card_width(slot: f32) -> f32 {
    (9.0 * slot + 24.0).max(320.0)
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
    hl: Option<&Highlight>,
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
            // Fill the full column width so cards sit flush with no gaps between
            // them, rather than shrinking to the 9-slot grid's content width.
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, hdr_resp) = ui.allocate_exact_size(Vec2::splat(48.0), Sense::hover());
                paint_slot_bg(ui, rect, false);
                let drew_head = entry.kind == EntryKind::Player
                    && !entry.uuid.is_empty()
                    && profiles
                        .head(ui.ctx(), &entry.uuid)
                        .map(|tex| {
                            ui.painter().image(tex.id(), rect.shrink(2.0), full_uv(), Color32::WHITE);
                        })
                        .is_some();
                if !drew_head && !paint_texture(ui, atlas, rect.shrink(4.0), &entry.header_icon) {
                    paint_missing(ui, rect.shrink(4.0));
                }
                if hdr_resp.hovered() {
                    paint_hover_ring(ui, rect);
                }
                hdr_resp.on_hover_ui(|ui| header_tooltip(ui, entry));
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
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&entry.title).color(title_col).strong(),
                        )
                        .truncate(),
                    );
                    for (label, value) in &entry.meta {
                        ui.label(
                            egui::RichText::new(format!("{label}: {value}"))
                                .weak()
                                .size(12.0),
                        );
                    }
                });
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("PNG").on_hover_text("Export this as a PNG").clicked() {
                    actions.push(Action::Export(si, ei));
                }
                if ui.button("Copy Img").on_hover_text("Copy image to clipboard").clicked() {
                    actions.push(Action::CopyImg(si, ei));
                }
                if !entry.copies.is_empty() {
                    ui.menu_button("Copy…", |ui| {
                        for c in &entry.copies {
                            if ui.button(&c.label).clicked() {
                                actions.push(Action::Copy(c.value.clone()));
                                ui.close_menu();
                            }
                        }
                    });
                }
            });

            if !entry.upgrades.is_empty() {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Upgrades:").weak().size(12.0));
                    let mut hovered: Option<Rect> = None;
                    let mut matches: Vec<Rect> = Vec::new();
                    for (idx, up) in entry.upgrades.iter().enumerate() {
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(slot), Sense::hover());
                        paint_slot(ui, atlas, profiles, rect, up, gframe, false, animating);
                        if hl.is_some_and(|h| h.item_matches(up)) {
                            matches.push(rect);
                        }
                        let resp = ui.interact(rect, Id::new(("up", si, ei, idx)), Sense::hover());
                        if resp.hovered() {
                            hovered = Some(rect);
                        }
                        let up = up.clone();
                        resp.on_hover_ui(|ui| item_tooltip(ui, atlas, &up, gframe, bp, hl, profiles));
                    }
                    for r in matches {
                        paint_match(ui, r, true);
                    }
                    if let Some(r) = hovered {
                        paint_hover_ring(ui, r);
                    }
                });
            }

            ui.add_space(6.0);

            let cols = entry.cols.max(1);
            let rows = entry.rows.max(1);
            let grid_w = cols as f32 * slot;
            // Center the fixed-size slot grid in the (wider) card.
            let indent = ((ui.available_width() - grid_w) * 0.5).max(0.0);
            let (row_rect, _) = ui.allocate_exact_size(
                Vec2::new((ui.available_width()).max(grid_w), rows as f32 * slot),
                Sense::hover(),
            );
            let grid_rect =
                Rect::from_min_size(row_rect.min + Vec2::new(indent, 0.0), Vec2::new(grid_w, rows as f32 * slot));
            let mut by_slot: std::collections::HashMap<i32, &Item> =
                std::collections::HashMap::new();
            for it in &entry.items {
                by_slot.insert(it.slot, it);
            }
            let mut hovered: Option<Rect> = None;
            let mut matches: Vec<(Rect, bool)> = Vec::new();
            for r in 0..rows {
                for c in 0..cols {
                    let min = grid_rect.min + Vec2::new(c as f32 * slot, r as f32 * slot);
                    let srect = Rect::from_min_size(min, Vec2::splat(slot));
                    let sidx = (r * cols + c) as i32;
                    if let Some(item) = by_slot.get(&sidx) {
                        let openable = has_openable(item, bp);
                        paint_slot(ui, atlas, profiles, srect, item, gframe, true, animating);
                        if openable {
                            paint_nested_badge(ui, srect);
                        }
                        match match_level(item, hl, bp) {
                            2 => matches.push((srect, true)),
                            1 => matches.push((srect, false)),
                            _ => {}
                        }
                        let sense = if openable { Sense::click() } else { Sense::hover() };
                        let resp = ui.interact(srect, Id::new(("slot", si, ei, sidx)), sense);
                        if resp.hovered() {
                            hovered = Some(srect);
                        }
                        if openable && resp.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if openable && resp.clicked() {
                            let items = resolve_nested(item, bp).to_vec();
                            actions.push(Action::OpenNested(
                                item.display_name(),
                                items,
                                item.storage_uuid.clone(),
                            ));
                        }
                        let item = (*item).clone();
                        resp.on_hover_ui(|ui| item_tooltip(ui, atlas, &item, gframe, bp, hl, profiles));
                    } else {
                        paint_slot_bg(ui, srect, true);
                    }
                }
            }
            for (r, strong) in matches {
                paint_match(ui, r, strong);
            }
            if let Some(r) = hovered {
                paint_hover_ring(ui, r);
            }
        });
}

// Draw a filled slot: background, item icon, durability/tank bar, special
// outline, and stack count. The hover ring is drawn separately, on top of the
// whole grid, so it never gets clipped by a neighbouring slot.
#[allow(clippy::too_many_arguments)]
fn paint_slot(
    ui: &egui::Ui,
    atlas: &mut Atlas,
    profiles: &mut Profiles,
    srect: Rect,
    item: &Item,
    gframe: usize,
    filled: bool,
    animating: &mut bool,
) {
    paint_slot_bg(ui, srect, filled);
    let icon = srect.shrink(srect.width() * 0.1);
    if paint_icon(ui, atlas, profiles, icon, item, gframe) {
        *animating = true;
    }
    if let Some(bar) = &item.bar {
        paint_bar(ui, srect, bar);
    }
    if let Some(col) = item.outline {
        paint_outline(ui, srect, Color32::from_rgb(col[0], col[1], col[2]));
    }
    paint_count(ui, srect, item.count);
}

fn header_tooltip(ui: &mut egui::Ui, entry: &Entry) {
    ui.set_max_width(320.0);
    let (kind, col) = match entry.kind {
        EntryKind::Backpack => ("Backpack", Color32::from_rgb(190, 150, 230)),
        EntryKind::Player => ("Player", Color32::from_rgb(110, 190, 240)),
        EntryKind::Container => ("Container", GOLD),
    };
    ui.label(egui::RichText::new(kind).color(col).strong());
    ui.label(egui::RichText::new(prettify_id(&entry.header_icon)).size(12.0));
    ui.label(egui::RichText::new(&entry.header_icon).weak().size(11.0).monospace());
    for (label, value) in &entry.meta {
        ui.label(egui::RichText::new(format!("{label}: {value}")).weak().size(11.0));
    }
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
    // Drawn on top of the whole grid and inset so it stays fully inside the
    // slot instead of clipping into neighbouring cells.
    ui.painter()
        .rect_stroke(rect.shrink(1.5), Rounding::same(3.0), Stroke::new(2.0, ACCENT));
}

// Minecraft-style durability / tank fill bar along the bottom of a slot.
fn paint_bar(ui: &egui::Ui, rect: Rect, bar: &Bar) {
    let m = rect.width() * 0.12;
    let h = (rect.height() * 0.055).clamp(2.0, 3.0);
    let y = rect.bottom() - rect.height() * 0.16;
    let left = rect.left() + m;
    let full = rect.width() - 2.0 * m;
    let painter = ui.painter();
    // dark backing (with a 1px shadow row beneath like vanilla)
    painter.rect_filled(
        Rect::from_min_size(Pos2::new(left, y), Vec2::new(full, h)),
        Rounding::ZERO,
        Color32::from_rgb(0, 0, 0),
    );
    let fw = (full * bar.frac.clamp(0.0, 1.0)).max(0.0);
    let c = bar.color;
    painter.rect_filled(
        Rect::from_min_size(Pos2::new(left, y), Vec2::new(fw, (h - 1.0).max(1.0))),
        Rounding::ZERO,
        Color32::from_rgb(c[0], c[1], c[2]),
    );
}

// Special slot outline (e.g. white for Create backtanks). Inset so it reads as
// a highlight without bleeding into adjacent slots.
fn paint_outline(ui: &egui::Ui, rect: Rect, color: Color32) {
    ui.painter()
        .rect_stroke(rect.shrink(1.5), Rounding::same(3.0), Stroke::new(2.0, color));
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

fn paint_texture(ui: &egui::Ui, atlas: &mut Atlas, rect: Rect, id: &str) -> bool {
    if let Some(tex) = atlas.texture(ui.ctx(), id) {
        ui.painter().image(tex.id(), rect, full_uv(), Color32::WHITE);
        true
    } else {
        false
    }
}

// Placeholder for an item whose sprite isn't in the atlas, so it reads as a
// missing texture rather than a blank slot.
fn paint_missing(ui: &egui::Ui, rect: Rect) {
    let painter = ui.painter();
    painter.rect_filled(rect, Rounding::same(2.0), Color32::from_rgb(48, 44, 58));
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "?",
        FontId::proportional(rect.height() * 0.62),
        Color32::from_rgb(150, 120, 180),
    );
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
            // A player head in a Minecraft slot renders as a small 8px skull,
            // noticeably smaller than a full item sprite. The head3d texture
            // already carries a margin, so drawing it slightly inset from the
            // icon rect matches the in-game footprint.
            ui.painter().image(tex.id(), rect.shrink(rect.width() * 0.06), full_uv(), Color32::WHITE);
            return false;
        }
    }
    if !paint_texture(ui, atlas, rect, &item.id) {
        paint_missing(ui, rect);
        return false;
    }
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
    hl: Option<&Highlight>,
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

    if let Some(g) = &item.gauge_text {
        let col = item.bar.map(|b| b.color).unwrap_or([150, 200, 150]);
        ui.label(
            egui::RichText::new(g)
                .color(Color32::from_rgb(col[0], col[1], col[2]))
                .size(12.0),
        );
    } else if let Some(dmg) = item.damage {
        let dur = match item.max_damage {
            Some(max) if max > 0 => format!("Durability: {} / {} used", max - dmg, max),
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
            if let Some(bar) = &c.bar {
                paint_bar(ui, srect, bar);
            }
            if let Some(col) = c.outline {
                paint_outline(ui, srect, Color32::from_rgb(col[0], col[1], col[2]));
            }
            if has_openable(c, bp) {
                paint_nested_badge(ui, srect);
            }
            match match_level(c, hl, bp) {
                2 => paint_match(ui, srect, true),
                1 => paint_match(ui, srect, false),
                _ => {}
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
    hl: Option<&Highlight>,
    profiles: &mut Profiles,
    gframe: usize,
    slot: f32,
    animating: &mut bool,
) -> Option<Drill> {
    let cols = 9usize;
    let rows = items.len().max(1).div_ceil(cols);
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(cols as f32 * slot, rows as f32 * slot),
        Sense::hover(),
    );
    let mut drill = None;
    let mut hovered: Option<Rect> = None;
    let mut matches: Vec<(Rect, bool)> = Vec::new();
    for (i, it) in items.iter().enumerate() {
        let min = rect.min + Vec2::new((i % cols) as f32 * slot, (i / cols) as f32 * slot);
        let srect = Rect::from_min_size(min, Vec2::splat(slot));
        let openable = has_openable(it, bp);
        paint_slot(ui, atlas, profiles, srect, it, gframe, true, animating);
        if openable {
            paint_nested_badge(ui, srect);
        }
        match match_level(it, hl, bp) {
            2 => matches.push((srect, true)),
            1 => matches.push((srect, false)),
            _ => {}
        }
        let sense = if openable { Sense::click() } else { Sense::hover() };
        let resp = ui.interact(srect, Id::new(("nested-slot", i)), sense);
        if resp.hovered() {
            hovered = Some(srect);
        }
        if openable && resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if openable && resp.clicked() {
            drill = Some((
                it.display_name(),
                resolve_nested(it, bp).to_vec(),
                it.storage_uuid.clone(),
            ));
        }
        let it2 = it.clone();
        resp.on_hover_ui(|ui| item_tooltip(ui, atlas, &it2, gframe, bp, hl, profiles));
    }
    for (r, strong) in matches {
        paint_match(ui, r, strong);
    }
    if let Some(r) = hovered {
        paint_hover_ring(ui, r);
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
