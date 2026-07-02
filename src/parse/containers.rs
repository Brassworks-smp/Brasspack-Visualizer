
use rayon::prelude::*;
use serde_json::Value as J;

use crate::model::{prettify_id, CopyAction, Entry, EntryKind, Item};
use crate::parse::nbt::extract_text;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum JsonKind {
    Containers,
    Players,
}

pub fn load_json(path: &str, forced: Option<JsonKind>) -> Result<Vec<Entry>, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("read: {e}"))?;
    let root: J = serde_json::from_str(&raw).map_err(|e| format!("json parse: {e}"))?;

    let list: &[J] = match &root {
        J::Array(a) => a.as_slice(),
        J::Object(o) => match o
            .get("containers")
            .or_else(|| o.get("players"))
            .and_then(|v| v.as_array())
        {
            Some(a) => a.as_slice(),
            None => std::slice::from_ref(&root),
        },
        _ => return Err("unexpected JSON shape".into()),
    };

    let kind = forced.unwrap_or_else(|| detect_kind(list));
    let entries = match kind {
        JsonKind::Players => crate::parse::players::entries_from(list),
        JsonKind::Containers => list.par_iter().filter_map(parse_container).collect(),
    };
    Ok(entries)
}

pub fn load_containers(path: &str) -> Result<Vec<Entry>, String> {
    load_json(path, None)
}

fn detect_kind(list: &[J]) -> JsonKind {
    if let Some(J::Object(o)) = list.first() {
        if o.contains_key("ender_chest") || (o.contains_key("uuid") && o.contains_key("inventory")) {
            return JsonKind::Players;
        }
    }
    JsonKind::Containers
}

fn str_field(c: &J, key: &str) -> Option<String> {
    match c.get(key)? {
        J::String(s) => Some(s.clone()),
        J::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn parse_container(c: &J) -> Option<Entry> {
    let id = str_field(c, "id").unwrap_or_else(|| "minecraft:chest".into());
    let x = str_field(c, "x").unwrap_or_else(|| "?".into());
    let y = str_field(c, "y").unwrap_or_else(|| "?".into());
    let z = str_field(c, "z").unwrap_or_else(|| "?".into());
    let dimension = str_field(c, "dimension").unwrap_or_else(|| "minecraft:overworld".into());
    let is_dungeon = c
        .get("is_dungeon")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let items = parse_items(c.get("items"));

    let coords = match (
        c.get("x").and_then(coord),
        c.get("y").and_then(coord),
        c.get("z").and_then(coord),
    ) {
        (Some(x), Some(y), Some(z)) => Some((x, y, z)),
        _ => None,
    };
    let mut nbt_blob = String::new();
    if let Some(items_json) = c.get("items") {
        collect_nbt(items_json, &mut nbt_blob);
    }

    let meta = vec![
        ("Type".into(), id.clone()),
        ("Position".into(), format!("{}, {}, {}", x, y, z)),
        ("Dimension".into(), dimension.clone()),
        (
            "Dungeon".into(),
            if is_dungeon { "Yes".into() } else { "No".into() },
        ),
    ];

    let tp = format!("/execute in {} run tp @s {} {} {}", dimension, x, y, z);
    let copies = vec![
        CopyAction { label: "Copy TP".into(), value: tp },
        CopyAction {
            label: "Copy Coords".into(),
            value: format!("{} {} {}", x, y, z),
        },
        CopyAction {
            label: "Copy Dimension".into(),
            value: dimension.clone(),
        },
    ];

    let title = format!("{} @ {}, {}, {}", prettify_id(&id), x, y, z);

    let mut entry = Entry {
        kind: EntryKind::Container,
        title,
        header_icon: id.clone(),
        meta,
        copies,
        items,
        upgrades: Vec::new(),
        cols: 9,
        rows: 0,
        is_dungeon,
        dimension,
        owner: String::new(),
        uuid: String::new(),
        coords,
        search_blob: String::new(),
        nbt_blob: nbt_blob.to_lowercase(),
        max_stack: 0,
        all_enchants: Vec::new(),
    };
    let extra = format!("{} {} {} {}", id, x, y, z);
    entry.finalize(&extra);
    Some(entry)
}

fn parse_items(v: Option<&J>) -> Vec<Item> {
    let mut out = Vec::new();
    match v {
        Some(J::Array(a)) => {
            for (i, it) in a.iter().enumerate() {
                if let Some(item) = item_from_json(it, i as i32) {
                    out.push(item);
                }
            }
        }
        Some(J::Object(o)) => {
            for (k, it) in o {
                let slot = k.parse::<i32>().unwrap_or(-1);
                if let Some(item) = item_from_json(it, slot) {
                    out.push(item);
                }
            }
        }
        _ => {}
    }
    out
}

fn with_object(v: &J, mut f: impl FnMut(&J)) {
    match v {
        J::Object(_) => f(v),
        J::String(s) => {
            if let Some(parsed) = crate::parse::snbt::parse(s) {
                if parsed.is_object() {
                    f(&parsed);
                }
            }
        }
        _ => {}
    }
}

fn num(v: Option<&J>) -> Option<i64> {
    v.and_then(|x| x.as_i64().or_else(|| x.as_f64().map(|f| f as i64)))
}

fn coord(v: &J) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_f64().map(|f| f as i64))
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

pub(crate) fn collect_nbt(v: &J, out: &mut String) {
    match v {
        J::Object(o) => {
            for (k, val) in o {
                if matches!(k.as_str(), "nbt" | "tag" | "components") {
                    if let Some(s) = val.as_str() {
                        out.push_str(s);
                        out.push('\n');
                        continue;
                    }
                }
                collect_nbt(val, out);
            }
        }
        J::Array(a) => a.iter().for_each(|e| collect_nbt(e, out)),
        _ => {}
    }
}

pub(crate) fn item_from_json(v: &J, default_slot: i32) -> Option<Item> {
    let id = v
        .get("id")
        .and_then(|x| x.as_str())
        .map(|s| s.to_lowercase())?;
    if id.is_empty() || id.contains("air") {
        return None;
    }
    let count = num(v.get("count")).or_else(|| num(v.get("Count"))).unwrap_or(1);
    let slot = num(v.get("Slot"))
        .or_else(|| num(v.get("slot")))
        .map(|x| x as i32)
        .unwrap_or(default_slot);

    let mut item = Item {
        id,
        count,
        slot,
        ..Default::default()
    };

    if let Some(c) = v.get("components") {
        with_object(c, |obj| apply_components(&mut item, obj));
    }
    if let Some(t) = v.get("tag").or_else(|| v.get("nbt")) {
        with_object(t, |obj| {
            apply_components(&mut item, obj);
            apply_legacy_tag(&mut item, obj);
        });
    }
    Some(item)
}

fn apply_components(item: &mut Item, comps: &J) {
    if let Some(s) = comps.get("minecraft:custom_name").and_then(|x| x.as_str()) {
        item.custom_name = Some(extract_text(s));
    }
    if item.custom_name.is_none() {
        if let Some(s) = comps.get("minecraft:item_name").and_then(|x| x.as_str()) {
            item.custom_name = Some(extract_text(s));
        }
    }
    if let Some(l) = comps.get("minecraft:lore").and_then(|x| x.as_array()) {
        item.lore = l
            .iter()
            .filter_map(|e| e.as_str())
            .map(extract_text)
            .filter(|s| !s.is_empty())
            .collect();
    }
    read_json_enchants(comps.get("minecraft:enchantments"), &mut item.enchants);
    read_json_enchants(comps.get("minecraft:stored_enchantments"), &mut item.enchants);
    if let Some(d) = num(comps.get("minecraft:damage")) {
        item.damage = Some(d as i32);
    }
    for key in crate::model::NESTED_KEYS {
        if let Some(cv) = comps.get(*key) {
            extract_nested(cv, &mut item.contents);
        }
    }
    if let Some(u) = comps
        .get("sophisticatedcore:storage_uuid")
        .and_then(uuid_from_json)
    {
        item.storage_uuid = Some(u.to_lowercase());
    }
    if item.is_player_head() {
        if let Some(prof) = comps.get("minecraft:profile") {
            extract_skull_modern(item, prof);
        }
    }
}

fn extract_skull_modern(item: &mut Item, prof: &J) {
    if let Some(s) = prof.as_str() {
        item.head_ref = Some(s.to_string());
        return;
    }
    if let Some(props) = prof.get("properties").and_then(|p| p.as_array()) {
        for p in props {
            if p.get("name").and_then(|n| n.as_str()) == Some("textures") {
                if let Some(v) = p.get("value").and_then(|v| v.as_str()) {
                    if let Some(url) = crate::model::skin_url_from_textures_value(v) {
                        item.head_skin = Some(url);
                        return;
                    }
                }
            }
        }
    }
    if let Some(u) = prof.get("id").and_then(uuid_from_json) {
        item.head_ref = Some(u.to_lowercase());
    } else if let Some(n) = prof.get("name").and_then(|n| n.as_str()) {
        item.head_ref = Some(n.to_string());
    }
}

fn extract_skull_legacy(item: &mut Item, owner: &J) {
    if let Some(s) = owner.as_str() {
        item.head_ref = Some(s.to_string());
        return;
    }
    if let Some(v) = owner
        .get("Properties")
        .and_then(|p| p.get("textures"))
        .and_then(|t| t.as_array())
        .and_then(|a| a.first())
        .and_then(|e| e.get("Value"))
        .and_then(|v| v.as_str())
    {
        if let Some(url) = crate::model::skin_url_from_textures_value(v) {
            item.head_skin = Some(url);
            return;
        }
    }
    if let Some(u) = owner.get("Id").and_then(uuid_from_json) {
        item.head_ref = Some(u.to_lowercase());
    } else if let Some(id) = owner.get("Id").and_then(|x| x.as_str()) {
        item.head_ref = Some(id.to_lowercase());
    } else if let Some(n) = owner.get("Name").and_then(|x| x.as_str()) {
        item.head_ref = Some(n.to_string());
    }
}

fn extract_nested(v: &J, out: &mut Vec<Item>) {
    match v {
        J::Array(l) => {
            for e in l {
                if let Some(inner) = e.get("item") {
                    let slot = num(e.get("slot")).map(|x| x as i32);
                    if let Some(it) = item_from_json(inner, slot.unwrap_or(out.len() as i32)) {
                        out.push(it);
                    }
                } else if e.get("id").is_some() {
                    if let Some(it) = item_from_json(e, out.len() as i32) {
                        out.push(it);
                    }
                }
            }
        }
        J::Object(_) => {
            if let Some(items) = v.get("items") {
                extract_nested(items, out);
            }
        }
        _ => {}
    }
}

fn uuid_from_json(v: &J) -> Option<String> {
    let arr = v.as_array()?;
    let ints: Vec<i32> = arr.iter().filter_map(|x| x.as_i64().map(|n| n as i32)).collect();
    crate::parse::nbt::uuid_from_ints(&ints)
}

fn apply_legacy_tag(item: &mut Item, tag: &J) {
    if let Some(display) = tag.get("display") {
        if let Some(name) = display.get("Name").and_then(|x| x.as_str()) {
            item.custom_name = Some(extract_text(name));
        }
        if let Some(lore) = display.get("Lore").and_then(|x| x.as_array()) {
            item.lore = lore
                .iter()
                .filter_map(|e| e.as_str())
                .map(extract_text)
                .filter(|s| !s.is_empty())
                .collect();
        }
    }
    read_legacy_ench(tag.get("Enchantments"), &mut item.enchants);
    read_legacy_ench(tag.get("StoredEnchantments"), &mut item.enchants);
    if let Some(d) = num(tag.get("Damage")) {
        item.damage = Some(d as i32);
    }
    if let Some(list) = tag
        .get("BlockEntityTag")
        .and_then(|b| b.get("Items"))
        .and_then(|x| x.as_array())
    {
        for (i, e) in list.iter().enumerate() {
            if let Some(it) = item_from_json(e, i as i32) {
                item.contents.push(it);
            }
        }
    }
    if item.is_player_head() && item.head_key().is_none() {
        if let Some(owner) = tag.get("SkullOwner") {
            extract_skull_legacy(item, owner);
        }
    }
}

fn read_json_enchants(v: Option<&J>, out: &mut Vec<(String, i32)>) {
    let Some(v) = v else { return };
    let levels = v.get("levels").unwrap_or(v);
    if let Some(map) = levels.as_object() {
        for (k, lvl) in map {
            if let Some(l) = lvl.as_i64() {
                out.push((k.clone(), l as i32));
            }
        }
    }
}

fn read_legacy_ench(v: Option<&J>, out: &mut Vec<(String, i32)>) {
    if let Some(arr) = v.and_then(|x| x.as_array()) {
        for e in arr {
            let id = e.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let lvl = num(e.get("lvl")).unwrap_or(1) as i32;
            if !id.is_empty() {
                out.push((id, lvl));
            }
        }
    }
}
