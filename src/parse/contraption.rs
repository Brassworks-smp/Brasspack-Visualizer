use std::collections::HashMap;

use fastnbt::Value;

use crate::model::Item;
use crate::parse::nbt::{as_i64, as_list, as_str, get, int_array, item_from_nbt, push_items};

macro_rules! packed_field {
    ($p:expr, $offset:expr, $bits:expr) => {
        (($p << (64 - $offset - $bits)) >> (64 - $bits)) as i32
    };
}

pub(crate) fn extract(data: &Value, out: &mut Vec<Item>) {
    process(data, out, 0);
}

fn process(data: &Value, out: &mut Vec<Item>, depth: u8) {
    if depth > 8 {
        return;
    }
    let blocks = block_positions(data);
    if let Some(storages) = get(data, "items").and_then(as_list) {
        for st in storages {
            let Some(value) = get(st, "storage").and_then(|s| get(s, "value")) else {
                continue;
            };
            let mut contents = Vec::new();
            storage_items(value, &mut contents);
            if contents.is_empty() {
                continue;
            }
            out.push(Item {
                id: storage_block_id(st, &blocks),
                count: 1,
                slot: out.len() as i32,
                contents,
                ..Default::default()
            });
        }
    }
    if let Some(subs) = get(data, "SubContraptions").and_then(as_list) {
        for sub in subs {
            if let Some(sd) = sub_contraption_data(sub) {
                process(sd, out, depth + 1);
            }
        }
    }
}

fn sub_contraption_data(sub: &Value) -> Option<&Value> {
    if is_contraption(sub) {
        return Some(sub);
    }
    for key in ["contraption", "Contraption", "data"] {
        if let Some(d) = get(sub, key) {
            if is_contraption(d) {
                return Some(d);
            }
        }
    }
    None
}

fn is_contraption(v: &Value) -> bool {
    get(v, "items").is_some() || get(v, "Blocks").is_some()
}

fn storage_items(value: &Value, out: &mut Vec<Item>) {
    if let Some(items) = get(value, "items") {
        push_items(items, out);
    } else if get(value, "id").is_some() {
        if let Some(item) = item_from_nbt(value, out.len() as i32) {
            out.push(item);
        }
    }
}

fn storage_block_id(st: &Value, blocks: &HashMap<(i32, i32, i32), String>) -> String {
    get(st, "pos")
        .and_then(int_array)
        .filter(|p| p.len() == 3)
        .and_then(|p| blocks.get(&(p[0], p[1], p[2])).cloned())
        .or_else(|| storage_type_fallback(st))
        .unwrap_or_else(|| "minecraft:chest".to_string())
        .to_lowercase()
}

fn storage_type_fallback(st: &Value) -> Option<String> {
    let ty = get(st, "storage").and_then(|s| get(s, "type")).and_then(as_str)?;
    Some(
        match ty {
            "create:vault" => "create:item_vault",
            "create:depot" => "create:depot",
            "create_connected:fluid_vessel" => "create_connected:fluid_vessel",
            _ => "minecraft:chest",
        }
        .to_string(),
    )
}

fn block_positions(data: &Value) -> HashMap<(i32, i32, i32), String> {
    let mut map = HashMap::new();
    let Some(list) = get(data, "Blocks")
        .and_then(|b| get(b, "BlockList"))
        .and_then(as_list)
    else {
        return map;
    };
    for entry in list {
        let Some(pos) = get(entry, "Pos").and_then(as_i64) else {
            continue;
        };
        if let Some(id) = get(entry, "Data").and_then(|d| get(d, "id")).and_then(as_str) {
            map.insert(unpack_pos(pos), id.to_string());
        }
    }
    map
}

fn unpack_pos(p: i64) -> (i32, i32, i32) {
    (
        packed_field!(p, 38, 26),
        packed_field!(p, 0, 12),
        packed_field!(p, 12, 26),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastnbt::Value;

    fn cmp(pairs: Vec<(&str, Value)>) -> Value {
        Value::Compound(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    fn ints(v: &[i64]) -> Value {
        Value::List(v.iter().map(|x| Value::Int(*x as i32)).collect())
    }

    fn pack(x: i64, y: i64, z: i64) -> i64 {
        ((x & 0x3FFFFFF) << 38) | (y & 0xFFF) | ((z & 0x3FFFFFF) << 12)
    }

    #[test]
    fn names_container_from_block_at_pos_and_nests_items() {
        let data = cmp(vec![
            (
                "Blocks",
                cmp(vec![(
                    "BlockList",
                    Value::List(vec![cmp(vec![
                        ("Pos", Value::Long(pack(0, 4, 0))),
                        ("Data", cmp(vec![("id", Value::String("farmersdelight:cabinet".into()))])),
                    ])]),
                )]),
            ),
            (
                "items",
                Value::List(vec![cmp(vec![
                    ("pos", ints(&[0, 4, 0])),
                    (
                        "storage",
                        cmp(vec![
                            ("type", Value::String("create:simple".into())),
                            (
                                "value",
                                cmp(vec![
                                    ("size", Value::Int(27)),
                                    (
                                        "items",
                                        cmp(vec![(
                                            "14",
                                            cmp(vec![
                                                ("id", Value::String("minecraft:black_dye".into())),
                                                ("count", Value::Int(22)),
                                            ]),
                                        )]),
                                    ),
                                ]),
                            ),
                        ]),
                    ),
                ])]),
            ),
        ]);
        let mut out = Vec::new();
        extract(&data, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "farmersdelight:cabinet");
        assert_eq!(out[0].contents.len(), 1);
        assert_eq!(out[0].contents[0].id, "minecraft:black_dye");
        assert_eq!(out[0].contents[0].count, 22);
        assert_eq!(out[0].contents[0].slot, 14);
    }

    #[test]
    fn depot_single_stack_uses_type_fallback_when_unmapped() {
        let data = cmp(vec![(
            "items",
            Value::List(vec![cmp(vec![
                ("pos", ints(&[9, 9, 9])),
                (
                    "storage",
                    cmp(vec![
                        ("type", Value::String("create:depot".into())),
                        (
                            "value",
                            cmp(vec![
                                ("id", Value::String("minecraft:iron_ingot".into())),
                                ("count", Value::Int(5)),
                            ]),
                        ),
                    ]),
                ),
            ])]),
        )]);
        let mut out = Vec::new();
        extract(&data, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "create:depot");
        assert_eq!(out[0].contents.len(), 1);
        assert_eq!(out[0].contents[0].id, "minecraft:iron_ingot");
        assert_eq!(out[0].contents[0].count, 5);
    }

    #[test]
    fn empty_storages_are_skipped() {
        let data = cmp(vec![(
            "items",
            Value::List(vec![cmp(vec![
                ("pos", ints(&[0, 0, 0])),
                (
                    "storage",
                    cmp(vec![
                        ("type", Value::String("create:simple".into())),
                        ("value", cmp(vec![("size", Value::Int(27)), ("items", cmp(vec![]))])),
                    ]),
                ),
            ])]),
        )]);
        let mut out = Vec::new();
        extract(&data, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn recurses_sub_contraptions() {
        let inner = cmp(vec![
            ("pos", ints(&[0, 0, 0])),
            (
                "storage",
                cmp(vec![
                    ("type", Value::String("create:simple".into())),
                    (
                        "value",
                        cmp(vec![(
                            "items",
                            cmp(vec![(
                                "0",
                                cmp(vec![
                                    ("id", Value::String("minecraft:emerald".into())),
                                    ("count", Value::Int(3)),
                                ]),
                            )]),
                        )]),
                    ),
                ]),
            ),
        ]);
        let sub = cmp(vec![(
            "contraption",
            cmp(vec![("items", Value::List(vec![inner]))]),
        )]);
        let data = cmp(vec![
            ("items", Value::List(vec![])),
            ("SubContraptions", Value::List(vec![sub])),
        ]);
        let mut out = Vec::new();
        extract(&data, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].contents.len(), 1);
        assert_eq!(out[0].contents[0].id, "minecraft:emerald");
        assert_eq!(out[0].contents[0].count, 3);
    }
}
