use std::collections::HashMap;
use std::fs::File;
use std::sync::{Arc, Mutex};

use memmap2::Mmap;
use rayon::prelude::*;

use crate::model::{Entry, EntryKind, Item};
use crate::parse::containers::{self, JsonKind, RawElement};
use crate::parse::dump_nbt::{self, DumpKind};

pub enum Load {
    Backpacks,
    Json(Option<JsonKind>),
    Nbt(Option<DumpKind>),
}

impl Load {
    pub fn auto(path: &str) -> Load {
        if path.to_lowercase().ends_with(".json") {
            Load::Json(None)
        } else {
            Load::Nbt(None)
        }
    }
}

const F_DUNGEON: u8 = 1;
const F_HAS_ITEMS: u8 = 2;
const F_HAS_UPGRADES: u8 = 4;

#[derive(Default)]
pub struct Interner {
    vals: Vec<String>,
    map: HashMap<String, u32>,
}

impl Interner {
    pub fn intern(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.map.get(s) {
            return i;
        }
        let i = self.vals.len() as u32;
        self.vals.push(s.to_string());
        self.map.insert(s.to_string(), i);
        i
    }
    pub fn get(&self, i: u32) -> &str {
        &self.vals[i as usize]
    }
}

pub struct EntryMeta {
    pub kind: EntryKind,
    pub icon: u32,
    pub dim: u32,
    pub owner: Box<str>,
    pub uuid: Box<str>,
    pub coords: Option<[i32; 3]>,
    pub flags: u8,
    pub rows: u16,
    pub meta_len: u8,
    pub max_stack: i32,
    pub enchants: Box<[(u32, i16)]>,
}

impl EntryMeta {
    pub fn is_dungeon(&self) -> bool {
        self.flags & F_DUNGEON != 0
    }
    pub fn has_items(&self) -> bool {
        self.flags & F_HAS_ITEMS != 0
    }
    pub fn has_upgrades(&self) -> bool {
        self.flags & F_HAS_UPGRADES != 0
    }
    pub fn coords64(&self) -> Option<(i64, i64, i64)> {
        self.coords.map(|[x, y, z]| (x as i64, y as i64, z as i64))
    }
}

struct MetaOwned {
    kind: EntryKind,
    icon: String,
    dim: String,
    owner: String,
    uuid: String,
    coords: Option<[i32; 3]>,
    flags: u8,
    rows: u16,
    meta_len: u8,
    max_stack: i32,
    enchants: Vec<(String, i16)>,
}

impl MetaOwned {
    fn from_entry(e: &Entry) -> Self {
        let mut flags = 0;
        if e.is_dungeon {
            flags |= F_DUNGEON;
        }
        if !e.items.is_empty() {
            flags |= F_HAS_ITEMS;
        }
        if !e.upgrades.is_empty() {
            flags |= F_HAS_UPGRADES;
        }
        MetaOwned {
            kind: e.kind,
            icon: e.header_icon.clone(),
            dim: e.dimension.clone(),
            owner: e.owner.clone(),
            uuid: e.uuid.clone(),
            coords: e.coords.map(|(x, y, z)| [x as i32, y as i32, z as i32]),
            flags,
            rows: e.rows.min(u16::MAX as usize) as u16,
            meta_len: e.meta.len().min(u8::MAX as usize) as u8,
            max_stack: e.max_stack.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
            enchants: e
                .all_enchants
                .iter()
                .map(|(n, l)| (n.clone(), (*l).clamp(i16::MIN as i32, i16::MAX as i32) as i16))
                .collect(),
        }
    }
    fn intern(self, it: &mut Interner) -> EntryMeta {
        EntryMeta {
            kind: self.kind,
            icon: it.intern(&self.icon),
            dim: it.intern(&self.dim),
            owner: self.owner.into_boxed_str(),
            uuid: self.uuid.into_boxed_str(),
            coords: self.coords,
            flags: self.flags,
            rows: self.rows,
            meta_len: self.meta_len,
            max_stack: self.max_stack,
            enchants: self
                .enchants
                .iter()
                .map(|(n, l)| (it.intern(n), *l))
                .collect(),
        }
    }
}

pub enum TextSource<'a> {
    Slice(&'a [u8]),
    Blob {
        search: &'a str,
        nbt: &'a str,
        upgrades: &'a [Item],
    },
}

enum Backend {
    Json {
        mmap: Arc<Mmap>,
        spans: Vec<(u32, u32)>,
        locs: Vec<(u32, u8)>,
        kind: JsonKind,
    },
    Nbt {
        bytes: Arc<Vec<u8>>,
        spans: Vec<(u32, u32)>,
        locs: Vec<(u32, u8)>,
        kind: DumpKind,
    },
    Mem {
        entries: Vec<Entry>,
    },
}

const CACHE_CAP: usize = 4096;

pub struct Store {
    metas: Vec<EntryMeta>,
    interner: Interner,
    backend: Backend,
    overrides: HashMap<String, String>,
    cache: Mutex<HashMap<usize, Arc<Entry>>>,
}

impl Store {
    pub fn open(path: &str, load: Load) -> Result<Store, String> {
        match load {
            Load::Backpacks => Self::from_entries(crate::parse::nbt::load_backpacks(path)?),
            Load::Json(forced) => Self::open_json(path, forced),
            Load::Nbt(forced) => Self::open_nbt(path, forced),
        }
    }

    fn from_entries(entries: Vec<Entry>) -> Result<Store, String> {
        let mut interner = Interner::default();
        let metas = entries
            .iter()
            .map(|e| MetaOwned::from_entry(e).intern(&mut interner))
            .collect();
        Ok(Store {
            metas,
            interner,
            backend: Backend::Mem { entries },
            overrides: HashMap::new(),
            cache: Mutex::new(HashMap::new()),
        })
    }

    fn open_nbt(path: &str, forced: Option<DumpKind>) -> Result<Store, String> {
        let raw = std::fs::read(path).map_err(|e| format!("read: {e}"))?;
        let bytes = crate::parse::nbt::decompress(&raw);
        let (detected, spans) = match dump_nbt::split_dump(&bytes) {
            Ok(v) => v,
            Err(e) => {
                if forced.is_none() {
                    return Self::from_entries(crate::parse::nbt::load_backpacks_bytes(&bytes)?);
                }
                return Err(e);
            }
        };
        let kind = forced.unwrap_or(detected);

        const CHUNK: usize = 16384;
        let mut interner = Interner::default();
        let mut metas = Vec::with_capacity(spans.len());
        let mut locs = Vec::with_capacity(spans.len());
        for (ci, chunk) in spans.chunks(CHUNK).enumerate() {
            let base = ci * CHUNK;
            let built: Vec<Vec<(MetaOwned, (u32, u8))>> = chunk
                .par_iter()
                .enumerate()
                .map(|(local, &span)| {
                    let spi = (base + local) as u32;
                    dump_nbt::build_one(&bytes, span, kind)
                        .iter()
                        .enumerate()
                        .map(|(sub, ent)| (MetaOwned::from_entry(ent), (spi, sub as u8)))
                        .collect()
                })
                .collect();
            for group in built {
                for (m, loc) in group {
                    metas.push(m.intern(&mut interner));
                    locs.push(loc);
                }
            }
        }

        Ok(Store {
            metas,
            interner,
            backend: Backend::Nbt {
                bytes: Arc::new(bytes),
                spans,
                locs,
                kind,
            },
            overrides: HashMap::new(),
            cache: Mutex::new(HashMap::new()),
        })
    }

    fn open_json(path: &str, forced: Option<JsonKind>) -> Result<Store, String> {
        let file = File::open(path).map_err(|e| format!("open: {e}"))?;
        let mmap = unsafe { Mmap::map(&file) }.map_err(|e| format!("mmap: {e}"))?;
        let (kind, spans) = index_json(&mmap, forced)?;

        const CHUNK: usize = 16384;
        let mut interner = Interner::default();
        let mut metas = Vec::with_capacity(spans.len());
        let mut locs = Vec::with_capacity(spans.len());
        for (ci, chunk) in spans.chunks(CHUNK).enumerate() {
            let base = ci * CHUNK;
            let built: Vec<Vec<(MetaOwned, (u32, u8))>> = chunk
                .par_iter()
                .enumerate()
                .map(|(local, &(s, e))| {
                    let el: RawElement = match serde_json::from_slice(&mmap[s as usize..e as usize])
                    {
                        Ok(v) => v,
                        Err(_) => return Vec::new(),
                    };
                    let spi = (base + local) as u32;
                    containers::build_one(&el, kind)
                        .iter()
                        .enumerate()
                        .map(|(sub, ent)| (MetaOwned::from_entry(ent), (spi, sub as u8)))
                        .collect()
                })
                .collect();
            for group in built {
                for (m, loc) in group {
                    metas.push(m.intern(&mut interner));
                    locs.push(loc);
                }
            }
        }

        Ok(Store {
            metas,
            interner,
            backend: Backend::Json {
                mmap: Arc::new(mmap),
                spans,
                locs,
                kind,
            },
            overrides: HashMap::new(),
            cache: Mutex::new(HashMap::new()),
        })
    }

    pub fn len(&self) -> usize {
        self.metas.len()
    }

    pub fn metas(&self) -> &[EntryMeta] {
        &self.metas
    }

    pub fn first_kind(&self) -> Option<EntryKind> {
        self.metas.first().map(|m| m.kind)
    }

    pub fn mem_entries(&self) -> &[Entry] {
        match &self.backend {
            Backend::Mem { entries } => entries,
            _ => &[],
        }
    }

    pub fn text_source(&self, i: usize) -> TextSource<'_> {
        match &self.backend {
            Backend::Json { mmap, spans, locs, .. } => {
                let (spi, _) = locs[i];
                let (s, e) = spans[spi as usize];
                TextSource::Slice(&mmap[s as usize..e as usize])
            }
            Backend::Nbt { bytes, spans, locs, .. } => {
                let (spi, _) = locs[i];
                let (s, e) = spans[spi as usize];
                TextSource::Slice(&bytes[s as usize..e as usize])
            }
            Backend::Mem { entries } => {
                let e = &entries[i];
                TextSource::Blob {
                    search: &e.search_blob,
                    nbt: &e.nbt_blob,
                    upgrades: &e.upgrades,
                }
            }
        }
    }

    pub fn entry(&self, i: usize) -> Option<Arc<Entry>> {
        if let Some(hit) = self.cache.lock().unwrap().get(&i) {
            return Some(hit.clone());
        }
        let mut entry = match &self.backend {
            Backend::Json { mmap, spans, locs, kind } => {
                let (spi, sub) = *locs.get(i)?;
                let (s, e) = spans[spi as usize];
                let el: RawElement = serde_json::from_slice(&mmap[s as usize..e as usize]).ok()?;
                containers::build_one(&el, *kind).into_iter().nth(sub as usize)?
            }
            Backend::Nbt { bytes, spans, locs, kind } => {
                let (spi, sub) = *locs.get(i)?;
                dump_nbt::build_one(bytes, spans[spi as usize], *kind)
                    .into_iter()
                    .nth(sub as usize)?
            }
            Backend::Mem { entries } => entries.get(i)?.clone(),
        };
        if !self.overrides.is_empty() && !entry.uuid.is_empty() {
            if let Some(name) = self.overrides.get(&entry.uuid) {
                apply_name(&mut entry, name);
            }
        }
        let arc = Arc::new(entry);
        let mut cache = self.cache.lock().unwrap();
        if cache.len() >= CACHE_CAP {
            cache.clear();
        }
        cache.insert(i, arc.clone());
        Some(arc)
    }

    pub fn filter(&self, c: &crate::search::Compiled) -> Vec<u32> {
        (0..self.metas.len() as u32)
            .into_par_iter()
            .filter(|&i| {
                c.matches_meta(&self.metas[i as usize], &self.text_source(i as usize), &self.interner)
            })
            .collect()
    }

    pub fn apply_username(&mut self, uuid: &str, name: &str) -> bool {
        let mut changed = false;
        for m in &mut self.metas {
            if m.uuid.as_ref() != uuid {
                continue;
            }
            if m.owner.is_empty() || m.owner.as_ref() == "unknown" {
                m.owner = name.to_lowercase().into_boxed_str();
                changed = true;
            }
        }
        if let Backend::Mem { entries } = &mut self.backend {
            for e in entries {
                if e.uuid == uuid && (e.owner.is_empty() || e.owner == "unknown") {
                    apply_name(e, name);
                }
            }
        }
        if changed {
            self.overrides.insert(uuid.to_string(), name.to_string());
            self.cache.lock().unwrap().clear();
        }
        changed
    }
}

fn apply_name(e: &mut Entry, name: &str) {
    e.title = e.title.replacen("Unknown", name, 1);
    for (label, value) in &mut e.meta {
        if (label == "Owner" || label == "Player") && value == "Unknown" {
            *value = name.to_string();
        }
    }
    e.owner = name.to_lowercase();
    if !e.search_blob.contains(&e.owner) {
        e.search_blob.push('\n');
        e.search_blob.push_str(&e.owner);
    }
}

pub fn ci_contains(hay: &[u8], needle: &str) -> bool {
    let n = needle.as_bytes();
    if n.is_empty() {
        return true;
    }
    if hay.len() < n.len() {
        return false;
    }
    let first = n[0];
    let last = hay.len() - n.len();
    let mut i = 0;
    while i <= last {
        if hay[i].to_ascii_lowercase() == first
            && hay[i..i + n.len()]
                .iter()
                .zip(n)
                .all(|(a, b)| a.to_ascii_lowercase() == *b)
        {
            return true;
        }
        i += 1;
    }
    false
}

fn index_json(bytes: &[u8], forced: Option<JsonKind>) -> Result<(JsonKind, Vec<(u32, u32)>), String> {
    let start = skip_ws(bytes, 0);
    if start >= bytes.len() {
        return Err("empty json".into());
    }
    let spans = match bytes[start] {
        b'[' => split_array(bytes, start),
        b'{' => match find_member_array(bytes, start) {
            Some(arr_open) => split_array(bytes, arr_open),
            None => vec![(start as u32, trim_end(bytes, bytes.len()) as u32)],
        },
        _ => return Err("unexpected json shape".into()),
    };
    let kind = match forced {
        Some(k) => k,
        None => detect_kind(bytes, &spans),
    };
    Ok((kind, spans))
}

fn detect_kind(bytes: &[u8], spans: &[(u32, u32)]) -> JsonKind {
    if let Some(&(s, e)) = spans.first() {
        if let Ok(el) = serde_json::from_slice::<RawElement>(&bytes[s as usize..e as usize]) {
            return containers::detect_kind(&el);
        }
    }
    JsonKind::Containers
}

fn skip_ws(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

fn trim_end(bytes: &[u8], mut end: usize) -> usize {
    while end > 0 && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    end
}

fn skip_string(bytes: &[u8], mut i: usize) -> usize {
    i += 1;
    let mut esc = false;
    while i < bytes.len() {
        let c = bytes[i];
        if esc {
            esc = false;
        } else if c == b'\\' {
            esc = true;
        } else if c == b'"' {
            return i + 1;
        }
        i += 1;
    }
    i
}

fn skip_value(bytes: &[u8], i: usize) -> usize {
    match bytes.get(i) {
        Some(b'"') => skip_string(bytes, i),
        Some(b'{') | Some(b'[') => {
            let mut depth = 0;
            let mut j = i;
            while j < bytes.len() {
                match bytes[j] {
                    b'"' => {
                        j = skip_string(bytes, j);
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return j + 1;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            j
        }
        _ => {
            let mut j = i;
            while j < bytes.len() && !matches!(bytes[j], b',' | b'}' | b']') {
                j += 1;
            }
            j
        }
    }
}

fn find_member_array(bytes: &[u8], obj_open: usize) -> Option<usize> {
    let mut i = skip_ws(bytes, obj_open + 1);
    while i < bytes.len() && bytes[i] != b'}' {
        if bytes[i] != b'"' {
            return None;
        }
        let key_start = i + 1;
        let key_end = skip_string(bytes, i) - 1;
        let key = &bytes[key_start..key_end];
        i = skip_ws(bytes, key_end + 1);
        if i >= bytes.len() || bytes[i] != b':' {
            return None;
        }
        i = skip_ws(bytes, i + 1);
        if (key == b"containers" || key == b"players") && bytes.get(i) == Some(&b'[') {
            return Some(i);
        }
        i = skip_value(bytes, i);
        i = skip_ws(bytes, i);
        if bytes.get(i) == Some(&b',') {
            i = skip_ws(bytes, i + 1);
        }
    }
    None
}

fn split_array(bytes: &[u8], open_idx: usize) -> Vec<(u32, u32)> {
    let mut spans = Vec::new();
    let mut i = open_idx + 1;
    let mut depth = 0i32;
    let mut elem_start: Option<usize> = None;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'"' => {
                if elem_start.is_none() {
                    elem_start = Some(i);
                }
                i = skip_string(bytes, i);
                continue;
            }
            b'{' | b'[' => {
                if elem_start.is_none() {
                    elem_start = Some(i);
                }
                depth += 1;
            }
            b'}' | b']' => {
                if depth == 0 {
                    if let Some(s) = elem_start.take() {
                        spans.push((s as u32, trim_end(bytes, i) as u32));
                    }
                    break;
                }
                depth -= 1;
            }
            b',' if depth == 0 => {
                if let Some(s) = elem_start.take() {
                    spans.push((s as u32, trim_end(bytes, i) as u32));
                }
            }
            c if !c.is_ascii_whitespace() => {
                if elem_start.is_none() {
                    elem_start = Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    spans
}
