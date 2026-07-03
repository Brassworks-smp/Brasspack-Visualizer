mod card;

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

use eframe::egui::{
    self, Align, Color32, FontId, Id, Layout, Pos2, Rect, Rounding, Stroke, Vec2,
};

use card::{card_height, card_width, draw_card, nested_grid};
use crate::render::atlas::{Atlas, GLINT_FRAMES};
use crate::render::export::McFont;
use crate::model::{Entry, EntryKind, Item};
use crate::profiles::{Fetched, Profiles};
use crate::search::{DungeonFilter, EnchOp, Filters, Highlight, TextCat};
use crate::settings::{SavedFile, Settings};
use crate::store::Store;

use crate::color::rgb;

pub(crate) const ACCENT: Color32 = rgb(0x56ab60);
pub(crate) const GOLD: Color32 = rgb(0xd49b24);
const ERRC: Color32 = rgb(0xdc5a50);
pub(crate) const KIND_BACKPACK: Color32 = rgb(0xbe96e6);
pub(crate) const KIND_PLAYER: Color32 = rgb(0x6ebef0);

enum Msg {
    Atlas(Result<Atlas, String>),
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
    OpenNested(String, Vec<Item>, Option<String>),
}

#[derive(Clone, Default)]
struct BpInfo {
    owner: String,
}

struct PopupLevel {
    title: String,
    items: Vec<Item>,
    uuid: Option<String>,
    owner: Option<String>,
    copies: Vec<crate::model::CopyAction>,
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
    loading_atlas: bool,
    atlas_path: Option<String>,
    atlas_error: Option<String>,
    status: String,
    error: Option<String>,
    clipboard: Option<arboard::Clipboard>,
    toast: Option<(String, Instant)>,
    hovering_file: bool,
    advanced_open: bool,
    bp_index: std::collections::HashMap<String, Vec<Item>>,
    bp_meta: std::collections::HashMap<String, BpInfo>,
    popup: Vec<PopupLevel>,
    search_help: bool,
    highlight: Option<Highlight>,
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

        let (font, font_err) = match McFont::load() {
            Ok(f) => (Some(f), None),
            Err(e) => (None, Some(format!("Font load failed: {e}"))),
        };

        let settings = Settings::load();
        let mut app = App {
            ctx,
            tx,
            rx,
            atlas: None,
            font,
            sources: Vec::new(),
            filtered: Vec::new(),
            filters: Filters::default(),
            mode: Mode::from_label(&settings.mode),
            slot: settings.zoom.clamp(24.0, 64.0),
            loading_atlas: false,
            atlas_path: None,
            atlas_error: None,
            status: "Select a brass_atlas.zip to show item sprites.".into(),
            error: font_err,
            clipboard: arboard::Clipboard::new().ok(),
            toast: None,
            hovering_file: false,
            advanced_open: false,
            bp_index: std::collections::HashMap::new(),
            bp_meta: std::collections::HashMap::new(),
            popup: Vec::new(),
            search_help: false,
            highlight: None,
            profiles: Profiles::new(),
            next_id: 0,
        };

        for f in &settings.files {
            if std::path::Path::new(&f.path).exists() {
                app.add_source(f.path.clone(), Mode::from_label(&f.mode), f.enabled);
            }
        }
        if !settings.atlas.is_empty() && std::path::Path::new(&settings.atlas).exists() {
            app.load_atlas(settings.atlas.clone());
        }
        app
    }

    fn load_atlas(&mut self, path: String) {
        self.atlas_path = Some(path.clone());
        self.atlas_error = None;
        self.loading_atlas = true;
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let res = Atlas::load(&path);
            let _ = tx.send(Msg::Atlas(res));
            ctx.request_repaint();
        });
    }

    fn pick_atlas(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Brass atlas", &["zip"])
            .pick_file()
        {
            self.load_atlas(path.display().to_string());
            self.save_settings();
        }
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
        self.highlight = self.filters.highlight();
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
                    if ui.button("Clear filters").clicked() {
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
            atlas: self.atlas_path.clone().unwrap_or_default(),
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
                Msg::Atlas(Ok(atlas)) => {
                    self.atlas = Some(atlas);
                    self.loading_atlas = false;
                    self.atlas_error = None;
                    self.status = "Ready.".into();
                }
                Msg::Atlas(Err(e)) => {
                    self.loading_atlas = false;
                    self.atlas_error = Some(e);
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
        self.bp_meta.clear();
        for s in &self.sources {
            if !s.enabled {
                continue;
            }
            let Some(store) = &s.store else { continue };
            for e in store.mem_entries() {
                if e.kind != EntryKind::Backpack || e.uuid.is_empty() {
                    continue;
                }
                if !e.items.is_empty() {
                    self.bp_index
                        .entry(e.uuid.clone())
                        .or_insert_with(|| e.items.clone());
                }
                self.bp_meta.entry(e.uuid.clone()).or_insert_with(|| {
                    let owner = e
                        .meta
                        .iter()
                        .find(|(k, _)| k == "Owner")
                        .map(|(_, v)| v.clone())
                        .filter(|v| !v.is_empty() && v != "Unknown")
                        .unwrap_or_else(|| e.owner.clone());
                    BpInfo { owner }
                });
            }
        }
    }

    fn open_nested(&self, title: String, items: Vec<Item>, uuid: Option<String>) -> PopupLevel {
        let info = uuid.as_ref().and_then(|u| self.bp_meta.get(u));
        let mut copies = Vec::new();
        if let Some(u) = &uuid {
            if let Some(info) = info {
                if !info.owner.is_empty() {
                    copies.push(crate::model::CopyAction {
                        label: "Copy Owner".into(),
                        value: info.owner.clone(),
                    });
                }
            }
            copies.push(crate::model::CopyAction {
                label: "Copy UUID".into(),
                value: u.clone(),
            });
        }
        PopupLevel {
            title,
            items,
            owner: info.map(|i| i.owner.clone()).filter(|s| !s.is_empty()),
            uuid,
            copies,
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
                ui.heading(egui::RichText::new("Backpack Infiltrator").color(ACCENT));
                ui.separator();

                if ui.button("Load files…").clicked() {
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
                ui.label(egui::RichText::new("Find").weak());
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut self.filters.text)
                            .hint_text("search…  (and / or / not, quotes, parens)")
                            .desired_width(280.0),
                    )
                    .changed();

                if ui
                    .selectable_label(self.search_help, "?")
                    .on_hover_text("Search syntax help")
                    .clicked()
                {
                    self.search_help = !self.search_help;
                }

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
                        "Advanced",
                    )
                    .clicked()
                {
                    self.advanced_open = !self.advanced_open;
                }

                ui.separator();
                ui.label(egui::RichText::new("Enchant").color(GOLD));
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

            if self.search_help {
                search_help_ui(ui);
            }

            if self.advanced_open {
                changed |= self.advanced_ui(ui);
            }

            ui.add_space(6.0);
            if changed {
                self.run_search();
            }
        });

        let mut add_clicked = false;
        let mut atlas_clicked = false;
        let mut sources_changed = false;
        egui::SidePanel::left("sources")
            .resizable(true)
            .default_width(240.0)
            .show(ctx, |ui| {
                ui.add_space(6.0);
                ui.heading(egui::RichText::new("Sprites").color(ACCENT));
                ui.add_space(3.0);
                egui::Frame::none()
                    .fill(rgb(0x21232b))
                    .rounding(Rounding::same(6.0))
                    .inner_margin(egui::Margin::same(7.0))
                    .show(ui, |ui| {
                        if self.loading_atlas {
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new().size(14.0));
                                ui.label(egui::RichText::new("loading atlas…").weak().size(11.0));
                            });
                        } else if let Some(p) = self.atlas_path.clone() {
                            let name = filename(&p);
                            if self.atlas.is_some() {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&name).color(ACCENT).strong(),
                                    )
                                    .truncate(),
                                )
                                .on_hover_text(&p);
                            } else {
                                ui.add(
                                    egui::Label::new(egui::RichText::new(&name).color(ERRC))
                                        .truncate(),
                                )
                                .on_hover_text(&p);
                            }
                            if let Some(e) = &self.atlas_error {
                                ui.label(egui::RichText::new(e).color(ERRC).size(11.0));
                            }
                        } else {
                            ui.label(
                                egui::RichText::new("No atlas selected").weak().size(12.0),
                            );
                            ui.label(
                                egui::RichText::new("Item icons need a brass_atlas.zip")
                                    .weak()
                                    .size(11.0),
                            );
                        }
                        let btn = if self.atlas.is_some() {
                            "Change atlas…"
                        } else {
                            "Select brass_atlas.zip…"
                        };
                        if ui.button(btn).clicked() {
                            atlas_clicked = true;
                        }
                    });

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.heading(egui::RichText::new("Files").color(ACCENT));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("+ Add").on_hover_text("Add files").clicked() {
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
                            .fill(rgb(0x21232b))
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
                                                .small_button("×")
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
        if atlas_clicked {
            self.pick_atlas();
        }
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
                let any_loading = self.loading_atlas || self.sources.iter().any(|s| s.loading);
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
        let mut pick_atlas = false;
        let mut animating = false;
        let gframe = ((ctx.input(|i| i.time) * 12.0) as usize) % GLINT_FRAMES;

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.atlas.is_none() {
                if let Some(err) = &self.error {
                    center_message(ui, "!", err, ERRC);
                } else if self.loading_atlas {
                    center_spinner(ui, "Loading item sprites…");
                } else {
                    pick_atlas = atlas_prompt(ui, self.atlas_error.as_deref());
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
                } else if !self.filters.text.trim().is_empty() {
                    center_message(
                        ui,
                        "?",
                        &format!(
                            "No matches for \"{}\" - try a broader term, OR, or clear filters.",
                            self.filters.text.trim()
                        ),
                        Color32::from_gray(150),
                    );
                } else {
                    center_message(
                        ui,
                        "?",
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
            let hl = self.highlight.as_ref();
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

            let cw = card_width(slot);
            let ncols = (((avail_w + spacing) / (cw + spacing)).floor() as usize)
                .clamp(1, filtered.len().max(1));
            let col_w = if ncols <= 1 {
                (avail_w - 8.0).max(cw)
            } else {
                (avail_w - spacing * (ncols as f32 - 1.0)) / ncols as f32
            };

            let mut col_y = vec![0.0f32; ncols];
            let mut placements: Vec<(f32, f32, f32)> = Vec::with_capacity(filtered.len());
            for &h in &heights {
                let ci = (0..ncols)
                    .min_by(|&a, &b| col_y[a].partial_cmp(&col_y[b]).unwrap_or(std::cmp::Ordering::Equal))
                    .unwrap_or(0);
                let x = ci as f32 * (col_w + spacing);
                let y = col_y[ci];
                placements.push((x, y, h));
                col_y[ci] = y + h + spacing;
            }
            let total = col_y.iter().cloned().fold(0.0f32, f32::max);

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show_viewport(ui, |ui, vp| {
                    ui.set_width(avail_w);
                    let (_id, block) = ui.allocate_space(Vec2::new(avail_w, total.max(1.0)));
                    let top = block.min.y;
                    for (k, &(si, ei)) in filtered.iter().enumerate() {
                        let (x, y, h) = placements[k];
                        let visible = !(y + h < vp.min.y - 300.0 || y > vp.max.y + 300.0);
                        if visible {
                            let rect = Rect::from_min_size(
                                Pos2::new(block.min.x + x, top + y),
                                Vec2::new(col_w, h),
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
                                    hl,
                                    profiles,
                                    &mut actions,
                                    &mut animating,
                                );
                            }
                        }
                    }
                });
        });

        if pick_atlas {
            self.pick_atlas();
        }

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
                "Drop file(s) to load",
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
                Action::OpenNested(title, items, uuid) => {
                    let level = self.open_nested(title, items, uuid);
                    self.popup.clear();
                    self.popup.push(level);
                }
            }
        }

        let mut nested_action: Option<Action> = None;
        if let (false, Some(atlas)) = (self.popup.is_empty(), self.atlas.as_mut()) {
            let slot = self.slot;
            let bp = &self.bp_index;
            let hl = self.highlight.as_ref();
            let profiles = &mut self.profiles;
            let depth = self.popup.len();
            let level = self.popup.last().unwrap();
            let title = level.title.clone();
            let items = &level.items;
            let count = items.len();
            let owner = level.owner.clone();
            let uuid = level.uuid.clone();
            let copies = level.copies.clone();

            let mut close = false;
            let mut back = false;
            let mut drill: Option<card::Drill> = None;

            egui::Window::new(egui::RichText::new(format!("{title}")).color(ACCENT))
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
                            if ui.button("× Close").clicked() {
                                close = true;
                            }
                        });
                    });
                    if owner.is_some() || uuid.is_some() {
                        if let Some(o) = &owner {
                            ui.label(
                                egui::RichText::new(format!("Owner: {o}"))
                                    .color(KIND_BACKPACK)
                                    .size(12.0),
                            );
                        }
                        if let Some(u) = &uuid {
                            ui.label(
                                egui::RichText::new(format!("UUID: {u}"))
                                    .weak()
                                    .size(11.0)
                                    .monospace(),
                            );
                        }
                        ui.horizontal(|ui| {
                            for c in &copies {
                                if ui.button(&c.label).clicked() {
                                    nested_action = Some(Action::Copy(c.value.clone()));
                                }
                            }
                        });
                    }
                    ui.separator();
                    egui::ScrollArea::vertical().max_height(440.0).show(ui, |ui| {
                        if let Some(d) =
                            nested_grid(ui, atlas, items, bp, hl, profiles, gframe, slot, &mut animating)
                        {
                            drill = Some(d);
                        }
                    });
                });

            if let Some((t, its, u)) = drill {
                let level = self.open_nested(t, its, u);
                self.popup.push(level);
            } else if back {
                self.popup.pop();
            } else if close {
                self.popup.clear();
            }
            ctx.request_repaint();
        }
        if let Some(Action::Copy(s)) = nested_action {
            ctx.copy_text(s);
            self.toast("Copied to clipboard");
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
        if self.loading_atlas || animating || self.sources.iter().any(|s| s.loading) {
            ctx.request_repaint();
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_settings();
    }
}

fn atlas_prompt(ui: &mut egui::Ui, err: Option<&str>) -> bool {
    let mut clicked = false;
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.3);
        ui.label(egui::RichText::new("Item sprites not loaded").size(20.0).strong());
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(
                "Pick your brass_atlas.zip to show item icons.\nYou can also select it from the Sprites panel on the left.",
            )
            .size(13.0)
            .weak(),
        );
        if let Some(e) = err {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(format!("Last attempt failed: {e}")).color(ERRC).size(12.0));
        }
        ui.add_space(14.0);
        if ui
            .add(
                egui::Button::new(
                    egui::RichText::new("Select brass_atlas.zip…").size(15.0).strong(),
                )
                .min_size(Vec2::new(220.0, 40.0))
                .fill(ACCENT.linear_multiply(0.85))
                .rounding(Rounding::same(9.0)),
            )
            .clicked()
        {
            clicked = true;
        }
    });
    clicked
}

fn search_help_ui(ui: &mut egui::Ui) {
    ui.add_space(4.0);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.label(egui::RichText::new("Search syntax").color(ACCENT).strong());
        egui::Grid::new("search_help_grid")
            .num_columns(2)
            .spacing([14.0, 4.0])
            .show(ui, |ui| {
                let row = |ui: &mut egui::Ui, ex: &str, desc: &str| {
                    ui.label(egui::RichText::new(ex).monospace().color(GOLD));
                    ui.label(egui::RichText::new(desc).size(12.0).weak());
                    ui.end_row();
                };
                row(ui, "diamond netherite", "both terms (space = AND)");
                row(ui, "diamond AND netherite", "both terms");
                row(ui, "diamond OR gold", "either term");
                row(ui, "NOT shulker", "exclude a term");
                row(ui, "\"diamond sword\"", "exact phrase");
                row(ui, "gold AND (sword OR axe)", "group with parentheses");
                row(ui, "netherite NOT (boots OR helmet)", "combine freely");
            });
        ui.label(
            egui::RichText::new(
                "Operators are case-insensitive. && and || also work. The category dropdown picks what the text matches against.",
            )
            .weak()
            .size(11.0),
        );
    });
}

fn source_color(s: &Source) -> Color32 {
    match s.store.as_ref().and_then(|st| st.first_kind()) {
        Some(EntryKind::Backpack) => KIND_BACKPACK,
        Some(EntryKind::Player) => KIND_PLAYER,
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
        ui.add_space(ui.available_height() * 0.18);
        ui.label(
            egui::RichText::new("Backpack Infiltrator")
                .size(30.0)
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
                egui::Button::new(egui::RichText::new("Load files…").size(15.0).strong())
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
            ("•", "sophisticatedbackpacks.dat", "backpacks - items, upgrades, enchants"),
            ("•", "*_container_dump.json", "world containers - chests, barrels, shulkers"),
            ("•", "*_player_dump.json", "players - inventory + ender chest"),
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
