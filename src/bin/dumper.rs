use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use fastnbt::Value;
use flate2::read::{GzDecoder, ZlibDecoder};
use flate2::write::GzEncoder;
use flate2::Compression;
use rayon::prelude::*;
use serde::Deserialize;

const NO_SLOT: i64 = i64::MIN;

#[derive(Deserialize)]
struct Config {
    #[serde(default)]
    mode: String,
    output: String,
    #[serde(default)]
    target_ids: Vec<String>,
    #[serde(default)]
    vault_ids: Vec<String>,
    #[serde(default)]
    dimensions: Vec<DimCfg>,
}

#[derive(Deserialize)]
struct DimCfg {
    id: String,
    region_dir: String,
}

#[derive(Deserialize)]
struct ChunkBlockEntities {
    #[serde(default)]
    block_entities: Vec<Value>,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(cfg_path) = args.get(1) else {
        eprintln!("usage: dumper <config.json>");
        std::process::exit(2);
    };

    let cfg: Config = match std::fs::read(cfg_path).map_err(|e| e.to_string()).and_then(|b| {
        serde_json::from_slice(&b).map_err(|e| e.to_string())
    }) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("failed to read config: {e}");
            std::process::exit(2);
        }
    };

    if cfg.mode != "containers" && !cfg.mode.is_empty() {
        eprintln!("unsupported mode: {}", cfg.mode);
        std::process::exit(2);
    }

    let targets: HashSet<String> = cfg.target_ids.into_iter().collect();
    let vaults: HashSet<String> = cfg.vault_ids.into_iter().collect();

    let mut region_files: Vec<(String, PathBuf)> = Vec::new();
    for dim in &cfg.dimensions {
        let dir = Path::new(&dim.region_dir);
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        for ent in rd.flatten() {
            let p = ent.path();
            if p.extension().and_then(|e| e.to_str()) == Some("mca") {
                region_files.push((dim.id.clone(), p));
            }
        }
    }

    let total = region_files.len();
    println!("SCAN {total}");

    let files_done = AtomicUsize::new(0);
    let chunks_scanned = AtomicUsize::new(0);
    let found = AtomicUsize::new(0);

    let results: Vec<Value> = region_files
        .par_iter()
        .flat_map_iter(|(dim, path)| {
            let mut out: Vec<Value> = Vec::new();
            let mut local_chunks = 0usize;
            scan_region(path, |chunk_nbt| {
                local_chunks += 1;
                let parsed: Result<ChunkBlockEntities, _> = fastnbt::from_bytes(chunk_nbt);
                if let Ok(chunk) = parsed {
                    for be in &chunk.block_entities {
                        if let Some(entry) = extract_container(be, dim, &targets, &vaults) {
                            out.push(entry);
                        }
                    }
                }
            });

            chunks_scanned.fetch_add(local_chunks, Ordering::Relaxed);
            found.fetch_add(out.len(), Ordering::Relaxed);
            let done = files_done.fetch_add(1, Ordering::Relaxed) + 1;
            if done == total || done % 32 == 0 {
                println!("PROGRESS {done} {total} {}", found.load(Ordering::Relaxed));
            }
            out
        })
        .collect();

    let mut root: HashMap<String, Value> = HashMap::new();
    root.insert("containers".to_string(), Value::List(results));
    let root = Value::Compound(root);

    let raw = match fastnbt::to_bytes(&root) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("nbt serialize failed: {e}");
            std::process::exit(1);
        }
    };

    if let Err(e) = write_gzip(Path::new(&cfg.output), &raw) {
        eprintln!("failed to write output: {e}");
        std::process::exit(1);
    }

    println!(
        "DONE {} {}",
        found.load(Ordering::Relaxed),
        chunks_scanned.load(Ordering::Relaxed)
    );
}

fn write_gzip(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("nbt.tmp");
    {
        let f = File::create(&tmp)?;
        let mut enc = GzEncoder::new(f, Compression::default());
        enc.write_all(data)?;
        enc.finish()?;
    }
    std::fs::rename(&tmp, path)
}

fn region_coords(path: &Path) -> (i32, i32) {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let parts: Vec<&str> = stem.split('.').collect();
    if parts.len() == 3 && parts[0] == "r" {
        let rx = parts[1].parse().unwrap_or(0);
        let rz = parts[2].parse().unwrap_or(0);
        (rx, rz)
    } else {
        (0, 0)
    }
}

fn scan_region<F: FnMut(&[u8])>(path: &Path, mut visit: F) {
    let Ok(mut f) = File::open(path) else { return };
    let mut header = [0u8; 8192];
    if f.read_exact(&mut header).is_err() {
        return;
    }
    let (rx, rz) = region_coords(path);
    let dir = path.parent().unwrap_or_else(|| Path::new("."));

    let mut chunk_buf: Vec<u8> = Vec::new();
    let mut nbt_buf: Vec<u8> = Vec::new();

    for idx in 0..1024usize {
        let b = idx * 4;
        let offset = ((header[b] as usize) << 16) | ((header[b + 1] as usize) << 8) | (header[b + 2] as usize);
        let sectors = header[b + 3] as usize;
        if offset == 0 || sectors == 0 {
            continue;
        }
        if f.seek(SeekFrom::Start((offset * 4096) as u64)).is_err() {
            continue;
        }
        let mut lb = [0u8; 5];
        if f.read_exact(&mut lb).is_err() {
            continue;
        }
        let length = u32::from_be_bytes([lb[0], lb[1], lb[2], lb[3]]) as usize;
        if length == 0 {
            continue;
        }
        let mut ctype = lb[4];
        let external = ctype & 0x80 != 0;
        ctype &= 0x7f;

        let raw: &[u8] = if external {
            let lx = idx % 32;
            let lz = idx / 32;
            let cx = rx * 32 + lx as i32;
            let cz = rz * 32 + lz as i32;
            let ext = dir.join(format!("c.{cx}.{cz}.mcc"));
            match std::fs::read(&ext) {
                Ok(d) => {
                    chunk_buf = d;
                    &chunk_buf[..]
                }
                Err(_) => continue,
            }
        } else {
            let payload = length - 1;
            chunk_buf.resize(payload, 0);
            if f.read_exact(&mut chunk_buf).is_err() {
                continue;
            }
            &chunk_buf[..]
        };

        nbt_buf.clear();
        let ok = match ctype {
            1 => GzDecoder::new(raw).read_to_end(&mut nbt_buf).is_ok(),
            2 => ZlibDecoder::new(raw).read_to_end(&mut nbt_buf).is_ok(),
            3 => {
                nbt_buf.extend_from_slice(raw);
                true
            }
            _ => false,
        };
        if ok {
            visit(&nbt_buf);
        }
    }
}

fn comp(v: &Value) -> Option<&HashMap<String, Value>> {
    match v {
        Value::Compound(m) => Some(m),
        _ => None,
    }
}

fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Byte(x) => Some(*x as i64),
        Value::Short(x) => Some(*x as i64),
        Value::Int(x) => Some(*x as i64),
        Value::Long(x) => Some(*x),
        _ => None,
    }
}

fn as_str(v: &Value) -> Option<&str> {
    match v {
        Value::String(s) => Some(s.as_str()),
        _ => None,
    }
}

fn as_list(v: &Value) -> Option<&Vec<Value>> {
    match v {
        Value::List(l) => Some(l),
        _ => None,
    }
}

fn get<'a>(m: &'a HashMap<String, Value>, key: &str) -> Option<&'a Value> {
    m.get(key)
}

fn int_field(m: &HashMap<String, Value>, key: &str) -> i32 {
    get(m, key).and_then(as_i64).unwrap_or(0) as i32
}

fn slot_unsigned(item: &HashMap<String, Value>) -> i64 {
    match item.get("Slot") {
        Some(Value::Byte(b)) => (*b as i64) & 0xFF,
        Some(Value::Int(i)) => *i as i64,
        Some(Value::Short(s)) => *s as i64,
        _ => -1,
    }
}

fn item_count(item: &HashMap<String, Value>, default: i64) -> i64 {
    if let Some(v) = item.get("count").and_then(as_i64) {
        v
    } else if let Some(v) = item.get("Count").and_then(as_i64) {
        v
    } else {
        default
    }
}

fn item_components(item: &HashMap<String, Value>) -> Option<&Value> {
    item.get("components").or_else(|| item.get("tag"))
}

fn item_to_nbt(item: &HashMap<String, Value>, slot: i64) -> Value {
    let mut out: HashMap<String, Value> = HashMap::new();
    out.insert(
        "id".to_string(),
        Value::String(item.get("id").and_then(as_str).unwrap_or("").to_string()),
    );
    if slot != NO_SLOT {
        out.insert("Slot".to_string(), Value::Int(slot as i32));
    }
    out.insert("count".to_string(), Value::Int(item_count(item, 1) as i32));
    if let Some(c) = item_components(item) {
        out.insert("components".to_string(), c.clone());
    }
    Value::Compound(out)
}

fn resolve_items(be: &HashMap<String, Value>) -> Option<&Vec<Value>> {
    if let Some(inv) = be.get("Inventory").and_then(comp) {
        if let Some(l) = inv.get("Items").and_then(as_list) {
            return Some(l);
        }
        if let Some(l) = inv.get("items").and_then(as_list) {
            return Some(l);
        }
    }
    for key in ["Items", "inventory", "Inventory", "real_items"] {
        if let Some(l) = be.get(key).and_then(as_list) {
            return Some(l);
        }
    }
    None
}

fn canonical(v: &Value) -> String {
    let mut s = String::new();
    canonical_into(v, &mut s);
    s
}

fn canonical_into(v: &Value, out: &mut String) {
    match v {
        Value::Compound(m) => {
            let sorted: BTreeMap<&String, &Value> = m.iter().collect();
            out.push('{');
            for (k, val) in sorted {
                out.push_str(k);
                out.push(':');
                canonical_into(val, out);
                out.push(',');
            }
            out.push('}');
        }
        Value::List(l) => {
            out.push('[');
            for e in l {
                canonical_into(e, out);
                out.push(',');
            }
            out.push(']');
        }
        Value::String(s) => {
            out.push('"');
            out.push_str(s);
            out.push('"');
        }
        Value::Byte(x) => out.push_str(&x.to_string()),
        Value::Short(x) => out.push_str(&x.to_string()),
        Value::Int(x) => out.push_str(&x.to_string()),
        Value::Long(x) => out.push_str(&x.to_string()),
        Value::Float(x) => out.push_str(&x.to_string()),
        Value::Double(x) => out.push_str(&x.to_string()),
        Value::ByteArray(a) => out.push_str(&format!("{:?}", a.iter().collect::<Vec<_>>())),
        Value::IntArray(a) => out.push_str(&format!("{:?}", a.iter().collect::<Vec<_>>())),
        Value::LongArray(a) => out.push_str(&format!("{:?}", a.iter().collect::<Vec<_>>())),
    }
}

fn extract_container(
    be_val: &Value,
    dimension: &str,
    targets: &HashSet<String>,
    vaults: &HashSet<String>,
) -> Option<Value> {
    let be = comp(be_val)?;
    let id = get(be, "id").and_then(as_str)?;
    let is_vault = vaults.contains(id);
    if !targets.contains(id) && !is_vault {
        return None;
    }

    let items_list = resolve_items(be);

    if is_vault && items_list.map(|l| l.is_empty()).unwrap_or(true) {
        return None;
    }

    let is_dungeon = matches!(be.get("LootTable"), Some(Value::String(_)));

    let mut entry: HashMap<String, Value> = HashMap::new();
    entry.insert("dimension".to_string(), Value::String(dimension.to_string()));
    entry.insert("id".to_string(), Value::String(id.to_string()));
    entry.insert("x".to_string(), Value::Int(int_field(be, "x")));
    entry.insert("y".to_string(), Value::Int(int_field(be, "y")));
    entry.insert("z".to_string(), Value::Int(int_field(be, "z")));
    entry.insert("is_dungeon".to_string(), Value::Byte(is_dungeon as i8));
    if is_vault {
        entry.insert("is_vault".to_string(), Value::Byte(1));
    }

    let mut out_items: Vec<Value> = Vec::new();
    if let Some(list) = items_list {
        if is_vault {
            let mut aggregated: HashMap<String, (String, Option<Value>)> = HashMap::new();
            let mut counts: HashMap<String, i64> = HashMap::new();
            let mut order: Vec<String> = Vec::new();
            for it in list {
                let Some(item) = comp(it) else { continue };
                let Some(item_id) = item.get("id").and_then(as_str) else { continue };
                let count = item_count(item, 0);
                let comps = item_components(item);
                let key = format!(
                    "{item_id}|{}",
                    comps.map(canonical).unwrap_or_default()
                );
                if !counts.contains_key(&key) {
                    order.push(key.clone());
                    aggregated.insert(key.clone(), (item_id.to_string(), comps.cloned()));
                }
                *counts.entry(key).or_insert(0) += count;
            }
            for key in order {
                let (item_id, comps) = &aggregated[&key];
                let mut o: HashMap<String, Value> = HashMap::new();
                o.insert("id".to_string(), Value::String(item_id.clone()));
                if let Some(c) = comps {
                    o.insert("components".to_string(), c.clone());
                }
                o.insert("count".to_string(), Value::Int(counts[&key] as i32));
                out_items.push(Value::Compound(o));
            }
        } else {
            for it in list {
                let Some(item) = comp(it) else { continue };
                if !matches!(item.get("id"), Some(Value::String(_))) {
                    continue;
                }
                let slot = slot_unsigned(item);
                if slot == -1 {
                    continue;
                }
                out_items.push(item_to_nbt(item, slot));
            }
        }
    }
    entry.insert("items".to_string(), Value::List(out_items));
    Some(Value::Compound(entry))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmp(pairs: Vec<(&str, Value)>) -> Value {
        Value::Compound(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    fn set(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn chest_slots_and_count() {
        let be = cmp(vec![
            ("id", Value::String("minecraft:chest".into())),
            ("x", Value::Int(10)),
            ("y", Value::Int(64)),
            ("z", Value::Int(-20)),
            (
                "Items",
                Value::List(vec![
                    cmp(vec![
                        ("id", Value::String("minecraft:diamond".into())),
                        ("Slot", Value::Byte(3)),
                        ("count", Value::Int(64)),
                    ]),
                    cmp(vec![
                        ("id", Value::String("minecraft:stone".into())),
                        ("Slot", Value::Byte(5)),
                        ("Count", Value::Byte(12)),
                    ]),
                ]),
            ),
        ]);
        let e = extract_container(&be, "minecraft:overworld", &set(&["minecraft:chest"]), &set(&[]))
            .expect("entry");
        let m = comp(&e).unwrap();
        assert_eq!(as_i64(&m["x"]).unwrap(), 10);
        assert_eq!(m["is_dungeon"], Value::Byte(0));
        assert!(!m.contains_key("is_vault"));
        let items = as_list(&m["items"]).unwrap();
        assert_eq!(items.len(), 2);
        let first = comp(&items[0]).unwrap();
        assert_eq!(as_i64(&first["Slot"]).unwrap(), 3);
        assert_eq!(as_i64(&first["count"]).unwrap(), 64);
        // Count -> count coercion
        let second = comp(&items[1]).unwrap();
        assert_eq!(as_i64(&second["count"]).unwrap(), 12);
    }

    #[test]
    fn item_without_slot_is_skipped_for_non_vault() {
        let be = cmp(vec![
            ("id", Value::String("minecraft:barrel".into())),
            (
                "Items",
                Value::List(vec![cmp(vec![
                    ("id", Value::String("minecraft:dirt".into())),
                    ("count", Value::Int(1)),
                ])]),
            ),
        ]);
        let e = extract_container(&be, "d", &set(&["minecraft:barrel"]), &set(&[])).unwrap();
        let m = comp(&e).unwrap();
        assert_eq!(as_list(&m["items"]).unwrap().len(), 0);
    }

    #[test]
    fn empty_vault_is_dropped() {
        let be = cmp(vec![
            ("id", Value::String("the_vault:vault".into())),
            ("Items", Value::List(vec![])),
        ]);
        assert!(extract_container(&be, "d", &set(&[]), &set(&["the_vault:vault"])).is_none());
    }

    #[test]
    fn vault_aggregates_identical_items() {
        let comps = cmp(vec![("custom", Value::String("x".into()))]);
        let be = cmp(vec![
            ("id", Value::String("the_vault:vault".into())),
            (
                "Items",
                Value::List(vec![
                    cmp(vec![
                        ("id", Value::String("minecraft:gold_ingot".into())),
                        ("count", Value::Int(5)),
                        ("components", comps.clone()),
                    ]),
                    cmp(vec![
                        ("id", Value::String("minecraft:gold_ingot".into())),
                        ("count", Value::Int(3)),
                        ("components", comps.clone()),
                    ]),
                    cmp(vec![
                        ("id", Value::String("minecraft:gold_ingot".into())),
                        ("count", Value::Int(7)),
                    ]),
                ]),
            ),
        ]);
        let e = extract_container(&be, "d", &set(&[]), &set(&["the_vault:vault"])).unwrap();
        let m = comp(&e).unwrap();
        assert_eq!(m["is_vault"], Value::Byte(1));
        let items = as_list(&m["items"]).unwrap();
        // two groups: with-components (5+3=8) and without (7)
        assert_eq!(items.len(), 2);
        let with = items
            .iter()
            .find(|it| comp(it).unwrap().contains_key("components"))
            .unwrap();
        assert_eq!(as_i64(&comp(with).unwrap()["count"]).unwrap(), 8);
    }

    fn zlib(data: &[u8]) -> Vec<u8> {
        use flate2::write::ZlibEncoder;
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn region_reader_roundtrip() {
        // one chunk with a chest block entity, placed at local (0,0)
        let chunk = cmp(vec![(
            "block_entities",
            Value::List(vec![cmp(vec![
                ("id", Value::String("minecraft:chest".into())),
                ("x", Value::Int(1)),
                ("y", Value::Int(2)),
                ("z", Value::Int(3)),
                (
                    "Items",
                    Value::List(vec![cmp(vec![
                        ("id", Value::String("minecraft:emerald".into())),
                        ("Slot", Value::Byte(0)),
                        ("count", Value::Int(9)),
                    ])]),
                ),
            ])]),
        )]);
        let nbt = fastnbt::to_bytes(&chunk).unwrap();
        let comp_data = zlib(&nbt);

        let mut file = vec![0u8; 8192];
        // chunk at index 0 -> sector offset 2, sector count 1
        file[0] = 0;
        file[1] = 0;
        file[2] = 2;
        file[3] = 1;
        // sector 2 payload: 4-byte length + 1 compression byte + data
        let len = (comp_data.len() + 1) as u32;
        file.extend_from_slice(&len.to_be_bytes());
        file.push(2); // zlib
        file.extend_from_slice(&comp_data);
        // pad to sector boundary
        while file.len() % 4096 != 0 {
            file.push(0);
        }

        let dir = std::env::temp_dir().join(format!("dumper_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mca = dir.join("r.0.0.mca");
        std::fs::write(&mca, &file).unwrap();

        let mut chunks = 0;
        let mut entries: Vec<Value> = Vec::new();
        scan_region(&mca, |nbt| {
            chunks += 1;
            let parsed: ChunkBlockEntities = fastnbt::from_bytes(nbt).unwrap();
            for be in &parsed.block_entities {
                if let Some(e) =
                    extract_container(be, "minecraft:overworld", &set(&["minecraft:chest"]), &set(&[]))
                {
                    entries.push(e);
                }
            }
        });
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(chunks, 1);
        assert_eq!(entries.len(), 1);
        let m = comp(&entries[0]).unwrap();
        assert_eq!(as_str(&m["id"]).unwrap(), "minecraft:chest");
        let items = as_list(&m["items"]).unwrap();
        assert_eq!(as_i64(&comp(&items[0]).unwrap()["count"]).unwrap(), 9);
    }
}
