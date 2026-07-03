mod card;

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

use eframe::egui::{
    self, Align, Color32, FontId, Id, Layout, Pos2, Rect, Rounding, Stroke, Vec2,
};

use card::{card_height, draw_card, nested_grid};
use crate::render::atlas::{Atlas, GLINT_FRAMES};
use crate::render::export::McFont;
use crate::model::{Entry, EntryKind, Item};
use crate::profiles::{Fetched, Profiles};
use crate::search::{DungeonFilter, EnchOp, Filters, TextCat};
use crate::settings::{SavedFile, Settings};
use crate::store::Store;

pub(crate) const ACCENT: Color32 = Color32::from_rgb(86, 171, 96);
pub(crate) const GOLD: Color32 = Color32::from_rgb(212, 155, 36);
const ERRC: Color32 = Color32::from_rgb(220, 90, 80);

enum Msg {
    Assets(Result<(Atlas, McFont), String>),
    Data(u64, Result<Store, String>),
    Profile(String, Result<Fetched, String>),
}

#[derive(PartialEq, Clone, Copy)]
enum Mode {
    Auto,
    Backpacks,
    Containers,
    Players,
}

impl Mode {
    fn label(self) -> &'static str {
        match self {
            Mode::Auto => "Auto",
            Mode::Backpacks => "Backpacks",
            Mode::Containers => "Containers",
            Mode::Players => "Players",
        }
    }
    fn from_label(s: &str) -> Mode {
        match s {
            "Backpacks" => Mode::Backpacks,
            "Containers" => Mode::Containers,
            "Players" => Mode::Players,
            _ => Mode::Auto,
        }
    }
}

struct Source {
    id: u64,
    path: String,
    name: String,
    mode: Mode,
    enabled: bool,
    loading: bool,
    error: Option<String>,
    store: Option<Store>,
}

impl Source {
    fn len(&self) -> usize {
        self.store.as_ref().map_or(0, |s| s.len())
    }
}

pub(crate) enum Action {
    Copy(String),
    CopyImg(usize, usize),
    Export(usize, usize),
    OpenNested(String, Vec<Item>),
}

struct PopupLevel {
    title: String,
    items: Vec<Item>,
}

pub struct App {
    ctx: egui::Context,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    atlas: Option<Atlas>,
    font: Option<McFont>,
    sources: Vec<Source>,
    filtered: Vec<(usize, usize)>,
    filters: Filters,
    mode: Mode,
    slot: f32,
    loading_assets: bool,
    status: String,
    error: Option<String>,
    clipboard: Option<arboard::Clipboard>,
    toast: Option<(String, Instant)>,
    hovering_file: bool,
    advanced_open: bool,
    bp_index: std::collections::HashMap<String, Vec<Item>>,
    popup: Vec<PopupLevel>,
    profiles: Profiles,
    next_id: u64,
}

fn filename(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

fn open_store(path: &str, mode: Mode) -> Result<Store, String> {
    use crate::parse::containers::JsonKind;
    use crate::parse::dump_nbt::DumpKind;
    use crate::store::Load;
    let is_json = path.to_lowercase().ends_with(".json");
    let load = match mode {
        Mode::Auto => Load::auto(path),
        Mode::Backpacks => Load::Backpacks,
        Mode::Containers => {
            if is_json {
                Load::Json(Some(JsonKind::Containers))
            } else {
                Load::Nbt(Some(DumpKind::Containers))
            }
        }
        Mode::Players => {
            if is_json {
                Load::Json(Some(JsonKind::Players))
            } else {
                Load::Nbt(Some(DumpKind::Players))
            }
        }
    };
    Store::open(path, load)
}

impl App {
    pub fn new(ctx: egui::Context) -> Self {
        let (tx, rx) = channel();

        let atx = tx.clone();
        let actx = ctx.clone();
        std::thread::spawn(move || {
            let res = (|| {
                let dir = crate::render::atlas::assets_dir();
                let atlas = Atlas::load(&dir)?;
                let font = McFont::load(&dir)?;
                Ok((atlas, font))
            })();
            let _ = atx.send(Msg::Assets(res));
            actx.request_repaint();
        });

        let settings = Settings::load();
        let mut app = App {
            ctx,
            tx,
            rx,
            atlas: None,
            font: None,
            sources: Vec::new(),
            filtered: Vec::new(),
            filters: Filters::default(),
            mode: Mode::from_label(&settings.mode),
            slot: settings.zoom.clamp(24.0, 64.0),
            loading_assets: true,
            status: "Loading item atlas…".into(),
            error: None,
            clipboard: arboard::Clipboard::new().ok(),
            toast: None,
            hovering_file: false,
            advanced_open: false,
            bp_index: std::collections::HashMap::new(),
            popup: Vec::new(),
            profiles: Profiles::new(),
            next_id: 0,
        };

        for f in &settings.files {
            if std::path::Path::new(&f.path).exists() {
                app.add_source(f.path.clone(), Mode::from_label(&f.mode), f.enabled);
            }
        }
        app
    }

    pub fn request_open(&mut self, path: String) {
        let mode = self.mode;
        self.add_source(path, mode, true);
    }

    fn add_source(&mut self, path: String, mode: Mode, enabled: bool) {
        if self.sources.iter().any(|s| s.path == path) {
            return;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.sources.push(Source {
            id,
            name: filename(&path),
            path: path.clone(),
            mode,
            enabled,
            loading: true,
            error: None,
            store: None,
        });

        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let res = open_store(&path, mode);
            let _ = tx.send(Msg::Data(id, res));
            ctx.request_repaint();
        });
    }

    fn run_search(&mut self) {
        let c = self.filters.compile();
        let mut out = Vec::new();
        let mut total = 0usize;
        for (si, s) in self.sources.iter().enumerate() {
            if !s.enabled {
                continue;
            }
            let Some(store) = &s.store else { continue };
            total += store.len();
            for ei in store.filter(&c) {
                out.push((si, ei as usize));
            }
        }
        self.filtered = out;
        self.status = format!("{} of {} shown", self.filtered.len(), total);
    }

    fn advanced_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let mut ch = false;
        let field = |ui: &mut egui::Ui, val: &mut String, hint: &str, w: f32| {
            ui.add(egui::TextEdit::singleline(val).hint_text(hint).desired_width(w))
                .changed()
        };
        ui.add_space(4.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            egui::Grid::new("adv_grid")
                .num_columns(4)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Show");
                    ui.horizontal(|ui| {
                        ch |= ui.checkbox(&mut self.filters.show_backpacks, "Backpacks").changed();
                        ch |= ui.checkbox(&mut self.filters.show_containers, "Containers").changed();
                        ch |= ui.checkbox(&mut self.filters.show_players, "Players").changed();
                    });
                    ui.label("Dungeon");
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("dungeon")
                            .selected_text(self.filters.dungeon.label())
                            .show_ui(ui, |ui| {
                                for d in
                                    [DungeonFilter::Any, DungeonFilter::Only, DungeonFilter::Hide]
                                {
                                    ch |= ui
                                        .selectable_value(&mut self.filters.dungeon, d, d.label())
                                        .changed();
                                }
                            });
                        ch |= ui
                            .checkbox(&mut self.filters.hide_empty, "Hide empty")
                            .on_hover_text("Hide containers/backpacks with no items")
                            .changed();
                    });
                    ui.end_row();

                    ui.label("Player / UUID");
                    ch |= field(ui, &mut self.filters.player, "name or uuid", 190.0);
                    ui.label("Item id");
                    ch |= field(ui, &mut self.filters.item, "minecraft:diamond", 190.0);
                    ui.end_row();

                    ui.label("Container type");
                    ch |= field(ui, &mut self.filters.ctype, "chest, barrel, shulker…", 190.0);
                    ui.label("Dimension");
                    ch |= field(ui, &mut self.filters.dimension, "overworld / nether / end", 190.0);
                    ui.end_row();

                    ui.label("Custom NBT");
                    ch |= field(ui, &mut self.filters.nbt, "unbreakable, sophisticatedcore:…", 190.0);
                    ui.label("Min stack ≥");
                    ch |= field(ui, &mut self.filters.min_count, "e.g. 65 (dupe hunt)", 190.0);
                    ui.end_row();

                    ui.label("X range");
                    ui.horizontal(|ui| {
                        ch |= field(ui, &mut self.filters.x_min, "min", 72.0);
                        ch |= field(ui, &mut self.filters.x_max, "max", 72.0);
                    });
                    ui.label("Y range");
                    ui.horizontal(|ui| {
                        ch |= field(ui, &mut self.filters.y_min, "min", 72.0);
                        ch |= field(ui, &mut self.filters.y_max, "max", 72.0);
                    });
                    ui.end_row();

                    ui.label("Z range");
                    ui.horizontal(|ui| {
                        ch |= field(ui, &mut self.filters.z_min, "min", 72.0);
                        ch |= field(ui, &mut self.filters.z_max, "max", 72.0);
                    });
                    ui.label("");
                    if ui.button("🧹 Clear filters").clicked() {
                        self.filters.clear_advanced();
                        ch = true;
                    }
                    ui.end_row();
                });
            ui.label(
                egui::RichText::new(
                    "Filters combine (AND). Coords/type/dungeon apply to containers; player/UUID to backpacks & players.",
                )
                .weak()
                .size(11.0),
            );
        });
        ch
    }

    fn save_settings(&self) {
        let files = self
            .sources
            .iter()
            .map(|s| SavedFile {
                path: s.path.clone(),
                enabled: s.enabled,
                mode: s.mode.label().to_string(),
            })
            .collect();
        Settings {
            files,
            zoom: self.slot,
            mode: self.mode.label().to_string(),
        }
        .save();
    }

    fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    fn drain_messages(&mut self) {
        let mut got_data = false;
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Assets(Ok((atlas, font))) => {
                    self.atlas = Some(atlas);
                    self.font = Some(font);
                    self.loading_assets = false;
                    self.status = "Ready.".into();
                }
                Msg::Assets(Err(e)) => {
                    self.loading_assets = false;
                    self.error = Some(format!("Asset load failed: {e}"));
                }
                Msg::Data(id, res) => {
                    if let Some(pos) = self.sources.iter().position(|s| s.id == id) {
                        let s = &mut self.sources[pos];
                        s.loading = false;
                        match res {
                            Ok(store) => {
                                s.store = Some(store);
                                s.error = None;
                            }
                            Err(e) => s.error = Some(e),
                        }
                    }
                    got_data = true;
                }
                Msg::Profile(uuid, res) => {
                    if let Ok(f) = &res {
                        if let Some(name) = &f.username {
                            if self.apply_username(&uuid, name) {
                                got_data = true;
                            }
                        }
                    }
                    self.profiles.set(uuid, res);
                }
            }
        }
        if got_data {
            self.rebuild_bp_index();
            self.run_search();
        }
    }

    fn spawn_pending_fetches(&mut self) {
        for key in self.profiles.drain_requests() {
            let tx = self.tx.clone();
            let ctx = self.ctx.clone();
            std::thread::spawn(move || {
                let res = crate::profiles::fetch(&key);
                let _ = tx.send(Msg::Profile(key, res));
                ctx.request_repaint();
            });
        }
    }

    fn apply_username(&mut self, uuid: &str, name: &str) -> bool {
        let mut changed = false;
        for s in &mut self.sources {
            if let Some(store) = &mut s.store {
                if store.apply_username(uuid, name) {
                    changed = true;
                }
            }
        }
        changed
    }

    fn rebuild_bp_index(&mut self) {
        self.bp_index.clear();
        for s in &self.sources {
            if !s.enabled {
                continue;
            }
            let Some(store) = &s.store else { continue };
            for e in store.mem_entries() {
                if !e.uuid.is_empty() && !e.items.is_empty() {
                    self.bp_index
                        .entry(e.uuid.clone())
                        .or_insert_with(|| e.items.clone());
                }
            }
        }
    }

    fn entry(&self, si: usize, ei: usize) -> Option<Arc<Entry>> {
        self.sources.get(si)?.store.as_ref()?.entry(ei)
    }

    fn render_png(&self, entry: &Entry) -> Result<image::RgbaImage, String> {
        let atlas = self.atlas.as_ref().ok_or("atlas not loaded")?;
        let font = self.font.as_ref().ok_or("font not loaded")?;
        Ok(crate::render::export::render_entry(entry, atlas, font))
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_messages();
        let mut dirty = false;

        let mut dropped: Vec<String> = Vec::new();
        ctx.input(|i| {
            self.hovering_file = !i.raw.hovered_files.is_empty();
            for f in &i.raw.dropped_files {
                if let Some(p) = &f.path {
                    dropped.push(p.display().to_string());
                }
            }
        });
        for path in dropped {
            let mode = self.mode;
            self.add_source(path, mode, true);
            dirty = true;
        }

        egui::TopBottomPanel::top("controls").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.heading(egui::RichText::new("🎒 Backpack Infiltrator").color(ACCENT));
                ui.separator();

                if ui.button("📂 Load files…").clicked() {
                    if let Some(paths) = rfd::FileDialog::new()
                        .add_filter("Backpacks / Containers / Players", &["dat", "nbt", "json"])
                        .pick_files()
                    {
                        let mode = self.mode;
                        for p in paths {
                            self.add_source(p.display().to_string(), mode, true);
                        }
                        dirty = true;
                    }
                }

                ui.separator();
                ui.label("Load as");
                egui::ComboBox::from_id_salt("mode")
                    .selected_text(self.mode.label())
                    .show_ui(ui, |ui| {
                        for m in [Mode::Auto, Mode::Backpacks, Mode::Containers, Mode::Players] {
                            if ui.selectable_value(&mut self.mode, m, m.label()).changed() {
                                dirty = true;
                            }
                        }
                    });

                ui.separator();
                ui.label("Zoom");
                let z = ui.add(egui::Slider::new(&mut self.slot, 24.0..=64.0).show_value(false));
                if z.drag_stopped() {
                    dirty = true;
                }
            });

            ui.add_space(2.0);
            let mut changed = false;
            ui.horizontal_wrapped(|ui| {
                ui.label("🔎");
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut self.filters.text)
                            .hint_text("search…")
                            .desired_width(240.0),
                    )
                    .changed();

                egui::ComboBox::from_id_salt("cat")
                    .selected_text(self.filters.cat.label())
                    .show_ui(ui, |ui| {
                        for c in [
                            TextCat::Any,
                            TextCat::Owner,
                            TextCat::Item,
                            TextCat::Type,
                            TextCat::Upgrade,
                        ] {
                            changed |= ui
                                .selectable_value(&mut self.filters.cat, c, c.label())
                                .changed();
                        }
                    });

                if ui
                    .selectable_label(
                        self.advanced_open || self.filters.advanced_active(),
                        "⚙ Advanced",
                    )
                    .clicked()
                {
                    self.advanced_open = !self.advanced_open;
                }

                ui.separator();
                ui.label(egui::RichText::new("✨ Enchant").color(GOLD));
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut self.filters.ench_name)
                            .hint_text("any enchant")
                            .desired_width(150.0),
                    )
                    .changed();

                egui::ComboBox::from_id_salt("ench_op")
                    .selected_text(self.filters.ench_op.label())
                    .show_ui(ui, |ui| {
                        for op in [EnchOp::Any, EnchOp::Gte, EnchOp::Eq, EnchOp::Gt] {
                            changed |= ui
                                .selectable_value(&mut self.filters.ench_op, op, op.label())
                                .changed();
                        }
                    });
                if self.filters.ench_op != EnchOp::Any {
                    changed |= ui
                        .add(egui::DragValue::new(&mut self.filters.ench_level).range(1..=255))
                        .changed();
                }
                if ui.button("Find 255s").clicked() {
                    self.filters.ench_name.clear();
                    self.filters.ench_op = EnchOp::Gte;
                    self.filters.ench_level = 100;
                    changed = true;
                }
            });

            if self.advanced_open {
                changed |= self.advanced_ui(ui);
            }

            ui.add_space(6.0);
            if changed {
                self.run_search();
            }
        });

        let mut add_clicked = false;
        let mut sources_changed = false;
        egui::SidePanel::left("sources")
            .resizable(true)
            .default_width(240.0)
            .show(ctx, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.heading(egui::RichText::new("Files").color(ACCENT));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("＋").on_hover_text("Add files").clicked() {
                            add_clicked = true;
                        }
                    });
                });
                ui.separator();

                if self.sources.is_empty() {
                    ui.label(egui::RichText::new("No files loaded.").weak().size(12.0));
                }

                let mut remove: Option<usize> = None;
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    for (i, s) in self.sources.iter_mut().enumerate() {
                        egui::Frame::none()
                            .fill(Color32::from_rgb(33, 35, 43))
                            .rounding(Rounding::same(6.0))
                            .inner_margin(egui::Margin::same(7.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    if ui.checkbox(&mut s.enabled, "").changed() {
                                        sources_changed = true;
                                    }
                                    let col = source_color(s);
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(&s.name).color(col).strong(),
                                        )
                                        .truncate(),
                                    )
                                    .on_hover_text(&s.path);
                                    ui.with_layout(
                                        Layout::right_to_left(Align::Center),
                                        |ui| {
                                            if ui
                                                .small_button("✕")
                                                .on_hover_text("Remove")
                                                .clicked()
                                            {
                                                remove = Some(i);
                                            }
                                        },
                                    );
                                });
                                if s.loading {
                                    ui.horizontal(|ui| {
                                        ui.add(egui::Spinner::new().size(14.0));
                                        ui.label(egui::RichText::new("parsing…").weak().size(11.0));
                                    });
                                } else if let Some(e) = &s.error {
                                    ui.label(egui::RichText::new(e).color(ERRC).size(11.0));
                                } else {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{} · {} entries",
                                            s.mode.label(),
                                            s.len()
                                        ))
                                        .weak()
                                        .size(11.0),
                                    );
                                }
                            });
                        ui.add_space(5.0);
                    }
                });

                if let Some(i) = remove {
                    self.sources.remove(i);
                    sources_changed = true;
                }
            });
        if add_clicked {
            if let Some(paths) = rfd::FileDialog::new()
                .add_filter("Backpacks / Containers / Players", &["dat", "nbt", "json"])
                .pick_files()
            {
                let mode = self.mode;
                for p in paths {
                    self.add_source(p.display().to_string(), mode, true);
                }
                dirty = true;
            }
        }
        if sources_changed {
            self.rebuild_bp_index();
            self.run_search();
            dirty = true;
        }

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let any_loading = self.loading_assets || self.sources.iter().any(|s| s.loading);
                if any_loading {
                    ui.add(egui::Spinner::new().size(14.0));
                }
                if let Some(err) = &self.error {
                    ui.colored_label(ERRC, err);
                } else {
                    ui.label(&self.status);
                }
            });
        });

        let mut actions: Vec<Action> = Vec::new();
        let mut do_load = false;
        let mut animating = false;
        let gframe = ((ctx.input(|i| i.time) * 12.0) as usize) % GLINT_FRAMES;

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.atlas.is_none() {
                if let Some(err) = &self.error {
                    center_message(ui, "⚠", err, ERRC);
                } else {
                    center_spinner(ui, "Loading item atlas…");
                }
                return;
            }
            if self.sources.is_empty() {
                do_load = welcome_screen(ui);
                return;
            }
            if self.filtered.is_empty() {
                if self.sources.iter().any(|s| s.loading) {
                    center_spinner(ui, "Parsing…");
                } else {
                    center_message(
                        ui,
                        "🔍",
                        "No results - adjust filters or enable files on the left.",
                        Color32::from_gray(150),
                    );
                }
                return;
            }

            let slot = self.slot;
            let avail_w = ui.available_width();
            let atlas = self.atlas.as_mut().unwrap();
            let sources = &self.sources;
            let filtered = &self.filtered;
            let bp = &self.bp_index;
            let profiles = &mut self.profiles;
            let spacing = 12.0;
            let heights: Vec<f32> = filtered
                .iter()
                .map(|&(si, ei)| {
                    sources[si]
                        .store
                        .as_ref()
                        .map(|s| card_height(&s.metas()[ei], slot))
                        .unwrap_or(0.0)
                })
                .collect();

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show_viewport(ui, |ui, vp| {
                    ui.set_width(avail_w);
                    let total: f32 = heights.iter().map(|h| h + spacing).sum();
                    let (_id, block) =
                        ui.allocate_space(Vec2::new(avail_w, total.max(1.0)));
                    let top = block.min.y;
                    let mut y = 0.0f32;
                    for (k, &(si, ei)) in filtered.iter().enumerate() {
                        let h = heights[k];
                        let visible = !(y + h < vp.min.y - 300.0 || y > vp.max.y + 300.0);
                        if visible {
                            let rect = Rect::from_min_size(
                                Pos2::new(block.min.x, top + y),
                                Vec2::new(avail_w - 8.0, h),
                            );
                            let mut child = ui.new_child(
                                egui::UiBuilder::new().max_rect(rect).layout(*ui.layout()),
                            );
                            if let Some(entry) =
                                sources[si].store.as_ref().and_then(|s| s.entry(ei))
                            {
                                draw_card(
                                    &mut child,
                                    atlas,
                                    &entry,
                                    si,
                                    ei,
                                    slot,
                                    gframe,
                                    bp,
                                    profiles,
                                    &mut actions,
                                    &mut animating,
                                );
                            }
                        }
                        y += h + spacing;
                    }
                });
        });

        if do_load {
            if let Some(paths) = rfd::FileDialog::new()
                .add_filter("Backpacks / Containers / Players", &["dat", "nbt", "json"])
                .pick_files()
            {
                let mode = self.mode;
                for p in paths {
                    self.add_source(p.display().to_string(), mode, true);
                }
                dirty = true;
            }
        }

        if self.hovering_file {
            let screen = ctx.screen_rect();
            let painter =
                ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("drop")));
            painter.rect_filled(screen, 0.0, Color32::from_rgba_unmultiplied(20, 24, 20, 200));
            painter.rect_stroke(screen.shrink(16.0), Rounding::same(16.0), Stroke::new(3.0, ACCENT));
            painter.text(
                screen.center(),
                egui::Align2::CENTER_CENTER,
                "⬇  Drop file(s) to load",
                FontId::proportional(28.0),
                ACCENT,
            );
        }

        for a in actions {
            match a {
                Action::Copy(s) => {
                    ctx.copy_text(s);
                    self.toast("Copied to clipboard");
                }
                Action::CopyImg(si, ei) => match self
                    .entry(si, ei)
                    .ok_or_else(|| "entry unavailable".to_string())
                    .and_then(|e| self.render_png(&e))
                {
                    Ok(img) => {
                        if let Some(cb) = self.clipboard.as_mut() {
                            let (w, h) = (img.width() as usize, img.height() as usize);
                            let data = arboard::ImageData {
                                width: w,
                                height: h,
                                bytes: img.into_raw().into(),
                            };
                            match cb.set_image(data) {
                                Ok(_) => self.toast("Image copied"),
                                Err(e) => self.toast(format!("Copy failed: {e}")),
                            }
                        }
                    }
                    Err(e) => self.toast(format!("Render failed: {e}")),
                },
                Action::Export(si, ei) => match self.entry(si, ei) {
                    Some(entry) => match self.render_png(&entry) {
                        Ok(img) => {
                            let name = default_png_name(&entry);
                            if let Some(path) = rfd::FileDialog::new()
                                .set_file_name(name)
                                .add_filter("PNG", &["png"])
                                .save_file()
                            {
                                match img.save(&path) {
                                    Ok(_) => self.toast("Saved PNG"),
                                    Err(e) => self.toast(format!("Save failed: {e}")),
                                }
                            }
                        }
                        Err(e) => self.toast(format!("Render failed: {e}")),
                    },
                    None => self.toast("Entry unavailable"),
                },
                Action::OpenNested(title, items) => {
                    self.popup.clear();
                    self.popup.push(PopupLevel { title, items });
                }
            }
        }

        if let (false, Some(atlas)) = (self.popup.is_empty(), self.atlas.as_mut()) {
            let slot = self.slot;
            let bp = &self.bp_index;
            let profiles = &mut self.profiles;
            let depth = self.popup.len();
            let level = self.popup.last().unwrap();
            let title = level.title.clone();
            let items = &level.items;
            let count = items.len();

            let mut close = false;
            let mut back = false;
            let mut drill: Option<(String, Vec<Item>)> = None;

            egui::Window::new(egui::RichText::new(format!("📦  {title}")).color(ACCENT))
                .id(Id::new("nested_popup"))
                .collapsible(false)
                .resizable(true)
                .default_width(9.0 * slot + 36.0)
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        if depth > 1 && ui.button("‹ Back").clicked() {
                            back = true;
                        }
                        ui.label(
                            egui::RichText::new(format!("{count} item(s)")).weak().size(12.0),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button("✕ Close").clicked() {
                                close = true;
                            }
                        });
                    });
                    ui.separator();
                    egui::ScrollArea::vertical().max_height(440.0).show(ui, |ui| {
                        if let Some(d) =
                            nested_grid(ui, atlas, items, bp, profiles, gframe, slot, &mut animating)
                        {
                            drill = Some(d);
                        }
                    });
                });

            if let Some((t, its)) = drill {
                self.popup.push(PopupLevel { title: t, items: its });
            } else if back {
                self.popup.pop();
            } else if close {
                self.popup.clear();
            }
            ctx.request_repaint();
        }

        self.spawn_pending_fetches();

        if let Some((msg, at)) = &self.toast {
            if at.elapsed().as_secs_f32() < 1.8 {
                let msg = msg.clone();
                egui::Area::new(Id::new("toast"))
                    .anchor(egui::Align2::CENTER_BOTTOM, Vec2::new(0.0, -46.0))
                    .show(ctx, |ui| {
                        egui::Frame::popup(ui.style()).fill(ACCENT).show(ui, |ui| {
                            ui.label(egui::RichText::new(msg).color(Color32::WHITE).strong());
                        });
                    });
                ctx.request_repaint();
            } else {
                self.toast = None;
            }
        }

        if dirty {
            self.save_settings();
        }
        if self.loading_assets || animating || self.sources.iter().any(|s| s.loading) {
            ctx.request_repaint();
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_settings();
    }
}

fn source_color(s: &Source) -> Color32 {
    match s.store.as_ref().and_then(|st| st.first_kind()) {
        Some(EntryKind::Backpack) => Color32::from_rgb(190, 150, 230),
        Some(EntryKind::Player) => Color32::from_rgb(110, 190, 240),
        Some(EntryKind::Container) => GOLD,
        None => Color32::from_gray(200),
    }
}

fn default_png_name(e: &Entry) -> String {
    let base = e.title.replace([':', '/', ' ', ',', '@'], "_").replace("__", "_");
    format!("{base}.png")
}

fn center_spinner(ui: &mut egui::Ui, text: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.38);
        ui.add(egui::Spinner::new().size(34.0).color(ACCENT));
        ui.add_space(10.0);
        ui.label(egui::RichText::new(text).size(15.0).weak());
    });
}

fn center_message(ui: &mut egui::Ui, icon: &str, text: &str, color: Color32) {
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.38);
        ui.label(egui::RichText::new(icon).size(34.0).color(color));
        ui.add_space(8.0);
        ui.label(egui::RichText::new(text).size(14.0).color(color));
    });
}

fn welcome_screen(ui: &mut egui::Ui) -> bool {
    let mut clicked = false;
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.16);
        ui.label(egui::RichText::new("🎒").size(58.0));
        ui.add_space(2.0);
        ui.label(
            egui::RichText::new("Backpack Infiltrator")
                .size(26.0)
                .strong()
                .color(ACCENT),
        );
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("Drag & drop files anywhere, or load them below")
                .size(14.0)
                .weak(),
        );
        ui.add_space(16.0);
        if ui
            .add(
                egui::Button::new(egui::RichText::new("📂  Load files…").size(15.0).strong())
                    .min_size(Vec2::new(190.0, 42.0))
                    .fill(ACCENT.linear_multiply(0.85))
                    .rounding(Rounding::same(9.0)),
            )
            .clicked()
        {
            clicked = true;
        }
        ui.add_space(22.0);
        ui.label(egui::RichText::new("SUPPORTED FILES").size(11.0).weak());
        ui.add_space(6.0);
        for (icon, name, desc) in [
            ("🎒", "sophisticatedbackpacks.dat", "backpacks - items, upgrades, enchants"),
            ("📦", "*_container_dump.json", "world containers - chests, barrels, shulkers"),
            ("🧍", "*_player_dump.json", "players - inventory + ender chest"),
        ] {
            ui.horizontal(|ui| {
                ui.add_space(ui.available_width() * 0.5 - 170.0);
                ui.label(egui::RichText::new(icon).size(15.0));
                ui.label(egui::RichText::new(name).size(13.0).monospace().color(GOLD));
                ui.label(egui::RichText::new(format!("- {desc}")).size(12.5).weak());
            });
        }
    });
    clicked
}
