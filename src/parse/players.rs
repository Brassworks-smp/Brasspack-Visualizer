
use serde_json::Value as J;

use crate::parse::containers::{item_from_json, RawElement};
use crate::model::{Entry, EntryKind, Item};

pub(crate) fn build_player(el: &RawElement) -> Vec<Entry> {
    let name = el
        .name
        .as_ref()
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown")
        .to_string();
    let uuid = el
        .uuid
        .as_ref()
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let mut out = Vec::new();

    if let Some(inv @ J::Array(items_json)) = el.inventory.as_ref() {
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

    if let Some(ec @ J::Array(items_json)) = el.ender_chest.as_ref() {
        let items: Vec<Item> = items_json.iter().filter_map(|raw| item_from_json(raw, 0)).collect();
        if !items.is_empty() {
            out.push(make_entry(&name, &uuid, "Ender Chest", items, "minecraft:ender_chest", ec));
        }
    }

    out
}

pub(crate) fn remap_inventory_slot(slot: i32) -> i32 {
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
    let copies = copies![
        "Copy Name" => name,
        "Copy UUID" => uuid,
    ];
    let meta = meta![
        "Player" => name,
        "UUID" => uuid,
        "Section" => section,
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
        owner: name.to_lowercase(),
        uuid: uuid.to_lowercase(),
        nbt_blob: nbt_blob.to_lowercase(),
        ..Default::default()
    };
    let extra = format!("{name} {uuid} {section}");
    entry.finalize(&extra);
    entry
}
