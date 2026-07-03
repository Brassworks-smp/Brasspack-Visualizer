
#[derive(Clone, Copy, Debug)]
pub struct Bar {
    pub frac: f32,
    pub color: [u8; 3],
}

#[derive(Clone, Debug, Default)]
pub struct Item {
    pub id: String,
    pub count: i64,
    pub slot: i32,
    pub custom_name: Option<String>,
    pub lore: Vec<String>,
    pub enchants: Vec<(String, i32)>,
    pub damage: Option<i32>,
    pub max_damage: Option<i32>,
    pub potion: Option<String>,
    pub contents: Vec<Item>,
    pub storage_uuid: Option<String>,
    pub head_skin: Option<String>,
    pub head_ref: Option<String>,
    pub bar: Option<Bar>,
    pub outline: Option<[u8; 3]>,
    pub gauge_text: Option<String>,
}

pub const SMALL_CAP: i64 = 800;
pub const MEDIUM_CAP: i64 = 1600;
pub const LARGE_CAP: i64 = 3200;

pub const BACKTANK_MAX_AIR: i64 = 1800;

use crate::color::rgb3;

const FILLING_COLOR: [u8; 3] = rgb3(0x7997d9);
const FUELING_COLOR: [u8; 3] = rgb3(0xffa565);
const BACKTANK_COLOR: [u8; 3] = rgb3(0xffffff);

pub const NESTED_KEYS: &[&str] = &[
    "minecraft:container",
    "minecraft:bundle_contents",
    "minecraft:charged_projectiles",
    "create:package_contents",
    "cmpackagecouriers:plane_package",
    "supplementaries:quiver_content",
    "supplementaries:lunch_basket_content",
];

impl Item {
    pub fn display_name(&self) -> String {
        if let Some(n) = &self.custom_name {
            if !n.is_empty() {
                return n.clone();
            }
        }
        prettify_id(&self.id)
    }

    pub fn is_player_head(&self) -> bool {
        self.id.contains("player_head") || self.id.contains("player_wall_head")
    }

    pub fn head_key(&self) -> Option<&str> {
        if !self.is_player_head() {
            return None;
        }
        self.head_skin.as_deref().or(self.head_ref.as_deref())
    }

    fn tank_capacity(&self) -> i64 {
        let id = self.id.as_str();
        if id.contains("small") {
            SMALL_CAP
        } else if id.contains("large") {
            LARGE_CAP
        } else {
            MEDIUM_CAP
        }
    }

    pub fn apply_gauges(&mut self, stock: Option<i64>, air: Option<i64>, capacity_ench: Option<i32>) {
        let id = self.id.to_lowercase();

        if id.contains("backtank") {
            self.outline = Some(BACKTANK_COLOR);
            if let Some(a) = air {
                let max = BACKTANK_MAX_AIR + capacity_ench.unwrap_or(0).max(0) as i64 * BACKTANK_MAX_AIR;
                let frac = (a as f32 / max as f32).clamp(0.0, 1.0);
                self.bar = Some(Bar { frac, color: BACKTANK_COLOR });
                self.gauge_text = Some(format!("Air: {a} / {max}"));
            }
            return;
        }

        if id.contains("tank") {
            let (color, kind) = if id.contains("fueling") {
                (Some(FUELING_COLOR), "Fuel")
            } else if id.contains("filling") {
                (Some(FILLING_COLOR), "Fluid")
            } else {
                (None, "Fluid")
            };
            if let (Some(color), Some(s)) = (color, stock) {
                let cap = self.tank_capacity();
                let frac = (s as f32 / cap as f32).clamp(0.0, 1.0);
                self.bar = Some(Bar { frac, color });
                self.gauge_text = Some(format!("{kind}: {s} / {cap} mB"));
                return;
            }
        }

        if let Some(dmg) = self.damage {
            let max = self.max_damage.filter(|m| *m > 0).or_else(|| vanilla_max_damage(&id));
            if let Some(max) = max {
                if max > 0 && dmg > 0 {
                    let frac = ((max - dmg) as f32 / max as f32).clamp(0.0, 1.0);
                    self.bar = Some(Bar { frac, color: durability_rgb(frac) });
                }
            }
        }
    }

    pub fn max_count(&self) -> i64 {
        let mut m = self.count;
        for c in &self.contents {
            m = m.max(c.max_count());
        }
        m
    }

    pub fn collect_enchants(&self, out: &mut Vec<(String, i32)>) {
        out.extend(self.enchants.iter().cloned());
        for c in &self.contents {
            c.collect_enchants(out);
        }
    }

    pub fn append_search(&self, out: &mut String) {
        out.push_str(&self.id.to_lowercase());
        out.push('\n');
        if let Some(n) = &self.custom_name {
            out.push_str(&n.to_lowercase());
            out.push('\n');
        }
        for l in &self.lore {
            out.push_str(&l.to_lowercase());
            out.push('\n');
        }
        for (e, _) in &self.enchants {
            out.push_str(&e.to_lowercase());
            out.push('\n');
        }
        if let Some(r) = &self.head_ref {
            out.push_str(&r.to_lowercase());
            out.push('\n');
        }
        for c in &self.contents {
            c.append_search(out);
        }
    }
}

pub fn skin_url_from_textures_value(b64: &str) -> Option<String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let url = json["textures"]["SKIN"]["url"].as_str()?;
    Some(match url.strip_prefix("http://") {
        Some(rest) => format!("https://{rest}"),
        None => url.to_string(),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EntryKind {
    Backpack,
    #[default]
    Container,
    Player,
}

#[derive(Clone, Debug)]
pub struct CopyAction {
    pub label: String,
    pub value: String,
}

#[derive(Clone, Debug, Default)]
pub struct Entry {
    pub kind: EntryKind,
    pub title: String,
    pub header_icon: String,
    pub meta: Vec<(String, String)>,
    pub copies: Vec<CopyAction>,
    pub items: Vec<Item>,
    pub upgrades: Vec<Item>,
    pub cols: usize,
    pub rows: usize,
    pub is_dungeon: bool,
    pub dimension: String,
    pub owner: String,
    pub uuid: String,
    pub coords: Option<(i64, i64, i64)>,
    pub search_blob: String,
    pub nbt_blob: String,
    pub max_stack: i64,
    pub all_enchants: Vec<(String, i32)>,
}

impl Entry {
    pub fn finalize(&mut self, extra_search: &str) {
        let max_slot = self.items.iter().map(|i| i.slot).max().unwrap_or(-1);
        if self.cols == 0 {
            self.cols = 9;
        }
        self.rows = ((max_slot as i64 / self.cols as i64) + 1).max(1) as usize;

        let mut blob = extra_search.to_lowercase();
        blob.push('\n');
        for it in &self.items {
            it.append_search(&mut blob);
        }
        for it in &self.upgrades {
            it.append_search(&mut blob);
        }
        self.search_blob = blob;

        let mut ench = Vec::new();
        let mut max_stack = 0;
        for it in self.items.iter().chain(self.upgrades.iter()) {
            it.collect_enchants(&mut ench);
            max_stack = max_stack.max(it.max_count());
        }
        self.all_enchants = ench;
        self.max_stack = max_stack;
    }
}

fn vanilla_max_damage(id: &str) -> Option<i32> {
    let n = id.rsplit(':').next().unwrap_or(id);

    let tool_material = |suffix: &str| -> Option<i32> {
        n.strip_suffix(suffix).map(|p| p.trim_end_matches('_')).and_then(|m| match m {
            "wooden" => Some(59),
            "stone" => Some(131),
            "iron" => Some(250),
            "golden" => Some(32),
            "diamond" => Some(1561),
            "netherite" => Some(2031),
            _ => None,
        })
    };
    for suffix in ["_pickaxe", "_axe", "_shovel", "_hoe", "_sword"] {
        if let Some(d) = tool_material(suffix) {
            return Some(d);
        }
    }

    let armor = |slot_base: i32| -> Option<i32> {
        let mat = if n.starts_with("leather_") {
            5
        } else if n.starts_with("chainmail_") || n.starts_with("iron_") {
            15
        } else if n.starts_with("golden_") {
            7
        } else if n.starts_with("diamond_") {
            33
        } else if n.starts_with("netherite_") {
            37
        } else {
            return None;
        };
        Some(slot_base * mat)
    };
    if n.ends_with("_helmet") {
        return armor(11);
    }
    if n.ends_with("_chestplate") {
        return armor(16);
    }
    if n.ends_with("_leggings") {
        return armor(15);
    }
    if n.ends_with("_boots") {
        return armor(13);
    }

    Some(match n {
        "turtle_helmet" => 275,
        "bow" => 384,
        "crossbow" => 465,
        "trident" => 250,
        "shield" => 336,
        "elytra" => 432,
        "fishing_rod" => 64,
        "flint_and_steel" => 64,
        "shears" => 238,
        "carrot_on_a_stick" => 25,
        "warped_fungus_on_a_stick" => 100,
        "brush" => 64,
        "mace" => 500,
        _ => return None,
    })
}

const DURABILITY_LUT: [[u8; 3]; 256] = build_durability_lut();

const fn build_durability_lut() -> [[u8; 3]; 256] {
    let mut lut = [[0u8; 3]; 256];
    let mut i = 0;
    while i < 256 {
        lut[i] = if i < 128 {
            [255, (i * 2) as u8, 0]
        } else {
            [(510 - i * 2) as u8, 255, 0]
        };
        i += 1;
    }
    lut
}

pub fn durability_rgb(frac: f32) -> [u8; 3] {
    DURABILITY_LUT[(frac.clamp(0.0, 1.0) * 255.0) as usize]
}

pub fn prettify_id(id: &str) -> String {
    let raw = id.rsplit(':').next().unwrap_or(id);
    let mut out = String::new();
    for (i, word) in raw.split('_').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let mut ch = word.chars();
        if let Some(f) = ch.next() {
            out.extend(f.to_uppercase());
            out.push_str(ch.as_str());
        }
    }
    out
}

pub fn format_short_date(ms: i64) -> String {
    if ms <= 0 {
        return "Never".into();
    }
    let secs = ms / 1000;
    let days = secs / 86400;
    let rem = secs % 86400;
    let (h, m) = (rem / 3600, (rem % 3600) / 60);
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mon = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if mon <= 2 { y + 1 } else { y };
    format!(
        "{:02}-{:02}-{:02} {:02}:{:02}",
        year % 100,
        mon,
        d,
        h,
        m
    )
}

pub fn format_count(count: i64) -> String {
    if count < 10000 {
        return count.to_string();
    }
    let mut n = count as f64;
    for suffix in ["k", "M", "B", "T"] {
        n /= 1000.0;
        if n < 1000.0 {
            if n >= 10.0 {
                return format!("{}{}", n as i64, suffix);
            }
            let s = format!("{:.1}", n);
            return format!("{}{}", s.trim_end_matches(".0"), suffix);
        }
    }
    "INF".into()
}
