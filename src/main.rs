#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod model;
mod parse;
mod profiles;
mod render;
mod search;
mod settings;
mod store;
mod ui;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(pos) = args.iter().position(|a| a == "--parse") {
        if let Some(path) = args.get(pos + 1) {
            parse_report(path);
            return Ok(());
        }
    }
    if let Some(pos) = args.iter().position(|a| a == "--store-mem") {
        if let Some(path) = args.get(pos + 1) {
            let t = std::time::Instant::now();
            let s = store::Store::open(path, store::Load::auto(path)).expect("store");
            println!("store opened: {} entries in {:?}", s.len(), t.elapsed());
            return Ok(());
        }
    }
    if let Some(pos) = args.iter().position(|a| a == "--verify-store") {
        if let Some(path) = args.get(pos + 1) {
            verify_store(path);
            return Ok(());
        }
    }
    if let Some(pos) = args.iter().position(|a| a == "--png") {
        if let Some(path) = args.get(pos + 1) {
            png_report(path, args.get(pos + 2).map(String::as_str));
            return Ok(());
        }
    }
    if let Some(pos) = args.iter().position(|a| a == "--head") {
        if let Some(skin) = args.get(pos + 1) {
            let out = args.get(pos + 2).map(String::as_str).unwrap_or("/tmp/head.png");
            let size = args.get(pos + 3).and_then(|s| s.parse().ok()).unwrap_or(128);
            let bytes = std::fs::read(skin).expect("read skin");
            let head = render::head3d::render_from_bytes(&bytes, size).expect("render head");
            head.save(out).expect("save");
            println!("wrote {out} ({size}x{size})");
            return Ok(());
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 860.0])
            .with_min_inner_size([820.0, 560.0])
            .with_title("Backpack Infiltrator"),
        ..Default::default()
    };

    let initial: Vec<String> = args
        .iter()
        .skip(1)
        .filter(|a| !a.starts_with("--"))
        .cloned()
        .collect();

    eframe::run_native(
        "Backpack Infiltrator",
        options,
        Box::new(move |cc| {
            install_theme(&cc.egui_ctx);
            egui_extras::install_image_loaders(&cc.egui_ctx);
            let mut app = ui::App::new(cc.egui_ctx.clone());
            for path in initial {
                app.request_open(path);
            }
            Ok(Box::new(app))
        }),
    )
}

fn parse_report(path: &str) {
    let t = std::time::Instant::now();
    let is_json = path.to_lowercase().ends_with(".json");
    let res = if is_json {
        parse::containers::load_containers(path)
    } else {
        parse::nbt::load_backpacks(path)
    };
    match res {
        Ok(entries) => {
            let items: usize = entries.iter().map(|e| e.items.len()).sum();
            let with_ench = entries.iter().filter(|e| !e.all_enchants.is_empty()).count();
            let max_lvl = entries
                .iter()
                .flat_map(|e| e.all_enchants.iter())
                .map(|(_, l)| *l)
                .max()
                .unwrap_or(0);
            let nested = entries
                .iter()
                .flat_map(|e| e.items.iter())
                .filter(|i| !i.contents.is_empty())
                .count();
            let heads = entries
                .iter()
                .flat_map(|e| e.items.iter())
                .filter(|i| i.head_key().is_some())
                .count();
            let head_skins = entries
                .iter()
                .flat_map(|e| e.items.iter())
                .filter(|i| i.head_skin.is_some())
                .count();
            println!(
                "parsed {} entries, {} items, {} entries-with-enchants, max_lvl={}, {} nested-container items, {} player-heads ({} with inline skin) in {:?}",
                entries.len(),
                items,
                with_ench,
                max_lvl,
                nested,
                heads,
                head_skins,
                t.elapsed()
            );
        }
        Err(e) => eprintln!("error: {e}"),
    }
}

fn verify_store(path: &str) {
    use search::{DungeonFilter, EnchOp, Filters, TextCat};
    let is_backpack = !path.to_lowercase().ends_with(".json");
    let eager = if is_backpack {
        parse::nbt::load_backpacks(path)
    } else {
        parse::containers::load_json(path, None)
    }
    .expect("eager parse");
    let store = store::Store::open(path, store::Load::auto(path)).expect("store open");

    println!("counts: eager={} store={}", eager.len(), store.len());
    if eager.len() != store.len() {
        eprintln!("COUNT MISMATCH");
    }

    let mut cases: Vec<(&str, Filters)> = Vec::new();
    let mut f = Filters::default();
    cases.push(("default", f.clone()));
    f = Filters::default();
    f.text = "diamond".into();
    cases.push(("text=diamond", f.clone()));
    f = Filters::default();
    f.text = "netherite".into();
    f.cat = TextCat::Item;
    cases.push(("item-text=netherite", f.clone()));
    f = Filters::default();
    f.item = "shulker".into();
    cases.push(("item=shulker", f.clone()));
    f = Filters::default();
    f.nbt = "enchant".into();
    cases.push(("nbt=enchant", f.clone()));
    f = Filters::default();
    f.dungeon = DungeonFilter::Only;
    cases.push(("dungeon-only", f.clone()));
    f = Filters::default();
    f.dimension = "nether".into();
    cases.push(("dim=nether", f.clone()));
    f = Filters::default();
    f.ctype = "chest".into();
    cases.push(("ctype=chest", f.clone()));
    f = Filters::default();
    f.min_count = "65".into();
    cases.push(("min_count=65", f.clone()));
    f = Filters::default();
    f.ench_op = EnchOp::Gte;
    f.ench_level = 5;
    cases.push(("ench>=5", f.clone()));
    f = Filters::default();
    f.x_min = "0".into();
    f.x_max = "500".into();
    cases.push(("x=0..500", f.clone()));
    f = Filters::default();
    f.player = "e".into();
    cases.push(("player~e", f.clone()));

    for (name, filt) in &cases {
        let c = filt.compile();
        let eager_hits: Vec<u32> = eager
            .iter()
            .enumerate()
            .filter(|(_, e)| c.matches(e))
            .map(|(i, _)| i as u32)
            .collect();
        let mut store_hits = store.filter(&c);
        store_hits.sort_unstable();
        let mismatch = eager_hits != store_hits;
        println!(
            "  {:16} eager={:>7} store={:>7} {}",
            name,
            eager_hits.len(),
            store_hits.len(),
            if mismatch { "<<< MISMATCH" } else { "ok" }
        );
        if mismatch {
            let ea: std::collections::HashSet<u32> = eager_hits.iter().copied().collect();
            let st: std::collections::HashSet<u32> = store_hits.iter().copied().collect();
            let only_eager: Vec<u32> = eager_hits.iter().copied().filter(|i| !st.contains(i)).take(3).collect();
            let only_store: Vec<u32> = store_hits.iter().copied().filter(|i| !ea.contains(i)).take(3).collect();
            println!("      only_eager={:?} only_store={:?}", only_eager, only_store);
        }
    }

    let mut checked = 0;
    for i in (0..eager.len()).step_by((eager.len() / 50).max(1)) {
        let m = store.entry(i).expect("materialize");
        let e = &eager[i];
        if m.title != e.title || m.coords != e.coords || m.items.len() != e.items.len() {
            eprintln!(
                "  ENTRY MISMATCH i={i}: title {:?}/{:?} coords {:?}/{:?} items {}/{}",
                m.title, e.title, m.coords, e.coords, m.items.len(), e.items.len()
            );
        }
        checked += 1;
    }
    println!("materialize spot-checks: {checked} ok");
}

fn png_report(path: &str, out: Option<&str>) {
    let is_json = path.to_lowercase().ends_with(".json");
    let entries = if is_json {
        parse::containers::load_containers(path)
    } else {
        parse::nbt::load_backpacks(path)
    }
    .expect("parse");
    let dir = render::atlas::assets_dir();
    let atlas = render::atlas::Atlas::load(&dir).expect("atlas");
    let font = render::export::McFont::load(&dir).expect("font");
    let idx = entries
        .iter()
        .enumerate()
        .max_by_key(|(_, e)| e.items.len())
        .map(|(i, _)| i)
        .unwrap_or(0);
    let img = render::export::render_entry(&entries[idx], &atlas, &font);
    let out = out.unwrap_or("/tmp/infiltrator_sample.png");
    img.save(out).expect("save png");
    println!("wrote {out} ({}x{}) from entry {idx}", img.width(), img.height());
}

fn install_theme(ctx: &egui::Context) {
    use egui::{Color32, Rounding, Stroke};
    let mut visuals = egui::Visuals::dark();

    let bg = Color32::from_rgb(24, 25, 30);
    let panel = Color32::from_rgb(30, 32, 39);
    let accent = Color32::from_rgb(86, 171, 96);

    visuals.override_text_color = Some(Color32::from_rgb(224, 226, 232));
    visuals.panel_fill = panel;
    visuals.window_fill = bg;
    visuals.extreme_bg_color = Color32::from_rgb(18, 19, 23);
    visuals.faint_bg_color = Color32::from_rgb(38, 40, 48);
    visuals.hyperlink_color = accent;
    visuals.selection.bg_fill = accent.linear_multiply(0.55);
    visuals.selection.stroke = Stroke::new(1.0, accent);

    let r = Rounding::same(7.0);
    visuals.widgets.noninteractive.rounding = r;
    visuals.widgets.inactive.rounding = r;
    visuals.widgets.hovered.rounding = r;
    visuals.widgets.active.rounding = r;
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(46, 49, 58);
    visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(46, 49, 58);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(58, 62, 73);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(58, 62, 73);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, accent.linear_multiply(0.7));
    visuals.widgets.active.bg_fill = accent.linear_multiply(0.8);

    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    style.interaction.tooltip_delay = 0.0;
    style.interaction.tooltip_grace_time = 0.2;
    style.interaction.show_tooltips_only_when_still = false;
    ctx.set_style(style);
}
