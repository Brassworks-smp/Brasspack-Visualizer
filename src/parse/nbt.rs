
use std::collections::HashMap;
use std::io::Read;

use fastnbt::Value;
use rayon::prelude::*;

use crate::model::{format_short_date, Entry, EntryKind, Item};

pub(crate) fn comp(v: &Value) -> Option<&HashMap<String, Value>> {
    match v {
        Value::Compound(m) => Some(m),
        _ => None,
    }
}

pub(crate) fn get<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    comp(v)?.get(key)
}

pub(crate) fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Byte(x) => Some(*x as i64),
        Value::Short(x) => Some(*x as i64),
        Value::Int(x) => Some(*x as i64),
        Value::Long(x) => Some(*x),
        _ => None,
    }
}

fn as_num(v: &Value) -> Option<i64> {
    as_i64(v).or_else(|| match v {
        Value::Float(f) => Some(*f as i64),
        Value::Double(d) => Some(*d as i64),
        _ => None,
    })
}

// Recursively find the first numeric value stored under `key` (case-insensitive).
pub(crate) fn find_num(v: &Value, key: &str) -> Option<i64> {
    match v {
        Value::Compound(m) => {
            for (k, val) in m {
                if k.eq_ignore_ascii_case(key) {
                    if let Some(n) = as_num(val) {
                        return Some(n);
                    }
                }
            }
            m.values().find_map(|val| find_num(val, key))
        }
        Value::List(l) => l.iter().find_map(|e| find_num(e, key)),
        _ => None,
    }
}

pub(crate) fn as_str(v: &Value) -> Option<&str> {
    match v {
        Value::String(s) => Some(s.as_str()),
        _ => None,
    }
}

pub(crate) fn as_list(v: &Value) -> Option<&Vec<Value>> {
    match v {
        Value::List(l) => Some(l),
        _ => None,
    }
}

fn int_array(v: &Value) -> Option<Vec<i32>> {
    match v {
        Value::IntArray(a) => Some(a.iter().copied().collect()),
        Value::List(l) => {
            let mut out = Vec::new();
            for e in l {
                out.push(as_i64(e)? as i32);
            }
            Some(out)
        }
        _ => None,
    }
}

pub fn load_backpacks(path: &str) -> Result<Vec<Entry>, String> {
    let raw = std::fs::read(path).map_err(|e| format!("read: {e}"))?;
    let bytes = decompress(&raw);
    load_backpacks_bytes(&bytes)
}

pub(crate) fn load_backpacks_bytes(bytes: &[u8]) -> Result<Vec<Entry>, String> {
    let root: Value = fastnbt::from_bytes(bytes).map_err(|e| format!("nbt parse: {e}"))?;

    let payload = find_payload(&root, 0).ok_or("could not locate 'backpackContents' in NBT")?;

    let owners = build_owner_index(payload);
    let empty = Vec::new();
    let contents = get(payload, "backpackContents")
        .and_then(as_list)
        .unwrap_or(&empty);

    let entries = contents
        .par_iter()
        .filter_map(|bc| parse_backpack(bc, &owners))
        .collect();
    Ok(entries)
}

pub(crate) fn decompress(raw: &[u8]) -> Vec<u8> {
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

fn find_payload(v: &Value, depth: usize) -> Option<&Value> {
    if depth > 5 {
        return None;
    }
    if let Some(m) = comp(v) {
        if m.contains_key("backpackContents") {
            return Some(v);
        }
        for child in m.values() {
            if let Some(found) = find_payload(child, depth + 1) {
                return Some(found);
            }
        }
    }
    None
}

#[derive(Default, Clone)]
struct Owner {
    player: String,
    access: i64,
    name: String,
    registry: String,
}

fn build_owner_index(payload: &Value) -> HashMap<String, Owner> {
    let mut idx = HashMap::new();
    let empty = Vec::new();
    let log = get(payload, "accessLogRecords")
        .and_then(as_list)
        .unwrap_or(&empty);
    for rec in log {
        let uuid = nbt_get!(rec, "backpackUuid" | "uuid" => int_array)
            .and_then(|a| uuid_from_ints(&a));
        if let Some(uuid) = uuid {
            idx.insert(
                uuid,
                Owner {
                    player: get(rec, "playerName")
                        .and_then(as_str)
                        .unwrap_or("")
                        .to_string(),
                    access: get(rec, "accessTime").and_then(as_i64).unwrap_or(0),
                    name: get(rec, "backpackName")
                        .and_then(as_str)
                        .unwrap_or("")
                        .to_string(),
                    registry: get(rec, "backpackItemRegistryName")
                        .and_then(as_str)
                        .unwrap_or("")
                        .to_string(),
                },
            );
        }
    }
    idx
}

pub(crate) fn uuid_from_ints(ints: &[i32]) -> Option<String> {
    if ints.len() != 4 {
        return None;
    }
    let p: Vec<u64> = ints.iter().map(|x| (*x as u32) as u64).collect();
    let combined = ((p[0] << 32 | p[1]) as u128) << 64 | (p[2] << 32 | p[3]) as u128;
    let hex = format!("{:032x}", combined);
    Some(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

fn parse_backpack(bc: &Value, owners: &HashMap<String, Owner>) -> Option<Entry> {
    let uuid = nbt_get!(bc, "uuid" | "backpackUuid" => int_array)
        .and_then(|a| uuid_from_ints(&a))?;

    let contents = get(bc, "contents");
    let items = contents
        .and_then(|c| get(c, "inventory"))
        .map(|inv| items_from_inv(inv))
        .unwrap_or_default();
    let upgrades = contents
        .and_then(|c| get(c, "upgradeInventory"))
        .map(|inv| items_from_inv(inv))
        .unwrap_or_default();

    let owner = owners.get(&uuid).cloned().unwrap_or_default();
    let header_icon = if owner.registry.is_empty() {
        "sophisticatedbackpacks:backpack".to_string()
    } else {
        owner.registry.clone()
    };

    let player = if owner.player.is_empty() {
        "Unknown".to_string()
    } else {
        owner.player.clone()
    };

    let mut meta = meta![
        "Owner" => player.clone(),
        "Last access" => format_short_date(owner.access),
        "UUID" => uuid.clone(),
    ];
    if !owner.name.is_empty() {
        meta.insert(1, ("Backpack".into(), owner.name.clone()));
    }

    let copies = copies![
        "Copy Player" => player.clone(),
        "Copy UUID" => uuid.clone(),
        "Copy Registry" => header_icon.clone(),
    ];

    let title = format!("{} - {}", player, crate::model::prettify_id(&header_icon));

    let mut entry = Entry {
        kind: EntryKind::Backpack,
        title,
        header_icon,
        meta,
        copies,
        items,
        upgrades,
        owner: player.to_lowercase(),
        uuid: uuid.to_lowercase(),
        ..Default::default()
    };
    let extra = format!("{} {} {}", player, uuid, owner.name);
    entry.finalize(&extra);
    Some(entry)
}

fn items_from_inv(inv: &Value) -> Vec<Item> {
    let empty = Vec::new();
    let items = get(inv, "Items").and_then(as_list).unwrap_or(&empty);
    let mut out = Vec::new();
    for (i, it) in items.iter().enumerate() {
        if let Some(item) = item_from_nbt(it, i as i32) {
            out.push(item);
        }
    }
    out
}

pub(crate) fn item_from_nbt(v: &Value, default_slot: i32) -> Option<Item> {
    let id = get(v, "id").and_then(as_str)?.to_lowercase();
    if id.is_empty() || id.contains("air") {
        return None;
    }
    let count = nbt_get!(v, "count" | "Count" => as_i64).unwrap_or(1);
    let slot = nbt_get!(v, "Slot" | "slot" => as_i64)
        .map(|x| x as i32)
        .unwrap_or(default_slot);

    let mut item = Item {
        id,
        count,
        slot,
        ..Default::default()
    };

    if let Some(components) = get(v, "components") {
        apply_components(&mut item, components);
    }
    Some(item)
}

fn apply_components(item: &mut Item, components: &Value) {
    let Some(map) = comp(components) else { return };

    if let Some(v) = map.get("minecraft:custom_name") {
        item.custom_name = as_str(v).map(extract_text);
    }
    if item.custom_name.is_none() {
        if let Some(v) = map.get("minecraft:item_name") {
            item.custom_name = as_str(v).map(extract_text);
        }
    }
    if let Some(v) = map.get("minecraft:lore").and_then(as_list) {
        item.lore = v
            .iter()
            .filter_map(as_str)
            .map(extract_text)
            .filter(|s| !s.is_empty())
            .collect();
    }

    read_enchants(map.get("minecraft:enchantments"), &mut item.enchants);
    read_enchants(map.get("minecraft:stored_enchantments"), &mut item.enchants);

    if let Some(v) = map.get("minecraft:damage").and_then(as_i64) {
        item.damage = Some(v as i32);
    }
    if let Some(v) = map.get("minecraft:max_damage").and_then(as_i64) {
        item.max_damage = Some(v as i32);
    }

    if let Some(pc) = map.get("minecraft:potion_contents") {
        item.potion = match pc {
            Value::String(s) => Some(crate::model::prettify_id(s)),
            _ => get(pc, "potion")
                .and_then(as_str)
                .map(crate::model::prettify_id),
        };
    }

    for key in crate::model::NESTED_KEYS {
        if let Some(cv) = map.get(*key) {
            extract_nested(cv, &mut item.contents);
        }
    }

    if let Some(u) = map
        .get("sophisticatedcore:storage_uuid")
        .and_then(int_array)
        .and_then(|a| uuid_from_ints(&a))
    {
        item.storage_uuid = Some(u.to_lowercase());
    }

    if item.is_player_head() {
        if let Some(prof) = map.get("minecraft:profile") {
            extract_skull(prof, item);
        }
    }

    if item.id.contains("toolbox") && item.contents.is_empty() {
        if let Some(tb) = map.get("create:toolbox_inventory") {
            extract_toolbox(tb, &mut item.contents);
        }
    }

    let stock = find_num(components, "tagStock").or_else(|| find_num(components, "tag_stock"));
    let air = find_num(components, "create:banktank_air").or_else(|| find_num(components, "Air"));
    let capacity_ench = item
        .enchants
        .iter()
        .find(|(id, _)| id.contains("create:capacity"))
        .map(|(_, l)| *l);
    item.apply_gauges(stock, air, capacity_ench);
}

// Create toolboxes store contents in `create:toolbox_inventory` as a nested
// `items -> items -> { "<slot>": item }` compound. Collect them so the toolbox
// becomes an openable nested container.
pub(crate) fn extract_toolbox(inv: &Value, out: &mut Vec<Item>) {
    let inner = get(inv, "items")
        .and_then(|i| get(i, "items"))
        .or_else(|| get(inv, "items"));
    match inner {
        Some(Value::Compound(m)) => {
            for (slot, it) in m {
                if get(it, "id").is_some() {
                    let s = slot.parse::<i32>().unwrap_or(out.len() as i32);
                    if let Some(item) = item_from_nbt(it, s) {
                        out.push(item);
                    }
                }
            }
        }
        Some(Value::List(l)) => {
            for it in l {
                if let Some(item) = item_from_nbt(it, out.len() as i32) {
                    out.push(item);
                }
            }
        }
        _ => {}
    }
}

fn extract_skull(prof: &Value, item: &mut Item) {
    if let Value::String(s) = prof {
        item.head_ref = Some(s.clone());
        return;
    }
    if let Some(props) = get(prof, "properties").and_then(as_list) {
        for p in props {
            if get(p, "name").and_then(as_str) == Some("textures") {
                if let Some(v) = get(p, "value").and_then(as_str) {
                    if let Some(url) = crate::model::skin_url_from_textures_value(v) {
                        item.head_skin = Some(url);
                        return;
                    }
                }
            }
        }
    }
    if let Some(id) = get(prof, "id").and_then(int_array).and_then(|a| uuid_from_ints(&a)) {
        item.head_ref = Some(id.to_lowercase());
    } else if let Some(n) = get(prof, "name").and_then(as_str) {
        item.head_ref = Some(n.to_string());
    }
}

fn extract_nested(v: &Value, out: &mut Vec<Item>) {
    match v {
        Value::List(l) => {
            for e in l {
                if let Some(inner) = get(e, "item") {
                    let slot = get(e, "slot").and_then(as_i64).map(|x| x as i32);
                    if let Some(it) = item_from_nbt(inner, slot.unwrap_or(out.len() as i32)) {
                        out.push(it);
                    }
                } else if get(e, "id").is_some() {
                    if let Some(it) = item_from_nbt(e, out.len() as i32) {
                        out.push(it);
                    }
                }
            }
        }
        Value::Compound(_) => {
            if let Some(items) = get(v, "items") {
                extract_nested(items, out);
            }
        }
        _ => {}
    }
}

fn read_enchants(v: Option<&Value>, out: &mut Vec<(String, i32)>) {
    let Some(v) = v else { return };
    let levels = get(v, "levels").unwrap_or(v);
    if let Some(map) = comp(levels) {
        for (k, lvl) in map {
            if let Some(l) = as_i64(lvl) {
                out.push((k.clone(), l as i32));
            }
        }
    }
}

pub fn extract_text(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') || trimmed.starts_with('"') {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) {
            let mut out = String::new();
            gather_text(&json, &mut out);
            if !out.is_empty() {
                return out;
            }
        }
    }
    trimmed.trim_matches('"').to_string()
}

fn gather_text(v: &serde_json::Value, out: &mut String) {
    match v {
        serde_json::Value::String(s) => out.push_str(s),
        serde_json::Value::Array(a) => {
            for e in a {
                gather_text(e, out);
            }
        }
        serde_json::Value::Object(o) => {
            if let Some(serde_json::Value::String(t)) = o.get("text") {
                out.push_str(t);
            }
            if let Some(extra) = o.get("extra") {
                gather_text(extra, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod gauge_tests {
    use super::*;
    use fastnbt::Value;
    use std::collections::HashMap;

    fn cmp(pairs: Vec<(&str, Value)>) -> Value {
        Value::Compound(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    #[test]
    fn filling_tank_bar() {
        // Create Stuff & Additions stores fluid in custom_data.tagStock (Double).
        let item = cmp(vec![
            ("id", Value::String("create_sa:small_filling_tank".into())),
            ("count", Value::Int(1)),
            (
                "components",
                cmp(vec![(
                    "minecraft:custom_data",
                    cmp(vec![("tagStock", Value::Double(400.0))]),
                )]),
            ),
        ]);
        let it = item_from_nbt(&item, 0).unwrap();
        let bar = it.bar.expect("tank bar");
        assert!((bar.frac - 0.5).abs() < 1e-3, "400/800 = 0.5, got {}", bar.frac);
        assert_eq!(bar.color, [0x79, 0x97, 0xd9]);
    }

    #[test]
    fn fueling_tank_large_full() {
        let item = cmp(vec![
            ("id", Value::String("create_sa:large_fueling_tank".into())),
            ("count", Value::Int(1)),
            (
                "components",
                cmp(vec![(
                    "minecraft:custom_data",
                    cmp(vec![("tagStock", Value::Double(3200.0))]),
                )]),
            ),
        ]);
        let it = item_from_nbt(&item, 0).unwrap();
        let bar = it.bar.expect("tank bar even when full");
        assert!((bar.frac - 1.0).abs() < 1e-3, "3200/3200 = 1.0, got {}", bar.frac);
        assert_eq!(bar.color, [0xff, 0xa5, 0x65]);
    }

    #[test]
    fn backtank_white_outline_and_air() {
        let item = cmp(vec![
            ("id", Value::String("create:netherite_backtank".into())),
            ("count", Value::Int(1)),
            (
                "components",
                cmp(vec![("create:banktank_air", Value::Int(900))]),
            ),
        ]);
        let it = item_from_nbt(&item, 0).unwrap();
        assert_eq!(it.outline, Some([255, 255, 255]));
        let bar = it.bar.expect("air bar");
        assert!((bar.frac - 0.5).abs() < 1e-3, "900/1800 = 0.5, got {}", bar.frac);
    }

    #[test]
    fn backtank_capacity_enchant_raises_max() {
        let item = cmp(vec![
            ("id", Value::String("create:netherite_backtank".into())),
            ("count", Value::Int(1)),
            (
                "components",
                cmp(vec![
                    ("create:banktank_air", Value::Int(900)),
                    (
                        "minecraft:enchantments",
                        cmp(vec![("levels", cmp(vec![("create:capacity", Value::Int(1))]))]),
                    ),
                ]),
            ),
        ]);
        let it = item_from_nbt(&item, 0).unwrap();
        let bar = it.bar.expect("air bar");
        // capacity 1 -> max 3600, 900/3600 = 0.25
        assert!((bar.frac - 0.25).abs() < 1e-3, "900/3600 = 0.25, got {}", bar.frac);
    }

    #[test]
    fn vanilla_durability_bar() {
        let item = cmp(vec![
            ("id", Value::String("minecraft:diamond_pickaxe".into())),
            ("count", Value::Int(1)),
            (
                "components",
                cmp(vec![
                    ("minecraft:damage", Value::Int(312)),
                    ("minecraft:max_damage", Value::Int(1561)),
                ]),
            ),
        ]);
        let it = item_from_nbt(&item, 0).unwrap();
        let bar = it.bar.expect("durability bar");
        let expect = (1561.0 - 312.0) / 1561.0;
        assert!((bar.frac - expect).abs() < 1e-3);
    }

    #[test]
    fn toolbox_nested_contents() {
        // create:toolbox_inventory -> items -> items -> { "<slot>": stack }
        let slot_map = cmp(vec![
            (
                "0",
                cmp(vec![
                    ("id", Value::String("create:shaft".into())),
                    ("count", Value::Int(64)),
                ]),
            ),
            (
                "16",
                cmp(vec![
                    ("id", Value::String("create:belt_connector".into())),
                    ("count", Value::Int(44)),
                ]),
            ),
        ]);
        let item = cmp(vec![
            ("id", Value::String("create:brown_toolbox".into())),
            ("count", Value::Int(1)),
            (
                "components",
                cmp(vec![(
                    "create:toolbox_inventory",
                    cmp(vec![("items", cmp(vec![("items", slot_map)]))]),
                )]),
            ),
        ]);
        let it = item_from_nbt(&item, 0).unwrap();
        assert_eq!(it.contents.len(), 2);
        assert!(it.contents.iter().any(|c| c.id == "create:shaft" && c.count == 64));
        assert!(it.contents.iter().any(|c| c.id == "create:belt_connector"));
    }
}
