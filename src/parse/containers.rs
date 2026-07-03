
use std::fs::File;
use std::io::BufReader;

use rayon::prelude::*;
use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use serde_json::Value as J;

use crate::model::{prettify_id, Entry, EntryKind, Item};
use crate::parse::nbt::extract_text;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum JsonKind {
    Containers,
    Players,
}

#[derive(Default, Deserialize)]
pub(crate) struct RawElement {
    pub id: Option<J>,
    pub x: Option<J>,
    pub y: Option<J>,
    pub z: Option<J>,
    pub dimension: Option<J>,
    pub is_dungeon: Option<J>,
    pub items: Option<J>,
    pub name: Option<J>,
    pub uuid: Option<J>,
    pub inventory: Option<J>,
    pub ender_chest: Option<J>,
}

pub fn load_json(path: &str, forced: Option<JsonKind>) -> Result<Vec<Entry>, String> {
    let file = File::open(path).map_err(|e| format!("read: {e}"))?;
    let reader = BufReader::with_capacity(1 << 20, file);
    let mut de = serde_json::Deserializer::from_reader(reader);
    RootSeed { forced }
        .deserialize(&mut de)
        .map_err(|e| format!("json parse: {e}"))
}

pub fn load_containers(path: &str) -> Result<Vec<Entry>, String> {
    load_json(path, None)
}

const BATCH: usize = 4096;

struct RootSeed {
    forced: Option<JsonKind>,
}

impl<'de> DeserializeSeed<'de> for RootSeed {
    type Value = Vec<Entry>;
    fn deserialize<D>(self, d: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        d.deserialize_any(RootVisitor { forced: self.forced })
    }
}

struct RootVisitor {
    forced: Option<JsonKind>,
}

impl<'de> Visitor<'de> for RootVisitor {
    type Value = Vec<Entry>;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a containers/players array, wrapper object, or single element")
    }

    fn visit_seq<A>(self, seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        drain_seq(seq, self.forced)
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut result: Option<Vec<Entry>> = None;
        let mut leftover = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if result.is_none() && (key == "containers" || key == "players") {
                result = Some(map.next_value_seed(ArraySeed { forced: self.forced })?);
            } else {
                leftover.insert(key, map.next_value::<J>()?);
            }
        }
        if let Some(entries) = result {
            return Ok(entries);
        }
        let el: RawElement =
            serde_json::from_value(J::Object(leftover)).map_err(serde::de::Error::custom)?;
        let kind = self.forced.unwrap_or_else(|| detect_kind(&el));
        Ok(build_one(&el, kind))
    }
}

struct ArraySeed {
    forced: Option<JsonKind>,
}

impl<'de> DeserializeSeed<'de> for ArraySeed {
    type Value = Vec<Entry>;
    fn deserialize<D>(self, d: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        d.deserialize_seq(ArrayVisitor { forced: self.forced })
    }
}

struct ArrayVisitor {
    forced: Option<JsonKind>,
}

impl<'de> Visitor<'de> for ArrayVisitor {
    type Value = Vec<Entry>;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("an array of containers/players")
    }
    fn visit_seq<A>(self, seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        drain_seq(seq, self.forced)
    }
}

fn drain_seq<'de, A>(mut seq: A, forced: Option<JsonKind>) -> Result<Vec<Entry>, A::Error>
where
    A: SeqAccess<'de>,
{
    let mut out: Vec<Entry> = Vec::new();
    let mut kind = forced;
    let mut batch: Vec<RawElement> = Vec::with_capacity(BATCH);
    while let Some(el) = seq.next_element::<RawElement>()? {
        if kind.is_none() {
            kind = Some(detect_kind(&el));
        }
        batch.push(el);
        if batch.len() >= BATCH {
            flush_batch(&mut batch, kind.unwrap(), &mut out);
        }
    }
    flush_batch(&mut batch, kind.unwrap_or(JsonKind::Containers), &mut out);
    Ok(out)
}

fn flush_batch(batch: &mut Vec<RawElement>, kind: JsonKind, out: &mut Vec<Entry>) {
    if batch.is_empty() {
        return;
    }
    let mut entries: Vec<Entry> = match kind {
        JsonKind::Containers => batch.par_iter().filter_map(build_container).collect(),
        JsonKind::Players => batch
            .par_iter()
            .flat_map(crate::parse::players::build_player)
            .collect(),
    };
    out.append(&mut entries);
    batch.clear();
}

pub(crate) fn build_one(el: &RawElement, kind: JsonKind) -> Vec<Entry> {
    match kind {
        JsonKind::Containers => build_container(el).into_iter().collect(),
        JsonKind::Players => crate::parse::players::build_player(el),
    }
}

pub(crate) fn detect_kind(el: &RawElement) -> JsonKind {
    if el.ender_chest.is_some() || (el.uuid.is_some() && el.inventory.is_some()) {
        JsonKind::Players
    } else {
        JsonKind::Containers
    }
}

fn j_str(v: &Option<J>) -> Option<String> {
    match v.as_ref()? {
        J::String(s) => Some(s.clone()),
        J::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

pub(crate) fn build_container(el: &RawElement) -> Option<Entry> {
    let id = j_str(&el.id).unwrap_or_else(|| "minecraft:chest".into());
    let x = j_str(&el.x).unwrap_or_else(|| "?".into());
    let y = j_str(&el.y).unwrap_or_else(|| "?".into());
    let z = j_str(&el.z).unwrap_or_else(|| "?".into());
    let dimension = j_str(&el.dimension).unwrap_or_else(|| "minecraft:overworld".into());
    let is_dungeon = el
        .is_dungeon
        .as_ref()
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let items = parse_items(el.items.as_ref());

    let coords = match (
        el.x.as_ref().and_then(coord),
        el.y.as_ref().and_then(coord),
        el.z.as_ref().and_then(coord),
    ) {
        (Some(x), Some(y), Some(z)) => Some((x, y, z)),
        _ => None,
    };
    let mut nbt_blob = String::new();
    if let Some(items_json) = el.items.as_ref() {
        collect_nbt(items_json, &mut nbt_blob);
    }

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

    let mut stock = None;
    let mut air = None;
    let is_toolbox = item.id.contains("toolbox");
    for root in [v.get("components"), v.get("tag"), v.get("nbt")].into_iter().flatten() {
        with_object(root, |obj| {
            stock = stock
                .or_else(|| json_find_num(obj, "tagStock"))
                .or_else(|| json_find_num(obj, "tag_stock"));
            air = air
                .or_else(|| json_find_num(obj, "create:banktank_air"))
                .or_else(|| json_find_num(obj, "Air"));
            if is_toolbox && item.contents.is_empty() {
                extract_toolbox_json(obj, &mut item.contents);
            }
        });
    }
    let capacity_ench = item
        .enchants
        .iter()
        .find(|(id, _)| id.contains("create:capacity"))
        .map(|(_, l)| *l);
    item.apply_gauges(stock, air, capacity_ench);
    Some(item)
}

fn json_find_num(v: &J, key: &str) -> Option<i64> {
    match v {
        J::Object(m) => {
            for (k, val) in m {
                if k.eq_ignore_ascii_case(key) {
                    if let Some(n) = val.as_i64().or_else(|| val.as_f64().map(|f| f as i64)) {
                        return Some(n);
                    }
                }
            }
            m.values().find_map(|val| json_find_num(val, key))
        }
        J::Array(a) => a.iter().find_map(|e| json_find_num(e, key)),
        _ => None,
    }
}

fn find_json<'a>(v: &'a J, key: &str) -> Option<&'a J> {
    match v {
        J::Object(m) => {
            if let Some(val) = m.get(key) {
                return Some(val);
            }
            m.values().find_map(|val| find_json(val, key))
        }
        J::Array(a) => a.iter().find_map(|e| find_json(e, key)),
        _ => None,
    }
}

fn extract_toolbox_json(v: &J, out: &mut Vec<Item>) {
    let Some(inv) = find_json(v, "create:toolbox_inventory") else { return };
    let inner = inv
        .get("items")
        .and_then(|i| i.get("items"))
        .or_else(|| inv.get("items"));
    match inner {
        Some(J::Object(m)) => {
            for (slot, it) in m {
                if it.get("id").is_some() {
                    let s = slot.parse::<i32>().unwrap_or(out.len() as i32);
                    if let Some(item) = item_from_json(it, s) {
                        out.push(item);
                    }
                }
            }
        }
        Some(J::Array(a)) => {
            for it in a {
                if let Some(item) = item_from_json(it, out.len() as i32) {
                    out.push(item);
                }
            }
        }
        _ => {}
    }
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
    if let Some(m) = num(comps.get("minecraft:max_damage")) {
        item.max_damage = Some(m as i32);
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
