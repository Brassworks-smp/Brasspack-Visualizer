
use crate::model::{Entry, EntryKind, Item};
use crate::store::{ci_contains, EntryMeta, Interner, TextSource};

#[derive(Clone, Debug)]
pub enum Expr {
    Term(String),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

impl Expr {
    pub fn eval(&self, has: &dyn Fn(&str) -> bool) -> bool {
        match self {
            Expr::Term(t) => has(t),
            Expr::Not(e) => !e.eval(has),
            Expr::And(a, b) => a.eval(has) && b.eval(has),
            Expr::Or(a, b) => a.eval(has) || b.eval(has),
        }
    }
}

#[derive(Clone, PartialEq)]
enum Tok {
    And,
    Or,
    Not,
    LParen,
    RParen,
    Term(String),
}

fn tokenize(s: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            c if c.is_whitespace() => {
                chars.next();
            }
            '(' => {
                chars.next();
                out.push(Tok::LParen);
            }
            ')' => {
                chars.next();
                out.push(Tok::RParen);
            }
            '"' | '\'' => {
                chars.next();
                let mut buf = String::new();
                for ch in chars.by_ref() {
                    if ch == c {
                        break;
                    }
                    buf.push(ch);
                }
                if !buf.is_empty() {
                    out.push(Tok::Term(buf.to_lowercase()));
                }
            }
            '&' => {
                chars.next();
                if chars.peek() == Some(&'&') {
                    chars.next();
                }
                out.push(Tok::And);
            }
            '|' => {
                chars.next();
                if chars.peek() == Some(&'|') {
                    chars.next();
                }
                out.push(Tok::Or);
            }
            _ => {
                let mut buf = String::new();
                while let Some(&ch) = chars.peek() {
                    if ch.is_whitespace() || matches!(ch, '(' | ')' | '"' | '\'' | '&' | '|') {
                        break;
                    }
                    buf.push(ch);
                    chars.next();
                }
                match buf.to_ascii_lowercase().as_str() {
                    "and" => out.push(Tok::And),
                    "or" => out.push(Tok::Or),
                    "not" => out.push(Tok::Not),
                    _ => out.push(Tok::Term(buf.to_lowercase())),
                }
            }
        }
    }
    out
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn eat(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn parse_or(&mut self) -> Option<Expr> {
        let mut left = self.parse_and()?;
        while matches!(self.peek(), Some(Tok::Or)) {
            self.eat();
            match self.parse_and() {
                Some(right) => left = Expr::Or(Box::new(left), Box::new(right)),
                None => break,
            }
        }
        Some(left)
    }

    fn parse_and(&mut self) -> Option<Expr> {
        let mut left = self.parse_unary()?;
        loop {
            match self.peek() {
                Some(Tok::And) => {
                    self.eat();
                    match self.parse_unary() {
                        Some(right) => left = Expr::And(Box::new(left), Box::new(right)),
                        None => break,
                    }
                }
                Some(Tok::Term(_)) | Some(Tok::Not) | Some(Tok::LParen) => {
                    match self.parse_unary() {
                        Some(right) => left = Expr::And(Box::new(left), Box::new(right)),
                        None => break,
                    }
                }
                _ => break,
            }
        }
        Some(left)
    }

    fn parse_unary(&mut self) -> Option<Expr> {
        match self.peek() {
            Some(Tok::Not) => {
                self.eat();
                Some(Expr::Not(Box::new(self.parse_unary()?)))
            }
            Some(Tok::LParen) => {
                self.eat();
                let e = self.parse_or();
                if matches!(self.peek(), Some(Tok::RParen)) {
                    self.eat();
                }
                e
            }
            Some(Tok::Term(_)) => {
                if let Some(Tok::Term(t)) = self.eat() {
                    Some(Expr::Term(t))
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

pub fn parse_query(s: &str) -> Option<Expr> {
    let toks = tokenize(s);
    if toks.is_empty() {
        return None;
    }
    let mut p = Parser { toks, pos: 0 };
    p.parse_or()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EnchOp {
    Any,
    Gte,
    Eq,
    Gt,
}

impl EnchOp {
    pub fn label(self) -> &'static str {
        match self {
            EnchOp::Any => "any level",
            EnchOp::Gte => "level ≥",
            EnchOp::Eq => "level =",
            EnchOp::Gt => "level >",
        }
    }
    pub fn test(self, level: i32, target: i32) -> bool {
        match self {
            EnchOp::Any => true,
            EnchOp::Gte => level >= target,
            EnchOp::Eq => level == target,
            EnchOp::Gt => level > target,
        }
    }
}

pub struct Highlight {
    text: Option<Expr>,
    ench_name: Option<String>,
    ench_op: EnchOp,
    ench_level: i32,
}

fn item_has_term(item: &Item, term: &str) -> bool {
    ci_contains(item.id.as_bytes(), term)
        || item
            .custom_name
            .as_deref()
            .is_some_and(|n| ci_contains(n.as_bytes(), term))
        || item.lore.iter().any(|l| ci_contains(l.as_bytes(), term))
        || item.enchants.iter().any(|(e, _)| ci_contains(e.as_bytes(), term))
        || item
            .head_ref
            .as_deref()
            .is_some_and(|r| ci_contains(r.as_bytes(), term))
}

impl Highlight {
    pub fn item_matches(&self, item: &Item) -> bool {
        let text_ok = self
            .text
            .as_ref()
            .map_or(true, |e| e.eval(&|t| item_has_term(item, t)));
        let ench_ok = if self.ench_name.is_some() || self.ench_op != EnchOp::Any {
            item.enchants.iter().any(|(id, lvl)| {
                self.ench_name
                    .as_ref()
                    .map_or(true, |n| id.to_lowercase().contains(n))
                    && self.ench_op.test(*lvl, self.ench_level)
            })
        } else {
            true
        };
        text_ok && ench_ok
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TextCat {
    Any,
    Owner,
    Item,
    Type,
    Upgrade,
}

impl TextCat {
    pub fn label(self) -> &'static str {
        match self {
            TextCat::Any => "Anything",
            TextCat::Owner => "Owner / UUID",
            TextCat::Item => "Item",
            TextCat::Type => "Container type",
            TextCat::Upgrade => "Upgrade",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DungeonFilter {
    Any,
    Only,
    Hide,
}

impl DungeonFilter {
    pub fn label(self) -> &'static str {
        match self {
            DungeonFilter::Any => "Any",
            DungeonFilter::Only => "Only dungeon",
            DungeonFilter::Hide => "Hide dungeon",
        }
    }
}

#[derive(Clone)]
pub struct Filters {
    pub text: String,
    pub cat: TextCat,
    pub show_backpacks: bool,
    pub show_containers: bool,
    pub show_players: bool,
    pub player: String,
    pub item: String,
    pub ctype: String,
    pub dimension: String,
    pub nbt: String,
    pub dungeon: DungeonFilter,
    pub hide_empty: bool,
    pub min_count: String,
    pub x_min: String,
    pub x_max: String,
    pub y_min: String,
    pub y_max: String,
    pub z_min: String,
    pub z_max: String,
    pub ench_name: String,
    pub ench_op: EnchOp,
    pub ench_level: i32,
}

impl Default for Filters {
    fn default() -> Self {
        Filters {
            text: String::new(),
            cat: TextCat::Any,
            show_backpacks: true,
            show_containers: true,
            show_players: true,
            player: String::new(),
            item: String::new(),
            ctype: String::new(),
            dimension: String::new(),
            nbt: String::new(),
            dungeon: DungeonFilter::Any,
            hide_empty: true,
            min_count: String::new(),
            x_min: String::new(),
            x_max: String::new(),
            y_min: String::new(),
            y_max: String::new(),
            z_min: String::new(),
            z_max: String::new(),
            ench_name: String::new(),
            ench_op: EnchOp::Any,
            ench_level: 255,
        }
    }
}

impl Filters {
    pub fn advanced_active(&self) -> bool {
        !self.show_backpacks
            || !self.show_containers
            || !self.show_players
            || !self.player.trim().is_empty()
            || !self.item.trim().is_empty()
            || !self.ctype.trim().is_empty()
            || !self.dimension.trim().is_empty()
            || !self.nbt.trim().is_empty()
            || self.dungeon != DungeonFilter::Any
            || !self.hide_empty
            || !self.min_count.trim().is_empty()
            || [
                &self.x_min, &self.x_max, &self.y_min, &self.y_max, &self.z_min, &self.z_max,
            ]
            .iter()
            .any(|s| !s.trim().is_empty())
            || !self.ench_name.trim().is_empty()
            || self.ench_op != EnchOp::Any
    }

    pub fn clear_advanced(&mut self) {
        let quick = (self.text.clone(), self.cat);
        *self = Filters::default();
        self.text = quick.0;
        self.cat = quick.1;
    }

    pub fn highlight(&self) -> Option<Highlight> {
        let text = if matches!(self.cat, TextCat::Any | TextCat::Item) {
            parse_query(&self.text)
        } else {
            None
        };
        let ench_name = {
            let t = self.ench_name.trim().to_lowercase();
            if t.is_empty() {
                None
            } else {
                Some(t)
            }
        };
        let ench_active = ench_name.is_some() || self.ench_op != EnchOp::Any;
        if text.is_none() && !ench_active {
            return None;
        }
        Some(Highlight {
            text,
            ench_name,
            ench_op: self.ench_op,
            ench_level: self.ench_level,
        })
    }

    pub fn compile(&self) -> Compiled {
        let low = |s: &str| {
            let t = s.trim().to_lowercase();
            if t.is_empty() {
                None
            } else {
                Some(t)
            }
        };
        let parse = |s: &str| s.trim().parse::<i64>().ok();
        Compiled {
            text: parse_query(&self.text),
            cat: self.cat,
            show_backpacks: self.show_backpacks,
            show_containers: self.show_containers,
            show_players: self.show_players,
            player: low(&self.player),
            item: low(&self.item),
            ctype: low(&self.ctype),
            dimension: low(&self.dimension),
            nbt: low(&self.nbt),
            dungeon: self.dungeon,
            hide_empty: self.hide_empty,
            min_count: parse(&self.min_count),
            x: (parse(&self.x_min), parse(&self.x_max)),
            y: (parse(&self.y_min), parse(&self.y_max)),
            z: (parse(&self.z_min), parse(&self.z_max)),
            ench_name: low(&self.ench_name),
            ench_op: self.ench_op,
            ench_level: self.ench_level,
        }
    }
}

pub struct Compiled {
    text: Option<Expr>,
    cat: TextCat,
    show_backpacks: bool,
    show_containers: bool,
    show_players: bool,
    player: Option<String>,
    item: Option<String>,
    ctype: Option<String>,
    dimension: Option<String>,
    nbt: Option<String>,
    dungeon: DungeonFilter,
    hide_empty: bool,
    min_count: Option<i64>,
    x: (Option<i64>, Option<i64>),
    y: (Option<i64>, Option<i64>),
    z: (Option<i64>, Option<i64>),
    ench_name: Option<String>,
    ench_op: EnchOp,
    ench_level: i32,
}

fn in_range(v: i64, bounds: (Option<i64>, Option<i64>)) -> bool {
    bounds.0.map_or(true, |lo| v >= lo) && bounds.1.map_or(true, |hi| v <= hi)
}

impl Compiled {
    pub fn coord_filter_active(&self) -> bool {
        [self.x, self.y, self.z]
            .iter()
            .any(|b| b.0.is_some() || b.1.is_some())
    }

    pub fn matches(&self, e: &Entry) -> bool {
        let kind_ok = match e.kind {
            EntryKind::Backpack => self.show_backpacks,
            EntryKind::Container => self.show_containers,
            EntryKind::Player => self.show_players,
        };
        if !kind_ok {
            return false;
        }

        if self.hide_empty && e.items.is_empty() {
            return false;
        }

        match self.dungeon {
            DungeonFilter::Only if !e.is_dungeon => return false,
            DungeonFilter::Hide if e.is_dungeon => return false,
            _ => {}
        }

        if self.coord_filter_active() {
            match e.coords {
                Some((x, y, z)) => {
                    if !in_range(x, self.x) || !in_range(y, self.y) || !in_range(z, self.z) {
                        return false;
                    }
                }
                None => return false,
            }
        }

        if let Some(p) = &self.player {
            if !e.owner.contains(p) && !e.uuid.contains(p) {
                return false;
            }
        }
        if let Some(t) = &self.ctype {
            if !e.header_icon.to_lowercase().contains(t) {
                return false;
            }
        }
        if let Some(d) = &self.dimension {
            if !e.dimension.to_lowercase().contains(d) {
                return false;
            }
        }
        if let Some(i) = &self.item {
            if !e.search_blob.contains(i) {
                return false;
            }
        }
        if let Some(n) = &self.nbt {
            if !e.nbt_blob.contains(n) && !e.search_blob.contains(n) {
                return false;
            }
        }
        if let Some(mc) = self.min_count {
            if e.max_stack < mc {
                return false;
            }
        }

        if self.ench_name.is_some() || self.ench_op != EnchOp::Any {
            let hit = e.all_enchants.iter().any(|(id, lvl)| {
                self.ench_name
                    .as_ref()
                    .map_or(true, |n| id.to_lowercase().contains(n))
                    && self.ench_op.test(*lvl, self.ench_level)
            });
            if !hit {
                return false;
            }
        }

        if let Some(expr) = &self.text {
            let icon = e.header_icon.to_lowercase();
            let ok = match self.cat {
                TextCat::Any => expr.eval(&|q| e.search_blob.contains(q) || e.nbt_blob.contains(q)),
                TextCat::Owner => {
                    matches!(e.kind, EntryKind::Backpack | EntryKind::Player)
                        && expr.eval(&|q| e.owner.contains(q) || e.uuid.contains(q))
                }
                TextCat::Item => expr.eval(&|q| e.search_blob.contains(q)),
                TextCat::Type => expr.eval(&|q| icon.contains(q)),
                TextCat::Upgrade => {
                    expr.eval(&|q| e.upgrades.iter().any(|u| u.id.to_lowercase().contains(q)))
                }
            };
            if !ok {
                return false;
            }
        }

        true
    }

    pub fn matches_meta(&self, m: &EntryMeta, text: &TextSource, it: &Interner) -> bool {
        let kind_ok = match m.kind {
            EntryKind::Backpack => self.show_backpacks,
            EntryKind::Container => self.show_containers,
            EntryKind::Player => self.show_players,
        };
        if !kind_ok {
            return false;
        }

        if self.hide_empty && !m.has_items() {
            return false;
        }

        match self.dungeon {
            DungeonFilter::Only if !m.is_dungeon() => return false,
            DungeonFilter::Hide if m.is_dungeon() => return false,
            _ => {}
        }

        if self.coord_filter_active() {
            match m.coords64() {
                Some((x, y, z)) => {
                    if !in_range(x, self.x) || !in_range(y, self.y) || !in_range(z, self.z) {
                        return false;
                    }
                }
                None => return false,
            }
        }

        if let Some(p) = &self.player {
            if !m.owner.contains(p.as_str()) && !m.uuid.contains(p.as_str()) {
                return false;
            }
        }
        if let Some(t) = &self.ctype {
            if !it.get(m.icon).to_lowercase().contains(t) {
                return false;
            }
        }
        if let Some(d) = &self.dimension {
            if !it.get(m.dim).to_lowercase().contains(d) {
                return false;
            }
        }
        if let Some(i) = &self.item {
            if !text_item(text, i) {
                return false;
            }
        }
        if let Some(n) = &self.nbt {
            if !text_nbt(text, n) {
                return false;
            }
        }
        if let Some(mc) = self.min_count {
            if (m.max_stack as i64) < mc {
                return false;
            }
        }

        if self.ench_name.is_some() || self.ench_op != EnchOp::Any {
            let hit = m.enchants.iter().any(|(id, lvl)| {
                self.ench_name
                    .as_ref()
                    .map_or(true, |n| it.get(*id).to_lowercase().contains(n))
                    && self.ench_op.test(*lvl as i32, self.ench_level)
            });
            if !hit {
                return false;
            }
        }

        if let Some(expr) = &self.text {
            let ok = match self.cat {
                TextCat::Any => expr.eval(&|q| text_any(text, q)),
                TextCat::Owner => {
                    matches!(m.kind, EntryKind::Backpack | EntryKind::Player)
                        && expr.eval(&|q| m.owner.contains(q) || m.uuid.contains(q))
                }
                TextCat::Item => expr.eval(&|q| text_item(text, q)),
                TextCat::Type => {
                    let icon = it.get(m.icon).to_lowercase();
                    expr.eval(&|q| icon.contains(q))
                }
                TextCat::Upgrade => expr.eval(&|q| text_upgrade(text, q)),
            };
            if !ok {
                return false;
            }
        }

        true
    }
}

fn text_item(text: &TextSource, q: &str) -> bool {
    match text {
        TextSource::Slice(b) => ci_contains(b, q),
        TextSource::Blob { search, .. } => search.contains(q),
    }
}

fn text_nbt(text: &TextSource, q: &str) -> bool {
    match text {
        TextSource::Slice(b) => ci_contains(b, q),
        TextSource::Blob { search, nbt, .. } => nbt.contains(q) || search.contains(q),
    }
}

fn text_any(text: &TextSource, q: &str) -> bool {
    match text {
        TextSource::Slice(b) => ci_contains(b, q),
        TextSource::Blob { search, nbt, .. } => search.contains(q) || nbt.contains(q),
    }
}

fn text_upgrade(text: &TextSource, q: &str) -> bool {
    match text {
        TextSource::Slice(_) => false,
        TextSource::Blob { upgrades, .. } => {
            upgrades.iter().any(|u| u.id.to_lowercase().contains(q))
        }
    }
}

#[cfg(test)]
mod query_tests {
    use super::parse_query;

    fn m(query: &str, hay: &str) -> bool {
        parse_query(query)
            .map(|e| e.eval(&|t| hay.contains(t)))
            .unwrap_or(true)
    }

    #[test]
    fn boolean_ops() {
        assert!(m("diamond and netherite", "a diamond and a netherite ingot"));
        assert!(!m("diamond and netherite", "just a diamond"));
        assert!(m("diamond or gold", "a gold bar"));
        assert!(!m("diamond or gold", "an emerald"));
        assert!(m("not diamond", "an emerald"));
        assert!(!m("not diamond", "a diamond"));
        assert!(m("gold and (diamond or emerald)", "gold and emerald"));
        assert!(!m("gold and (diamond or emerald)", "gold and iron"));
        assert!(m("\"diamond sword\"", "a diamond sword here"));
        assert!(!m("\"diamond sword\"", "a diamond pickaxe"));
        assert!(m("gold diamond", "gold and diamond"));
        assert!(!m("gold diamond", "only gold"));
        assert!(m("   ", "whatever"));
    }

    #[test]
    fn and_binds_tighter_than_or() {
        assert!(m("a or b and c", "just a"));
        assert!(!m("a or b and c", "only zb"));
        assert!(m("a or b and c", "zb zc"));
        assert!(!m("a or b and c", "zc only"));
    }

    #[test]
    fn grouping_and_not() {
        assert!(m("(a or b) and (c or d)", "a d"));
        assert!(!m("(a or b) and (c or d)", "a only"));
        assert!(m("not (a or b)", "just c"));
        assert!(!m("not (a or b)", "has a here"));
        assert!(m("gold and not silver", "gold bar"));
        assert!(!m("gold and not silver", "gold and silver"));
    }

    #[test]
    fn amp_pipe_aliases_and_case() {
        assert!(m("a && b", "a b"));
        assert!(!m("a && b", "a only"));
        assert!(m("a || b", "b only"));
        assert!(m("DIAMOND", "a diamond"));
        assert!(m("AND", "the word and"));
    }

    #[test]
    fn tolerates_unbalanced_parens() {
        assert!(m("(diamond or gold", "a gold bar"));
        assert!(m("diamond)", "a diamond"));
    }
}

#[cfg(test)]
mod filter_tests {
    use super::*;
    use crate::model::{Entry, EntryKind, Item};

    fn item(id: &str, count: i64) -> Item {
        Item {
            id: id.into(),
            count,
            ..Default::default()
        }
    }

    fn enchanted(id: &str, level: i32) -> Item {
        Item {
            id: "minecraft:diamond_sword".into(),
            count: 1,
            enchants: vec![(id.into(), level)],
            ..Default::default()
        }
    }

    fn make(kind: EntryKind, items: Vec<Item>) -> Entry {
        let mut e = Entry {
            kind,
            items,
            header_icon: "minecraft:barrel".into(),
            dimension: "minecraft:the_nether".into(),
            owner: "steve".into(),
            uuid: "abc-123".into(),
            ..Default::default()
        };
        e.finalize("");
        e
    }

    fn compiled(build: impl FnOnce(&mut Filters)) -> Compiled {
        let mut f = Filters::default();
        build(&mut f);
        f.compile()
    }

    #[test]
    fn kind_visibility() {
        let bp = make(EntryKind::Backpack, vec![item("minecraft:stone", 1)]);
        let c = compiled(|f| f.show_backpacks = false);
        assert!(!c.matches(&bp));
        assert!(compiled(|_| {}).matches(&bp));
    }

    #[test]
    fn hide_empty_and_dungeon() {
        let empty = make(EntryKind::Container, vec![]);
        assert!(!compiled(|_| {}).matches(&empty));
        assert!(compiled(|f| f.hide_empty = false).matches(&empty));

        let mut dungeon = make(EntryKind::Container, vec![item("minecraft:stone", 1)]);
        dungeon.is_dungeon = true;
        assert!(!compiled(|f| f.dungeon = DungeonFilter::Hide).matches(&dungeon));
        assert!(compiled(|f| f.dungeon = DungeonFilter::Only).matches(&dungeon));
    }

    #[test]
    fn coord_range_and_min_count() {
        let mut e = make(EntryKind::Container, vec![item("minecraft:diamond", 64)]);
        e.coords = Some((100, 64, -200));
        assert!(compiled(|f| f.x_min = "50".into()).matches(&e));
        assert!(!compiled(|f| f.x_max = "50".into()).matches(&e));
        assert!(compiled(|f| f.min_count = "64".into()).matches(&e));
        assert!(!compiled(|f| f.min_count = "65".into()).matches(&e));
    }

    #[test]
    fn enchant_and_text_categories() {
        let e = make(EntryKind::Container, vec![enchanted("minecraft:sharpness", 5)]);
        assert!(compiled(|f| {
            f.ench_op = EnchOp::Gte;
            f.ench_level = 5;
        })
        .matches(&e));
        assert!(!compiled(|f| {
            f.ench_op = EnchOp::Gt;
            f.ench_level = 5;
        })
        .matches(&e));
        assert!(compiled(|f| f.ench_name = "sharp".into()).matches(&e));

        assert!(compiled(|f| {
            f.text = "sword".into();
            f.cat = TextCat::Item;
        })
        .matches(&e));
        assert!(compiled(|f| {
            f.text = "barrel".into();
            f.cat = TextCat::Type;
        })
        .matches(&e));

        let bp = make(EntryKind::Backpack, vec![item("minecraft:stone", 1)]);
        assert!(compiled(|f| {
            f.text = "steve".into();
            f.cat = TextCat::Owner;
        })
        .matches(&bp));
        assert!(!compiled(|f| {
            f.text = "steve".into();
            f.cat = TextCat::Owner;
        })
        .matches(&e));
    }

    #[test]
    fn dimension_and_player_fields() {
        let e = make(EntryKind::Container, vec![item("minecraft:stone", 1)]);
        assert!(compiled(|f| f.dimension = "nether".into()).matches(&e));
        assert!(!compiled(|f| f.dimension = "end".into()).matches(&e));
        assert!(compiled(|f| f.player = "steve".into()).matches(&e));
        assert!(compiled(|f| f.player = "abc-123".into()).matches(&e));
        assert!(!compiled(|f| f.player = "alex".into()).matches(&e));
    }
}
