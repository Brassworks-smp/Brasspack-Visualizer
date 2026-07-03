use fastnbt::Value;

use crate::model::{prettify_id, Entry, EntryKind, Item};
use crate::parse::nbt::{as_i64, as_list, as_str, get, item_from_nbt};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DumpKind {
    Containers,
    Players,
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

fn read_u16(b: &[u8], i: usize) -> Option<usize> {
    Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]) as usize)
}

fn read_i32(b: &[u8], i: usize) -> Option<i64> {
    Some(i32::from_be_bytes([*b.get(i)?, *b.get(i + 1)?, *b.get(i + 2)?, *b.get(i + 3)?]) as i64)
}

fn skip_payload(b: &[u8], mut i: usize, t: u8) -> Option<usize> {
    Some(match t {
        1 => i + 1,
        2 => i + 2,
        3 => i + 4,
        4 => i + 8,
        5 => i + 4,
        6 => i + 8,
        7 => {
            let n = read_i32(b, i)? as usize;
            i + 4 + n
        }
        8 => {
            let n = read_u16(b, i)?;
            i + 2 + n
        }
        9 => {
            let et = *b.get(i)?;
            let n = read_i32(b, i + 1)? as usize;
            i += 5;
            for _ in 0..n {
                i = skip_payload(b, i, et)?;
            }
            i
        }
        10 => {
            loop {
                let ct = *b.get(i)?;
                i += 1;
                if ct == 0 {
                    break;
                }
                let nl = read_u16(b, i)?;
                i += 2 + nl;
                i = skip_payload(b, i, ct)?;
            }
            i
        }
        11 => {
            let n = read_i32(b, i)? as usize;
            i + 4 + 4 * n
        }
        12 => {
            let n = read_i32(b, i)? as usize;
            i + 4 + 8 * n
        }
        _ => return None,
    })
}

pub fn split_dump(b: &[u8]) -> Result<(DumpKind, Vec<(u32, u32)>), String> {
    let mut i = 0usize;
    if *b.get(i).ok_or("empty nbt")? != 10 {
        return Err("nbt root is not a compound".into());
    }
    i += 1;
    let root_name = read_u16(b, i).ok_or("bad nbt root name")?;
    i += 2 + root_name;
    loop {
        let ct = *b.get(i).ok_or("truncated nbt")?;
        i += 1;
        if ct == 0 {
            break;
        }
        let name_len = read_u16(b, i).ok_or("truncated nbt name")?;
        let name =
            std::str::from_utf8(b.get(i + 2..i + 2 + name_len).ok_or("truncated nbt")?).unwrap_or("");
        let target = if ct == 9 && name == "containers" {
            Some(DumpKind::Containers)
        } else if ct == 9 && name == "players" {
            Some(DumpKind::Players)
        } else {
            None
        };
        i += 2 + name_len;
        if let Some(kind) = target {
            let et = *b.get(i).ok_or("truncated list")?;
            let n = read_i32(b, i + 1).ok_or("truncated list len")? as usize;
            let mut j = i + 5;
            let mut spans = Vec::with_capacity(n);
            if et == 10 {
                for _ in 0..n {
                    let start = j;
                    j = skip_payload(b, j, 10).ok_or("truncated list element")?;
                    spans.push((start as u32, j as u32));
                }
            }
            return Ok((kind, spans));
        }
        i = skip_payload(b, i, ct).ok_or("truncated nbt skip")?;
    }
    Err("nbt has no 'containers' or 'players' list".into())
}

fn parse_element(b: &[u8], span: (u32, u32)) -> Option<Value> {
    let (s, e) = span;
    let body = b.get(s as usize..e as usize)?;
    let mut buf = Vec::with_capacity(3 + body.len());
    buf.extend_from_slice(&[0x0A, 0x00, 0x00]);
    buf.extend_from_slice(body);
    fastnbt::from_bytes(&buf).ok()
}

pub fn build_one(b: &[u8], span: (u32, u32), kind: DumpKind) -> Vec<Entry> {
    let Some(v) = parse_element(b, span) else {
        return Vec::new();
    };
    match kind {
        DumpKind::Containers => build_container(&v).into_iter().collect(),
        DumpKind::Players => build_players(&v),
    }
}

fn items_from_list(v: Option<&Value>) -> Vec<Item> {
    let mut out = Vec::new();
    if let Some(list) = v.and_then(as_list) {
        for (i, it) in list.iter().enumerate() {
            if let Some(item) = item_from_nbt(it, i as i32) {
                out.push(item);
            }
        }
    }
    out
}

fn coord_str(v: Option<&Value>) -> String {
    v.and_then(as_i64).map(|n| n.to_string()).unwrap_or_else(|| "?".into())
}

fn collect_search(v: Option<&Value>, out: &mut String) {
    match v {
        Some(Value::Compound(m)) => {
            for (k, val) in m {
                out.push_str(k);
                out.push('\n');
                collect_search(Some(val), out);
            }
        }
        Some(Value::List(l)) => l.iter().for_each(|e| collect_search(Some(e), out)),
        Some(Value::String(s)) => {
            out.push_str(s);
            out.push('\n');
        }
        _ => {}
    }
}

fn build_container(v: &Value) -> Option<Entry> {
    let id = get(v, "id").and_then(as_str).unwrap_or("minecraft:chest").to_string();
    let x = coord_str(get(v, "x"));
    let y = coord_str(get(v, "y"));
    let z = coord_str(get(v, "z"));
    let dimension = get(v, "dimension")
        .and_then(as_str)
        .unwrap_or("minecraft:overworld")
        .to_string();
    let is_dungeon = get(v, "is_dungeon").and_then(as_i64).map(|n| n != 0).unwrap_or(false);

    let items = items_from_list(get(v, "items"));

    let coords = match (
        get(v, "x").and_then(as_i64),
        get(v, "y").and_then(as_i64),
        get(v, "z").and_then(as_i64),
    ) {
        (Some(x), Some(y), Some(z)) => Some((x, y, z)),
        _ => None,
    };

    let mut nbt_blob = String::new();
    collect_search(get(v, "items"), &mut nbt_blob);

    let meta = meta![
        "Type" => id.clone(),
        "Position" => format!("{}, {}, {}", x, y, z),
        "Dimension" => dimension.clone(),
        "Dungeon" => if is_dungeon { "Yes" } else { "No" },
    ];

    let copies = copies![
        "Copy TP" => format!("/execute in {} run tp @s {} {} {}", dimension, x, y, z),
        "Copy Coords" => format!("{} {} {}", x, y, z),
        "Copy Dimension" => dimension.clone(),
    ];

    let title = format!("{} @ {}, {}, {}", prettify_id(&id), x, y, z);

    let mut entry = Entry {
        kind: EntryKind::Container,
        title,
        header_icon: id.clone(),
        meta,
        copies,
        items,
        is_dungeon,
        dimension,
        coords,
        nbt_blob: nbt_blob.to_lowercase(),
        ..Default::default()
    };
    let extra = format!("{} {} {} {}", id, x, y, z);
    entry.finalize(&extra);
    Some(entry)
}

fn build_players(v: &Value) -> Vec<Entry> {
    let name = get(v, "name").and_then(as_str).unwrap_or("Unknown").to_string();
    let uuid = get(v, "uuid").and_then(as_str).unwrap_or("").to_string();

    let mut out = Vec::new();

    let inv: Vec<Item> = get(v, "inventory")
        .and_then(as_list)
        .map(|l| {
            l.iter()
                .filter_map(|raw| {
                    let mut it = item_from_nbt(raw, 0)?;
                    it.slot = remap_inventory_slot(it.slot);
                    Some(it)
                })
                .collect()
        })
        .unwrap_or_default();
    if !inv.is_empty() {
        out.push(make_player_entry(
            &name,
            &uuid,
            "Inventory",
            inv,
            "minecraft:player_head",
            get(v, "inventory"),
        ));
    }

    let ec = items_from_list(get(v, "ender_chest"));
    if !ec.is_empty() {
        out.push(make_player_entry(
            &name,
            &uuid,
            "Ender Chest",
            ec,
            "minecraft:ender_chest",
            get(v, "ender_chest"),
        ));
    }

    out
}

fn make_player_entry(
    name: &str,
    uuid: &str,
    section: &str,
    items: Vec<Item>,
    icon: &str,
    raw: Option<&Value>,
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
    collect_search(raw, &mut nbt_blob);
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

#[cfg(test)]
mod tests {
    use super::*;
    use fastnbt::Value;
    use std::collections::HashMap;

    fn cmp(pairs: Vec<(&str, Value)>) -> Value {
        Value::Compound(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    fn item(id: &str, slot: i32, count: i32) -> Value {
        cmp(vec![
            ("id", Value::String(id.into())),
            ("Slot", Value::Int(slot)),
            ("count", Value::Int(count)),
            ("components", Value::Compound(HashMap::new())),
        ])
    }

    fn to_nbt(root: &Value) -> Vec<u8> {
        fastnbt::to_bytes(root).expect("serialize")
    }

    #[test]
    fn containers_roundtrip() {
        let root = cmp(vec![(
            "containers",
            Value::List(vec![
                cmp(vec![
                    ("id", Value::String("minecraft:chest".into())),
                    ("dimension", Value::String("minecraft:overworld".into())),
                    ("x", Value::Int(10)),
                    ("y", Value::Int(64)),
                    ("z", Value::Int(-20)),
                    ("is_dungeon", Value::Byte(0)),
                    (
                        "items",
                        Value::List(vec![
                            item("minecraft:diamond", 0, 64),
                            item("minecraft:netherite_ingot", 5, 3),
                        ]),
                    ),
                ]),
                cmp(vec![
                    ("id", Value::String("minecraft:barrel".into())),
                    ("dimension", Value::String("minecraft:the_nether".into())),
                    ("x", Value::Int(1)),
                    ("y", Value::Int(2)),
                    ("z", Value::Int(3)),
                    ("is_dungeon", Value::Byte(1)),
                    ("items", Value::List(vec![item("minecraft:gold_ingot", 2, 10)])),
                ]),
            ]),
        )]);
        let bytes = to_nbt(&root);
        let (kind, spans) = split_dump(&bytes).expect("split");
        assert!(matches!(kind, DumpKind::Containers));
        assert_eq!(spans.len(), 2);

        let e0 = build_one(&bytes, spans[0], kind);
        assert_eq!(e0.len(), 1);
        let c0 = &e0[0];
        assert_eq!(c0.kind, EntryKind::Container);
        assert_eq!(c0.header_icon, "minecraft:chest");
        assert_eq!(c0.coords, Some((10, 64, -20)));
        assert!(!c0.is_dungeon);
        assert_eq!(c0.items.len(), 2);
        assert!(c0.search_blob.contains("diamond"));

        let e1 = build_one(&bytes, spans[1], kind);
        assert!(e1[0].is_dungeon);
        assert_eq!(e1[0].dimension, "minecraft:the_nether");
    }

    #[test]
    fn store_opens_gzipped_nbt_dump() {
        use std::io::Write;
        let root = cmp(vec![(
            "containers",
            Value::List(vec![
                cmp(vec![
                    ("id", Value::String("minecraft:chest".into())),
                    ("dimension", Value::String("minecraft:overworld".into())),
                    ("x", Value::Int(7)),
                    ("y", Value::Int(70)),
                    ("z", Value::Int(7)),
                    ("is_dungeon", Value::Byte(0)),
                    ("items", Value::List(vec![item("minecraft:emerald", 0, 5)])),
                ]),
                cmp(vec![
                    ("id", Value::String("minecraft:barrel".into())),
                    ("dimension", Value::String("minecraft:overworld".into())),
                    ("x", Value::Int(0)),
                    ("y", Value::Int(0)),
                    ("z", Value::Int(0)),
                    ("is_dungeon", Value::Byte(1)),
                    ("items", Value::List(vec![item("minecraft:diamond_pickaxe", 3, 1)])),
                ]),
            ]),
        )]);
        let bytes = to_nbt(&root);
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&bytes).unwrap();
        let gz = enc.finish().unwrap();

        let dir = std::env::temp_dir().join(format!("infil_nbt_test_{}.nbt", std::process::id()));
        std::fs::write(&dir, &gz).unwrap();

        let store = crate::store::Store::open(dir.to_str().unwrap(), crate::store::Load::Nbt(None))
            .expect("store open");
        assert_eq!(store.len(), 2);
        let e = store.entry(0).expect("entry 0");
        assert_eq!(e.header_icon, "minecraft:chest");
        assert!(e.search_blob.contains("emerald"));

        let mut f = crate::search::Filters::default();
        f.item = "diamond_pickaxe".into();
        let hits = store.filter(&f.compile());
        assert_eq!(hits.len(), 1);

        std::fs::remove_file(&dir).ok();
    }

    #[test]
    fn players_roundtrip() {
        let root = cmp(vec![(
            "players",
            Value::List(vec![cmp(vec![
                ("name", Value::String("Notch".into())),
                ("uuid", Value::String("069a79f4-44e9-4726-a5be-fca90e38aaf5".into())),
                ("inventory", Value::List(vec![item("minecraft:diamond_sword", 0, 1)])),
                ("ender_chest", Value::List(vec![item("minecraft:obsidian", 0, 64)])),
            ])]),
        )]);
        let bytes = to_nbt(&root);
        let (kind, spans) = split_dump(&bytes).expect("split");
        assert!(matches!(kind, DumpKind::Players));
        assert_eq!(spans.len(), 1);

        let entries = build_one(&bytes, spans[0], kind);
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.kind == EntryKind::Player));
        assert!(entries.iter().any(|e| e.title.contains("Inventory")));
        assert!(entries.iter().any(|e| e.title.contains("Ender Chest")));
        assert_eq!(entries[0].owner, "notch");
    }
}
