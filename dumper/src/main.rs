use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use fastnbt::Value;
use libdeflater::{CompressionLvl, Compressor, Decompressor};
use rayon::prelude::*;
use rustc_hash::FxHashSet;
use serde::Deserialize;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

const NO_SLOT: i64 = i64::MIN;

type IdSet = FxHashSet<String>;

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

struct Ctx {
    dec: Decompressor,
    file_buf: Vec<u8>,
    out_buf: Vec<u8>,
}

impl Ctx {
    fn new() -> Self {
        Ctx {
            dec: Decompressor::new(),
            file_buf: Vec::with_capacity(4 << 20),
            out_buf: vec![0u8; 1 << 20],
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(cfg_path) = args.get(1) else {
        eprintln!("usage: dumper <config.json>");
        std::process::exit(2);
    };

    let cfg: Config = match std::fs::read(cfg_path)
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("failed to read config: {e}");
            std::process::exit(2);
        }
    };

    if !cfg.mode.is_empty() && cfg.mode != "containers" {
        eprintln!("unsupported mode: {}", cfg.mode);
        std::process::exit(2);
    }

    let targets: IdSet = cfg.target_ids.into_iter().collect();
    let vaults: IdSet = cfg.vault_ids.into_iter().collect();

    let mut region_files: Vec<(String, PathBuf)> = Vec::new();
    for dim in &cfg.dimensions {
        let Ok(rd) = std::fs::read_dir(&dim.region_dir) else { continue };
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
        .map_init(Ctx::new, |ctx, (dim, path)| {
            let mut out: Vec<Value> = Vec::new();
            let scanned = scan_region_file(path, dim, &targets, &vaults, ctx, &mut out);

            chunks_scanned.fetch_add(scanned, Ordering::Relaxed);
            found.fetch_add(out.len(), Ordering::Relaxed);
            let done = files_done.fetch_add(1, Ordering::Relaxed) + 1;
            if done == total || done % 64 == 0 {
                println!("PROGRESS {done} {total} {}", found.load(Ordering::Relaxed));
            }
            out
        })
        .flatten_iter()
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
    let mut c = Compressor::new(CompressionLvl::default());
    let bound = c.gzip_compress_bound(data.len());
    let mut out = vec![0u8; bound];
    let n = c
        .gzip_compress(data, &mut out)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("{e:?}")))?;
    out.truncate(n);

    let tmp = path.with_extension("nbt.tmp");
    std::fs::write(&tmp, &out)?;
    std::fs::rename(&tmp, path)
}

fn region_coords(path: &Path) -> (i32, i32) {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let parts: Vec<&str> = stem.split('.').collect();
    if parts.len() == 3 && parts[0] == "r" {
        (parts[1].parse().unwrap_or(0), parts[2].parse().unwrap_or(0))
    } else {
        (0, 0)
    }
}

fn scan_region_file(
    path: &Path,
    dim: &str,
    targets: &IdSet,
    vaults: &IdSet,
    ctx: &mut Ctx,
    out: &mut Vec<Value>,
) -> usize {
    ctx.file_buf.clear();
    let Ok(mut f) = File::open(path) else { return 0 };
    if f.read_to_end(&mut ctx.file_buf).is_err() {
        return 0;
    }
    if ctx.file_buf.len() < 8192 {
        return 0;
    }

    let (rx, rz) = region_coords(path);
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut chunks = 0usize;

    for idx in 0..1024usize {
        let h = idx * 4;
        let offset = ((ctx.file_buf[h] as usize) << 16)
            | ((ctx.file_buf[h + 1] as usize) << 8)
            | (ctx.file_buf[h + 2] as usize);
        let sectors = ctx.file_buf[h + 3] as usize;
        if offset == 0 || sectors == 0 {
            continue;
        }

        let start = offset * 4096;
        if start + 5 > ctx.file_buf.len() {
            continue;
        }
        let length = u32::from_be_bytes([
            ctx.file_buf[start],
            ctx.file_buf[start + 1],
            ctx.file_buf[start + 2],
            ctx.file_buf[start + 3],
        ]) as usize;
        if length == 0 {
            continue;
        }
        let raw_ctype = ctx.file_buf[start + 4];
        let external = raw_ctype & 0x80 != 0;
        let ct = raw_ctype & 0x7f;
        chunks += 1;

        let nbt_len = if external {
            let lx = idx % 32;
            let lz = idx / 32;
            let cx = rx * 32 + lx as i32;
            let cz = rz * 32 + lz as i32;
            let ext = dir.join(format!("c.{cx}.{cz}.mcc"));
            let Ok(ext_data) = std::fs::read(&ext) else { continue };
            match decompress(&mut ctx.dec, ct, &ext_data, &mut ctx.out_buf) {
                Some(n) => n,
                None => continue,
            }
        } else {
            let ds = start + 5;
            let de = start + 4 + length;
            let Some(input) = ctx.file_buf.get(ds..de) else { continue };
            match decompress(&mut ctx.dec, ct, input, &mut ctx.out_buf) {
                Some(n) => n,
                None => continue,
            }
        };

        scan_chunk(&ctx.out_buf[..nbt_len], dim, targets, vaults, out);
    }

    chunks
}

fn decompress(dec: &mut Decompressor, ct: u8, input: &[u8], out: &mut Vec<u8>) -> Option<usize> {
    match ct {
        3 => {
            out.clear();
            out.extend_from_slice(input);
            Some(input.len())
        }
        1 | 2 => {
            let mut cap = out.capacity().max(1 << 20);
            loop {
                if out.len() < cap {
                    out.resize(cap, 0);
                }
                let r = if ct == 1 {
                    dec.gzip_decompress(input, &mut out[..cap])
                } else {
                    dec.zlib_decompress(input, &mut out[..cap])
                };
                match r {
                    Ok(n) => return Some(n),
                    Err(_) if cap < (128 << 20) => cap = cap.saturating_mul(2),
                    Err(_) => return None,
                }
            }
        }
        _ => None,
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
        10 => loop {
            let ct = *b.get(i)?;
            i += 1;
            if ct == 0 {
                break i;
            }
            let nl = read_u16(b, i)?;
            i += 2 + nl;
            i = skip_payload(b, i, ct)?;
        },
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

fn scan_chunk(b: &[u8], dim: &str, targets: &IdSet, vaults: &IdSet, out: &mut Vec<Value>) -> Option<()> {
    if *b.first()? != 10 {
        return None;
    }
    let mut i = 1usize;
    let root_name = read_u16(b, i)?;
    i += 2 + root_name;

    loop {
        let t = *b.get(i)?;
        i += 1;
        if t == 0 {
            return Some(());
        }
        let name_len = read_u16(b, i)?;
        let name = b.get(i + 2..i + 2 + name_len)?;
        i += 2 + name_len;

        if t == 9 && name == b"block_entities" {
            let et = *b.get(i)?;
            let n = read_i32(b, i + 1)?;
            i += 5;
            if et == 10 {
                for _ in 0..n {
                    let start = i;
                    let (end, id_span) = walk_block_entity(b, i)?;
                    if let Some((s, e)) = id_span {
                        if let Ok(id) = std::str::from_utf8(&b[s..e]) {
                            if targets.contains(id) || vaults.contains(id) {
                                if let Some(v) = parse_span(b, start, end) {
                                    if let Some(entry) = extract_container(&v, dim, targets, vaults) {
                                        out.push(entry);
                                    }
                                }
                            }
                        }
                    }
                    i = end;
                }
            }
            return Some(());
        }

        i = skip_payload(b, i, t)?;
    }
}

fn walk_block_entity(b: &[u8], mut i: usize) -> Option<(usize, Option<(usize, usize)>)> {
    let mut id_span = None;
    loop {
        let t = *b.get(i)?;
        i += 1;
        if t == 0 {
            return Some((i, id_span));
        }
        let name_len = read_u16(b, i)?;
        let name = b.get(i + 2..i + 2 + name_len)?;
        i += 2 + name_len;

        if t == 8 && name == b"id" {
            let sl = read_u16(b, i)?;
            let s = i + 2;
            let e = s + sl;
            let _ = b.get(s..e)?;
            id_span = Some((s, e));
            i = e;
        } else {
            i = skip_payload(b, i, t)?;
        }
    }
}

fn parse_span(b: &[u8], s: usize, e: usize) -> Option<Value> {
    let body = b.get(s..e)?;
    let mut buf = Vec::with_capacity(3 + body.len());
    buf.extend_from_slice(&[0x0A, 0x00, 0x00]);
    buf.extend_from_slice(body);
    fastnbt::from_bytes(&buf).ok()
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

fn extract_container(be_val: &Value, dimension: &str, targets: &IdSet, vaults: &IdSet) -> Option<Value> {
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
                let key = format!("{item_id}|{}", comps.map(canonical).unwrap_or_default());
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

    fn cmp_val(pairs: Vec<(&str, Value)>) -> Value {
        Value::Compound(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    fn set(ids: &[&str]) -> IdSet {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn chest_slots_and_count() {
        let be = cmp_val(vec![
            ("id", Value::String("minecraft:chest".into())),
            ("x", Value::Int(10)),
            ("y", Value::Int(64)),
            ("z", Value::Int(-20)),
            (
                "Items",
                Value::List(vec![
                    cmp_val(vec![
                        ("id", Value::String("minecraft:diamond".into())),
                        ("Slot", Value::Byte(3)),
                        ("count", Value::Int(64)),
                    ]),
                    cmp_val(vec![
                        ("id", Value::String("minecraft:stone".into())),
                        ("Slot", Value::Byte(5)),
                        ("Count", Value::Byte(12)),
                    ]),
                ]),
            ),
        ]);
        let e = extract_container(&be, "minecraft:overworld", &set(&["minecraft:chest"]), &set(&[])).unwrap();
        let m = comp(&e).unwrap();
        assert_eq!(as_i64(&m["x"]).unwrap(), 10);
        assert_eq!(m["is_dungeon"], Value::Byte(0));
        assert!(!m.contains_key("is_vault"));
        let items = as_list(&m["items"]).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(as_i64(&comp(&items[0]).unwrap()["Slot"]).unwrap(), 3);
        assert_eq!(as_i64(&comp(&items[0]).unwrap()["count"]).unwrap(), 64);
        assert_eq!(as_i64(&comp(&items[1]).unwrap()["count"]).unwrap(), 12);
    }

    #[test]
    fn item_without_slot_is_skipped_for_non_vault() {
        let be = cmp_val(vec![
            ("id", Value::String("minecraft:barrel".into())),
            (
                "Items",
                Value::List(vec![cmp_val(vec![
                    ("id", Value::String("minecraft:dirt".into())),
                    ("count", Value::Int(1)),
                ])]),
            ),
        ]);
        let e = extract_container(&be, "d", &set(&["minecraft:barrel"]), &set(&[])).unwrap();
        assert_eq!(as_list(&comp(&e).unwrap()["items"]).unwrap().len(), 0);
    }

    #[test]
    fn empty_vault_is_dropped() {
        let be = cmp_val(vec![
            ("id", Value::String("the_vault:vault".into())),
            ("Items", Value::List(vec![])),
        ]);
        assert!(extract_container(&be, "d", &set(&[]), &set(&["the_vault:vault"])).is_none());
    }

    #[test]
    fn vault_aggregates_identical_items() {
        let comps = cmp_val(vec![("custom", Value::String("x".into()))]);
        let be = cmp_val(vec![
            ("id", Value::String("the_vault:vault".into())),
            (
                "Items",
                Value::List(vec![
                    cmp_val(vec![
                        ("id", Value::String("minecraft:gold_ingot".into())),
                        ("count", Value::Int(5)),
                        ("components", comps.clone()),
                    ]),
                    cmp_val(vec![
                        ("id", Value::String("minecraft:gold_ingot".into())),
                        ("count", Value::Int(3)),
                        ("components", comps.clone()),
                    ]),
                    cmp_val(vec![
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
        assert_eq!(items.len(), 2);
        let with = items
            .iter()
            .find(|it| comp(it).unwrap().contains_key("components"))
            .unwrap();
        assert_eq!(as_i64(&comp(with).unwrap()["count"]).unwrap(), 8);
    }

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut c = Compressor::new(CompressionLvl::default());
        let bound = c.zlib_compress_bound(data.len());
        let mut out = vec![0u8; bound];
        let n = c.zlib_compress(data, &mut out).unwrap();
        out.truncate(n);
        out
    }

    #[test]
    fn region_reader_roundtrip() {
        let chunk = cmp_val(vec![(
            "block_entities",
            Value::List(vec![
                cmp_val(vec![
                    ("id", Value::String("minecraft:furnace".into())),
                    ("x", Value::Int(0)),
                    ("y", Value::Int(0)),
                    ("z", Value::Int(0)),
                ]),
                cmp_val(vec![
                    ("id", Value::String("minecraft:chest".into())),
                    ("x", Value::Int(1)),
                    ("y", Value::Int(2)),
                    ("z", Value::Int(3)),
                    (
                        "Items",
                        Value::List(vec![cmp_val(vec![
                            ("id", Value::String("minecraft:emerald".into())),
                            ("Slot", Value::Byte(0)),
                            ("count", Value::Int(9)),
                        ])]),
                    ),
                ]),
            ]),
        )]);
        let nbt = fastnbt::to_bytes(&chunk).unwrap();
        let comp_data = zlib(&nbt);

        let mut file = vec![0u8; 8192];
        file[0] = 0;
        file[1] = 0;
        file[2] = 2;
        file[3] = 1;
        let len = (comp_data.len() + 1) as u32;
        file.extend_from_slice(&len.to_be_bytes());
        file.push(2);
        file.extend_from_slice(&comp_data);
        while file.len() % 4096 != 0 {
            file.push(0);
        }

        let dir = std::env::temp_dir().join(format!("dumper_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mca = dir.join("r.0.0.mca");
        std::fs::write(&mca, &file).unwrap();

        let mut ctx = Ctx::new();
        let mut entries: Vec<Value> = Vec::new();
        let chunks = scan_region_file(&mca, "minecraft:overworld", &set(&["minecraft:chest"]), &set(&[]), &mut ctx, &mut entries);
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(chunks, 1);
        assert_eq!(entries.len(), 1);
        let m = comp(&entries[0]).unwrap();
        assert_eq!(as_str(&m["id"]).unwrap(), "minecraft:chest");
        assert_eq!(as_i64(&comp(&as_list(&m["items"]).unwrap()[0]).unwrap()["count"]).unwrap(), 9);
    }

    #[test]
    fn scan_chunk_skips_non_targets() {
        let chunk = cmp_val(vec![(
            "block_entities",
            Value::List(vec![
                cmp_val(vec![("id", Value::String("minecraft:sign".into()))]),
                cmp_val(vec![
                    ("id", Value::String("minecraft:barrel".into())),
                    ("x", Value::Int(4)),
                    ("y", Value::Int(5)),
                    ("z", Value::Int(6)),
                    (
                        "Items",
                        Value::List(vec![cmp_val(vec![
                            ("id", Value::String("minecraft:gold_ingot".into())),
                            ("Slot", Value::Byte(1)),
                            ("count", Value::Int(2)),
                        ])]),
                    ),
                ]),
            ]),
        )]);
        let nbt = fastnbt::to_bytes(&chunk).unwrap();
        let mut out = Vec::new();
        scan_chunk(&nbt, "d", &set(&["minecraft:barrel"]), &set(&[]), &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(as_str(&comp(&out[0]).unwrap()["id"]).unwrap(), "minecraft:barrel");
    }
}
