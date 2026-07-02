
use rayon::prelude::*;
use serde_json::Value as J;

use crate::parse::containers::item_from_json;
use crate::model::{CopyAction, Entry, EntryKind, Item};

pub fn entries_from(list: &[J]) -> Vec<Entry> {
    list.par_iter().flat_map(parse_player).collect()
}

fn parse_player(p: &J) -> Vec<Entry> {
    let name = p
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown")
        .to_string();
    let uuid = p
        .get("uuid")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let mut out = Vec::new();

    if let Some(inv @ J::Array(items_json)) = p.get("inventory") {
        let items: Vec<Item> = items_json
            .iter()
            .filter_map(|raw| {
                let mut it = item_from_json(raw, 0)?;
                it.slot = remap_inventory_slot(it.slot);
                Some(it)
            })
            .collect();
        if !items.is_empty() {
            out.push(make_entry(&name, &uuid, "Inventory", items, "minecraft:player_head", inv));
        }
    }

    if let Some(ec @ J::Array(items_json)) = p.get("ender_chest") {
        let items: Vec<Item> = items_json.iter().filter_map(|raw| item_from_json(raw, 0)).collect();
        if !items.is_empty() {
            out.push(make_entry(&name, &uuid, "Ender Chest", items, "minecraft:ender_chest", ec));
        }
    }

    out
}

fn remap_inventory_slot(slot: i32) -> i32 {
    match slot {
        103 => 0,
        102 => 1,
        101 => 2,
        100 => 3,
        -106 => 4,
        0..=8 => 36 + slot,
        9..=35 => slot,
        other => other,
    }
}

fn make_entry(
    name: &str,
    uuid: &str,
    section: &str,
    items: Vec<Item>,
    icon: &str,
    raw: &J,
) -> Entry {
    let copies = vec![
        CopyAction { label: "Copy Name".into(), value: name.to_string() },
        CopyAction { label: "Copy UUID".into(), value: uuid.to_string() },
    ];
    let meta = vec![
        ("Player".into(), name.to_string()),
        ("UUID".into(), uuid.to_string()),
        ("Section".into(), section.to_string()),
    ];
    let mut nbt_blob = String::new();
    crate::parse::containers::collect_nbt(raw, &mut nbt_blob);
    let mut entry = Entry {
        kind: EntryKind::Player,
        title: format!("{name} - {section}"),
        header_icon: icon.to_string(),
        meta,
        copies,
        items,
        upgrades: Vec::new(),
        cols: 9,
        rows: 0,
        is_dungeon: false,
        dimension: String::new(),
        owner: name.to_lowercase(),
        uuid: uuid.to_lowercase(),
        coords: None,
        search_blob: String::new(),
        nbt_blob: nbt_blob.to_lowercase(),
        max_stack: 0,
        all_enchants: Vec::new(),
    };
    let extra = format!("{name} {uuid} {section}");
    entry.finalize(&extra);
    entry
}
