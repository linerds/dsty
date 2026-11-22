//! Правила повної перевірки: факти компіляції + модель коду.
//!
//! Typst не віддає рядків коду, тож кожен елемент потоку прив'язується до
//! моделі ([`crate::model`]): мітка, текст заголовка чи назви, початок абзацу,
//! порядок. Неприв'язане зауваження стає на найближчий прив'язаний елемент
//! перед ним, а зауваження до налаштувань документа стає на `set`-правило чи
//! рядок шаблону.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

use super::facts::{Entry, Facts, Item, Page, Run};
use crate::model::{Block, Doc, Heading, Label};
use crate::rules::{self, Found, structural};
use crate::text::{Loc, fmt_num, fmt_pt, mm, quote};

/// Літери додатків (7.15.1): українська абетка без Ґ, Є, З, І, Ї, Й, О, Ч, Ь
/// або латинська без I та O.
const UKR_LETTERS: &str = "АБВГДЕЖИКЛМНПРСТУФХЦШЩЮЯ";
const LAT_LETTERS: &str = "ABCDEFGHJKLMNPQRSTUVWXYZ";
const FORBIDDEN_LETTERS: &str = "ҐЄЗІЇЙОЧЬ";

/// Мм у пунктах.
const MM: f64 = 72.0 / 25.4;
/// Одинарний міжрядковий інтервал Times New Roman (як у текстових
/// редакторах): 1,15 кегля.
const SINGLE: f64 = 1.15;

static NUMBER_RUN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(\d+|[А-ЯҐЄІЇA-Z])((?:\.\d+)*)(\.?)\s*$").unwrap());
static CAPTION_NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*([\dА-ЯҐЄІЇA-Z]+(?:\.\d+)*)(\.?)\s*$").unwrap());
static EQ_NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\(([^()]*)\)|(\d+(?:\.\d+)*))\s*$").unwrap());
/// Позначка нумерованого переліку: «а)», «1)» (латинська «a)» дає окреме зауваження).
static ENUM_MARK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([а-яґєіїa]|\d+)\)$").unwrap());
static APPENDIX_WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(додаток)\s*([^\s.]+)?").unwrap());
static COUNTS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    [
        ("pages", r"(\d+)\s*(?:с\.|стор)"),
        ("figures", r"(\d+)\s*рис"),
        ("tables", r"(\d+)\s*табл"),
        ("appendices", r"(\d+)\s*дод"),
        ("sources", r"(\d+)\s*(?:джерел|посил)"),
    ]
    .into_iter()
    .map(|(k, r)| (k, Regex::new(r).unwrap()))
    .collect()
});

pub fn run(doc: &Doc, facts: &Facts, active: &dyn Fn(&str) -> bool) -> Vec<Found> {
    let mut cx = Checker::new(doc, facts, active);
    cx.pages();
    cx.fonts_missing();
    cx.headings();
    cx.appendix_letters();
    cx.figures();
    cx.tables();
    cx.equations();
    cx.footnotes();
    cx.paragraphs();
    cx.lists();
    cx.abstract_and_contents();
    cx.citations();
    cx.out
}

/// Ключ прив'язки: текст без розмітки й розділових знаків, малими літерами.
fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Зіставляє елементи потоку з елементами моделі: спершу за однаковими
/// ключами в тому самому порядку, потім у проміжках між зіставленими за
/// порядком, якщо в них порівну елементів. `None` у ключі означає, що ключа немає.
fn align(stream: &[Option<String>], model: &[Option<String>]) -> Vec<Option<usize>> {
    let mut out = vec![None; stream.len()];
    let mut used = vec![false; model.len()];
    let mut cursor = 0;
    for (i, key) in stream.iter().enumerate() {
        let Some(key) = key else { continue };
        if key.is_empty() {
            continue;
        }
        if let Some(j) = (cursor..model.len()).find(|&j| !used[j] && model[j].as_ref() == Some(key))
        {
            out[i] = Some(j);
            used[j] = true;
            cursor = j + 1;
        }
    }
    // Проміжки між зіставленими
    let mut i = 0;
    let mut model_from = 0;
    while i < stream.len() {
        if let Some(j) = out[i] {
            model_from = j + 1;
            i += 1;
            continue;
        }
        let gap_start = i;
        while i < stream.len() && out[i].is_none() {
            i += 1;
        }
        let model_to = out.get(i).copied().flatten().unwrap_or(model.len());
        let free: Vec<usize> = (model_from..model_to.max(model_from))
            .filter(|&j| !used[j])
            .collect();
        if free.len() == i - gap_start {
            for (k, j) in free.into_iter().enumerate() {
                out[gap_start + k] = Some(j);
                used[j] = true;
            }
        }
    }
    out
}

/// Де в коді шукати налаштування для зауваження на весь документ.
#[derive(Debug, Clone, Copy)]
enum Place<'s> {
    /// `set ціль(аргументи)`.
    Set(&'s str, &'s [&'s str]),
    /// `show елемент: set ціль(аргументи)`, інакше `set елемент(…)`.
    In(&'s str, &'s str, &'s [&'s str]),
}

struct Checker<'a> {
    doc: &'a Doc,
    e: &'a [Entry],
    facts: &'a Facts,
    active: &'a dyn Fn(&str) -> bool,
    out: Vec<Found>,
    /// Прив'язка записів потоку до коду.
    locs: Vec<Option<Loc>>,
    /// Для кожного запису: найближчий прив'язаний не пізніше за нього.
    nearest: Vec<Option<usize>>,
    /// Запис потоку заголовка -> заголовок моделі.
    heading_model: HashMap<usize, &'a Heading>,
    /// Запис -> рівень-1 заголовок додатка, у якому він стоїть (номер додатка з 0).
    appendix_of: Vec<Option<usize>>,
    /// Записи заголовків додатків першого рівня.
    appendices: Vec<usize>,
    /// Літера кожного додатка, як її надруковано.
    appendix_letters: Vec<Option<String>>,
    /// Номер розділу (перший рівень, не додаток) для кожного запису.
    chapter_of: Vec<Option<i64>>,
    pages: BTreeMap<usize, &'a Page>,
    /// Перший заголовок: усе до нього належить титульному аркушу.
    body_start: usize,
    /// Звичний абзацний відступ основного тексту, pt.
    indent: f64,
    /// Перше посилання на мітку: індекс запису.
    first_ref: HashMap<&'a str, usize>,
}

impl<'a> Checker<'a> {
    fn new(doc: &'a Doc, facts: &'a Facts, active: &'a dyn Fn(&str) -> bool) -> Self {
        let e = &facts.entries[..];
        let mut cx = Checker {
            doc,
            e,
            facts,
            active,
            out: Vec::new(),
            locs: vec![None; e.len()],
            nearest: Vec::new(),
            heading_model: HashMap::new(),
            appendix_of: vec![None; e.len()],
            appendices: Vec::new(),
            appendix_letters: Vec::new(),
            chapter_of: vec![None; e.len()],
            pages: BTreeMap::new(),
            body_start: e.len(),
            indent: 0.0,
            first_ref: HashMap::new(),
        };
        for (i, entry) in e.iter().enumerate() {
            match &entry.item {
                Item::Page(p) => {
                    cx.pages.entry(entry.pg).or_insert(p);
                }
                Item::Heading { .. } if cx.body_start == e.len() => cx.body_start = i,
                Item::Ref { target } => {
                    cx.first_ref.entry(target.as_str()).or_insert(i);
                }
                _ => {}
            }
        }
        cx.map_headings();
        cx.map_figures();
        cx.map_equations();
        cx.map_footnotes();
        cx.map_pars();
        cx.map_lists();
        let mut last = None;
        cx.nearest = cx
            .locs
            .iter()
            .enumerate()
            .map(|(j, loc)| {
                if loc.is_some() {
                    last = Some(j);
                }
                last
            })
            .collect();
        cx.regions();
        cx.indent = cx.dominant_indent();
        cx
    }

    // --- Прив'язка до коду ------------------------------------------------------

    fn map_headings(&mut self) {
        let doc = self.doc;
        let model: Vec<&Heading> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Heading(h) => Some(h),
                _ => None,
            })
            .collect();
        let stream: Vec<usize> = self.indices(|i| matches!(i, Item::Heading { .. }));
        let keys = |s: &str| Some(norm(s));
        let skeys: Vec<_> = stream
            .iter()
            .map(|&i| match &self.e[i].item {
                Item::Heading { body, .. } => keys(body),
                _ => None,
            })
            .collect();
        let mkeys: Vec<_> = model.iter().map(|h| keys(h.title.trimmed())).collect();
        for (k, m) in align(&skeys, &mkeys).into_iter().enumerate() {
            if let Some(m) = m {
                self.locs[stream[k]] = Some(model[m].loc.clone());
                self.heading_model.insert(stream[k], model[m]);
            } else if let Item::Heading { body, .. } = &self.e[stream[k]].item {
                self.locs[stream[k]] = self.search(body);
            }
        }
    }

    fn map_figures(&mut self) {
        let doc = self.doc;
        let model: Vec<_> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Figure(f) => Some(f),
                _ => None,
            })
            .collect();
        let stream = self.indices(
            |i| matches!(i, Item::Figure { kind, .. } if kind == "image" || kind == "table"),
        );
        let skeys: Vec<_> = stream
            .iter()
            .map(|&i| match &self.e[i].item {
                Item::Figure { label, caption, .. } => label
                    .as_ref()
                    .map(|l| format!("l:{l}"))
                    .or_else(|| caption.as_ref().map(|c| format!("c:{}", norm(c)))),
                _ => None,
            })
            .collect();
        let mkeys: Vec<_> = model
            .iter()
            .map(|f| match &f.label {
                Some(Label::Name(l)) => Some(format!("l:{l}")),
                Some(Label::Dynamic) => None,
                None => f
                    .caption
                    .as_ref()
                    .map(|c| format!("c:{}", norm(c.trimmed()))),
            })
            .collect();
        for (k, m) in align(&skeys, &mkeys).into_iter().enumerate() {
            if let Some(m) = m {
                let f = model[m];
                let loc = f
                    .caption
                    .as_ref()
                    .and_then(|c| c.whole())
                    .unwrap_or_else(|| f.loc.clone());
                self.locs[stream[k]] = Some(loc);
            } else if let Item::Figure {
                caption: Some(c), ..
            } = &self.e[stream[k]].item
            {
                // Власна обгортка (`img(…)`): назву шукаємо в коді
                self.locs[stream[k]] = self.search(c);
            }
        }
    }

    /// Текст у вихідних файлах дослівно (назва рисунка в аргументі власної
    /// функції). Лише якщо він трапляється один раз.
    fn search(&self, text: &str) -> Option<Loc> {
        let text = text.trim();
        if text.chars().count() < 4 {
            return None;
        }
        let mut found = None;
        for (file, f) in self.doc.files.iter().enumerate() {
            let src = f.source.text();
            let mut at = 0;
            while let Some(pos) = src[at..].find(text) {
                if found.is_some() {
                    return None;
                }
                let start = at + pos;
                found = Some(Loc {
                    file,
                    range: start..start + text.len(),
                });
                at = start + text.len();
            }
        }
        found
    }

    fn map_equations(&mut self) {
        let doc = self.doc;
        let model: Vec<(&Loc, Option<&Label>)> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Equation { loc, label, .. } => Some((loc, label.as_ref())),
                _ => None,
            })
            .collect();
        let stream = self.indices(|i| matches!(i, Item::Equation { .. }));
        let skeys: Vec<_> = stream
            .iter()
            .map(|&i| match &self.e[i].item {
                Item::Equation { label, .. } => label.clone(),
                _ => None,
            })
            .collect();
        let mkeys: Vec<_> = model
            .iter()
            .map(|(_, l)| match l {
                Some(Label::Name(n)) => Some(n.clone()),
                _ => None,
            })
            .collect();
        for (k, m) in align(&skeys, &mkeys).into_iter().enumerate() {
            if let Some(m) = m {
                self.locs[stream[k]] = Some(model[m].0.clone());
            }
        }
    }

    fn map_footnotes(&mut self) {
        let stream = self.indices(|i| matches!(i, Item::Footnote { again: false, .. }));
        if stream.len() == self.doc.footnotes.len() {
            for (k, &i) in stream.iter().enumerate() {
                self.locs[i] = Some(self.doc.footnotes[k].clone());
            }
        }
    }

    fn map_pars(&mut self) {
        let doc = self.doc;
        let model: Vec<&crate::model::Par> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Par(p) => Some(p),
                _ => None,
            })
            .collect();
        let stream = self.indices(
            |i| matches!(i, Item::Par(p) if p.kind.is_empty() || p.kind == "l" || p.kind == "b"),
        );
        // Ключ складається з перших 24 літер і цифр: абзаци різняться вже на початку
        let key = |s: &str| {
            let n: String = norm(s).chars().take(24).collect();
            (n.chars().count() >= 6).then_some(n)
        };
        let skeys: Vec<_> = stream
            .iter()
            .map(|&i| match &self.e[i].item {
                Item::Par(p) => key(&p.head),
                _ => None,
            })
            .collect();
        let mkeys: Vec<_> = model.iter().map(|p| key(p.text.as_str())).collect();
        for (k, m) in align(&skeys, &mkeys).into_iter().enumerate() {
            // Абзац без ключа за порядком не зіставляється: шаблон додає свої
            if let (Some(m), Some(_)) = (m, &skeys[k]) {
                self.locs[stream[k]] = model[m].text.whole();
            }
        }
    }

    fn map_lists(&mut self) {
        let doc = self.doc;
        let model: Vec<&crate::model::List> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::List(l) => Some(l),
                _ => None,
            })
            .collect();
        let stream = self.indices(|i| matches!(i, Item::List { .. } | Item::Enum { .. }));
        if stream.len() == model.len() {
            for (k, &i) in stream.iter().enumerate() {
                self.locs[i] = model[k].items.first().map(|it| it.loc.clone());
            }
        }
    }

    /// Додатки й розділи: де кожен запис.
    fn regions(&mut self) {
        let mut appendix: Option<usize> = None;
        let mut chapter: Option<i64> = None;
        for i in 0..self.e.len() {
            if let Item::Heading {
                level: 1,
                num,
                body,
                counter,
                ..
            } = &self.e[i].item
            {
                let model_appendix = self.heading_model.get(&i).map(|h| h.appendix);
                let printed = self
                    .heading_runs(i)
                    .iter()
                    .any(|(_, r)| r.t.to_lowercase().contains("додаток"));
                let is_appendix = model_appendix.unwrap_or(false)
                    || printed
                    || (appendix.is_some() && num.is_some() && structural(body).is_none());
                if is_appendix {
                    self.appendices.push(i);
                    appendix = Some(self.appendices.len() - 1);
                    chapter = None;
                } else if structural(body).is_some() {
                    appendix = None;
                    chapter = None;
                } else if num.is_some() {
                    appendix = None;
                    chapter = counter.first().copied();
                }
            }
            self.appendix_of[i] = appendix;
            self.chapter_of[i] = chapter;
        }
        self.appendix_letters = self
            .appendices
            .iter()
            .map(|&i| {
                let text = self.heading_text(i);
                APPENDIX_WORD
                    .captures(&text)
                    .and_then(|c| c.get(2))
                    .map(|m| m.as_str().to_owned())
            })
            .collect();
    }

    fn dominant_indent(&self) -> f64 {
        let mut counts: HashMap<i64, usize> = HashMap::new();
        for (i, entry) in self.e.iter().enumerate() {
            if let Item::Par(p) = &entry.item
                && i > self.body_start
                && p.kind.is_empty()
                && p.indent > 0.5
            {
                *counts.entry((p.indent * 10.0).round() as i64).or_default() += 1;
            }
        }
        counts
            .into_iter()
            .max_by_key(|&(v, n)| (n, -v))
            .map_or(0.0, |(v, _)| v as f64 / 10.0)
    }

    fn indices(&self, f: impl Fn(&Item) -> bool) -> Vec<usize> {
        (0..self.e.len()).filter(|&i| f(&self.e[i].item)).collect()
    }

    /// Місце запису в коді: власне або найближчого прив'язаного перед ним.
    fn loc(&self, i: usize) -> Loc {
        self.nearest
            .get(i.min(self.e.len().saturating_sub(1)))
            .copied()
            .flatten()
            .and_then(|j| self.locs[j].clone())
            .unwrap_or_else(|| self.doc.anchor())
    }

    fn add(&mut self, code: &str, loc: Loc, msg: String) {
        self.add_clause(code, loc, msg, None);
    }

    fn add_clause(&mut self, code: &str, loc: Loc, msg: String, clause: Option<&'static str>) {
        if !(self.active)(code) {
            return;
        }
        let Some(rule) = rules::find(code) else {
            return;
        };
        let mut found = Found::new(rule, loc, msg, Vec::new());
        found.clause = clause;
        self.out.push(found);
    }

    fn place(&self, place: Place) -> Loc {
        match place {
            Place::Set(target, args) => self.doc.setting(target, args),
            Place::In(selector, target, args) => self.doc.setting_in(selector, target, args),
        }
    }

    fn page(&self, pg: usize) -> Option<&'a Page> {
        self.pages
            .get(&pg)
            .copied()
            .or_else(|| self.pages.values().next().copied())
    }

    /// Запис на берегах сторінки (колонтитул, номер сторінки).
    fn in_margin(&self, i: usize) -> bool {
        let entry = &self.e[i];
        self.page(entry.pg).is_some_and(|p| {
            let (top, _, bottom, _) = p.margins();
            entry.y < top - 1.0 || entry.y > p.h - bottom + 1.0
        })
    }

    fn text_area(&self, pg: usize) -> (f64, f64) {
        self.page(pg).map_or((70.87, 566.93), Page::text_area)
    }

    /// Прогони тексту заголовка: записи `h`/`H` одразу після нього.
    fn heading_runs(&self, i: usize) -> Vec<(usize, &'a Run)> {
        let mut out = Vec::new();
        for j in i + 1..self.e.len() {
            match &self.e[j].item {
                Item::Run(r) if r.kind == "h" || r.kind == "H" => out.push((j, r)),
                Item::Ref { .. } | Item::Cite { .. } | Item::Footnote { .. } => {}
                Item::Run(r) if r.kind == "s" || r.kind == "n" => {}
                _ => break,
            }
        }
        out
    }

    fn heading_text(&self, i: usize) -> String {
        self.heading_runs(i)
            .iter()
            .map(|(_, r)| r.t.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Чи запис належить до вмісту сторінки (а не службовий запис, виноска чи номер).
    fn is_content(item: &Item) -> bool {
        match item {
            Item::Par(p) => p.kind != "n",
            Item::Run(r) => !matches!(r.kind.as_str(), "n" | "s" | "m" | "w"),
            Item::Heading { .. }
            | Item::Figure { .. }
            | Item::Equation { .. }
            | Item::Table { .. }
            | Item::List { .. }
            | Item::Enum { .. } => true,
            _ => false,
        }
    }

    // --- Сторінки -------------------------------------------------------------------

    fn pages(&mut self) {
        let pages: Vec<(usize, &Page)> = self.pages.iter().map(|(&k, &v)| (k, v)).collect();
        if pages.is_empty() {
            return;
        }
        // Номери сторінок (7.3.1, 7.3.3)
        let loc = self.doc.setting("page", &["numbering", "number-align"]);
        fn shown(p: &Page) -> Option<&str> {
            p.num.as_deref().map(str::trim).filter(|n| !n.is_empty())
        }
        // Титульним аркушем вважається перша сторінка без заголовків (у відповідях на
        // іспит, наприклад, його немає)
        let title_page = pages.first().is_some_and(|(pg, _)| {
            !self
                .e
                .iter()
                .any(|x| x.pg == *pg && matches!(x.item, Item::Heading { .. }))
        });
        if title_page
            && let Some((_, first)) = pages.first()
            && let Some(n) = shown(first)
        {
            self.add(
                "page-number",
                loc.clone(),
                format!("на титульному аркуші проставлено номер «{n}»"),
            );
        }
        let custom_header = pages.iter().any(|(_, p)| !p.header);
        let missing: Vec<usize> = pages
            .iter()
            .skip(1)
            .filter(|(_, p)| shown(p).is_none())
            .map(|(pg, _)| *pg)
            .collect();
        // Власний колонтитул може друкувати номер сам: тоді його не видно
        if !missing.is_empty() && !custom_header {
            self.add(
                "page-number",
                loc.clone(),
                format!("сторінки без номера: {}", list_pages(&missing)),
            );
        }
        if let Some((pg, n)) = pages.iter().skip(1).find_map(|(pg, p)| {
            shown(p)
                .filter(|n| !n.chars().all(|c| c.is_ascii_digit()))
                .map(|n| (pg, n))
        }) {
            self.add(
                "page-number",
                loc.clone(),
                format!("с. {pg}: номер «{n}», має бути арабськими цифрами без крапки"),
            );
        } else if let Some((pg, n)) = pages.iter().skip(1).find_map(|(pg, p)| {
            shown(p)
                .and_then(|n| n.parse::<usize>().ok())
                .filter(|n| n != pg)
                .map(|n| (pg, n))
        }) {
            self.add(
                "page-number",
                loc.clone(),
                format!(
                    "на с. {pg} номер {n}: сторінки нумерують наскрізно, враховуючи титульний аркуш"
                ),
            );
        }
        if !custom_header
            && let Some((_, p)) = pages.iter().skip(1).find(|(_, p)| p.numbered)
            && !(p.align.contains("right") && p.align.contains("top"))
        {
            self.add(
                "page-number",
                loc,
                format!(
                    "номер сторінки розміщено «{}», а ставлять праворуч у верхньому куті",
                    p.align
                ),
            );
        }
    }

    /// Шрифту немає в системі: typst попереджає й набирає запасним.
    fn fonts_missing(&mut self) {
        let mut seen = HashSet::new();
        for w in &self.facts.warnings {
            let Some(name) = w.message.strip_prefix("unknown font family: ") else {
                continue;
            };
            if !name.contains("times new roman") {
                continue;
            }
            let loc = self
                .warning_loc(w.file.as_deref(), w.line)
                .unwrap_or_else(|| self.doc.setting("text", &["font"]));
            if seen.insert((loc.file, loc.range.start)) {
                self.add(
                    "font-missing",
                    loc,
                    "шрифту Times New Roman немає в системі: typst набирає текст запасним".into(),
                );
            }
        }
    }

    /// Рядок попередження typst у файлі документа.
    fn warning_loc(&self, file: Option<&str>, line: Option<usize>) -> Option<Loc> {
        let (file, line) = (file?, line?);
        let idx = self
            .doc
            .files
            .iter()
            .position(|f| f.path.ends_with(file.trim_start_matches('/')))?;
        let source = &self.doc.files[idx].source;
        let start = source.lines().line_to_byte(line.checked_sub(1)?)?;
        let text = source.text();
        let end = text[start..].find('\n').map_or(text.len(), |n| start + n);
        let lead = text[start..end].len() - text[start..end].trim_start().len();
        Some(Loc {
            file: idx,
            range: start + lead..end,
        })
    }

    // --- Заголовки ------------------------------------------------------------------

    fn headings(&mut self) {
        let heads = self.indices(|i| matches!(i, Item::Heading { .. }));
        // Оформлення заголовків спершу збирається, а потім дає зауваження на кожному
        // заголовку чи одне на весь документ ([`Self::report_spread`])
        let mut upper = Vec::new();
        let mut bold = Vec::new();
        let mut sizes = Vec::new();
        let mut fonts = Vec::new();
        let mut indents = Vec::new();
        let (mut styled, mut indented) = (0, 0);
        for &i in &heads {
            let entry = &self.e[i];
            let Item::Heading {
                level,
                num,
                body,
                counter,
                ..
            } = &entry.item
            else {
                continue;
            };
            let level = *level;
            let runs = self.heading_runs(i);
            let model = self.heading_model.get(&i).copied();
            // Заголовок, який друкує шаблон (`bibliography`), у коді не набрано:
            // у повідомленні він стоїть так, як надруковано
            let title = model
                .filter(|h| !h.generated)
                .map_or(body.as_str(), |h| h.title.trimmed());
            let shown = quote(title);
            let loc = self.loc(i);
            let is_structural = level == 1 && structural(body).is_some();
            let is_appendix = level == 1 && self.appendices.contains(&i);
            let numbered = num.is_some();
            let number_run = runs
                .first()
                .filter(|(_, r)| is_heading_number(&r.t))
                .map(|(_, r)| r.t.trim().to_owned());
            let text_runs: Vec<&Run> = runs
                .iter()
                .filter(|(_, r)| !is_heading_number(&r.t) && r.t.chars().any(char::is_alphabetic))
                .map(|(_, r)| *r)
                .collect();

            // 7.1.18: заголовок, який шаблон друкує з номером
            if is_structural
                && numbered
                && number_run.is_some()
                && !model.is_some_and(|h| h.numbered)
            {
                self.add(
                    "heading-numbered",
                    loc.clone(),
                    format!(
                        "структурний елемент {shown} надруковано з номером «{}»",
                        number_run.clone().unwrap_or_default()
                    ),
                );
            }

            // 7.1.20: великими літерами, напівжирним
            if level == 1 && !is_appendix && (is_structural || numbered) && !text_runs.is_empty() {
                styled += 1;
                if text_runs.iter().any(|r| r.upper == Some(false)) {
                    upper.push((
                        i,
                        format!("заголовок {shown} надруковано не великими літерами"),
                    ));
                }
                if let Some(w) = text_runs
                    .iter()
                    .filter_map(|r| r.weight())
                    .find(|&w| w < 600)
                {
                    bold.push((
                        i,
                        format!("заголовок {shown} надруковано не напівжирним (накреслення {w})"),
                    ));
                }
            }

            // 6.2.2: заголовок додатка набирають малими літерами з першої великої
            // (над ним «ДОДАТОК А» великими)
            if is_appendix {
                let title: Vec<&Run> = text_runs
                    .iter()
                    .copied()
                    .filter(|r| {
                        !r.t.to_lowercase().contains("додаток")
                            && r.t.chars().filter(|c| c.is_alphabetic()).count() > 1
                    })
                    .collect();
                if !title.is_empty() && title.iter().all(|r| r.upper == Some(true)) {
                    self.add_clause(
                        "heading-style",
                        loc.clone(),
                        format!(
                            "заголовок додатка {shown} надруковано великими літерами, а друкують малими з першої великої"
                        ),
                        Some("6.2.2"),
                    );
                }
            }

            // 7.1.18: надрукована назва структурного елемента (заголовок, який
            // друкує typst чи шаблон: «Бібліографія»)
            if level <= 2
                && let Some(canonical) = rules::canonical_name(body)
            {
                self.add(
                    "structure-name",
                    loc.clone(),
                    format!(
                        "структурний елемент надруковано як {}, а називають «{canonical}»",
                        quote(body)
                    ),
                );
            }

            // Кегль і шрифт заголовка (7.1.5)
            if let Some(r) = runs
                .iter()
                .map(|(_, r)| *r)
                .find(|r| (r.size - 14.0).abs() > 0.05)
            {
                sizes.push((
                    i,
                    format!("заголовок {shown} кеглем {}, має бути 14", fmt_pt(r.size)),
                ));
            }
            if let Some(r) = runs
                .iter()
                .map(|(_, r)| *r)
                .find(|r| r.font != "times new roman")
            {
                fonts.push((
                    i,
                    format!(
                        "заголовок {shown} шрифтом «{}», має бути Times New Roman",
                        r.font
                    ),
                ));
            }

            // Розміщення (7.1.21): заголовки 2-го рівня й нижче починаються з абзацного
            // відступу. Мітка першого прогону стоїть там, де починається рядок,
            // лише коли рядок не вирівняно посередині, тож заголовки першого
            // рівня (їх можна посередині) за позицією не перевіряються.
            if level >= 2
                && let Some((j, _)) = runs.first()
                && self.indent > 0.0
            {
                let (left, _) = self.text_area(entry.pg);
                let start = self.e[*j].x;
                indented += 1;
                if (start - (left + self.indent)).abs() > 2.5 {
                    indents.push((
                        i,
                        format!(
                            "заголовок {shown} починається за {} від берега, має бути з абзацного відступу ({})",
                            mm(start - left),
                            mm(self.indent)
                        ),
                    ));
                }
            }

            // 7.1.23: переноси в багаторядковому заголовку
            if !runs.is_empty() && runs.iter().any(|(_, r)| r.hyph == Some(true)) {
                let (left, right) = self.text_area(entry.pg);
                let width: f64 = runs.iter().filter_map(|(_, r)| r.w).sum();
                // y першого прогону дає верх блоку, y решти дає рядки тексту
                let ys: Vec<f64> = runs.iter().skip(1).map(|(j, _)| self.e[*j].y).collect();
                let lines = ys.windows(2).filter(|w| (w[1] - w[0]).abs() > 3.0).count() + 1;
                if lines > 1 || width > right - left - self.indent {
                    self.add(
                        "heading-hyphen",
                        loc.clone(),
                        format!("у заголовку {shown} ввімкнено переноси слів"),
                    );
                }
            }

            // 7.4: формат номера
            if numbered
                && !is_structural
                && !is_appendix
                && let Some(printed) = &number_run
            {
                let expected = match self.appendix_of[i] {
                    Some(a) => {
                        let letter = self.appendix_letter(a);
                        let rest: Vec<String> = counter
                            .iter()
                            .skip(1)
                            .take(level.saturating_sub(1))
                            .map(i64::to_string)
                            .collect();
                        std::iter::once(letter)
                            .chain(rest)
                            .collect::<Vec<_>>()
                            .join(".")
                    }
                    None => counter
                        .iter()
                        .take(level)
                        .map(i64::to_string)
                        .collect::<Vec<_>>()
                        .join("."),
                };
                if printed.ends_with('.') {
                    self.add(
                        "heading-number-format",
                        loc.clone(),
                        format!("після номера «{printed}» заголовка {shown} крапку не ставлять"),
                    );
                } else if !expected.is_empty() && printed != &expected {
                    self.add(
                        "heading-number-format",
                        loc.clone(),
                        format!(
                            "заголовок {shown} має номер «{printed}», за стандартом — «{expected}»"
                        ),
                    );
                }
            }

            // З нової сторінки: структурні елементи й додатки
            if is_structural || is_appendix {
                let before = (0..i)
                    .rev()
                    .take_while(|&j| self.e[j].pg == entry.pg)
                    .find(|&j| Self::is_content(&self.e[j].item));
                if before.is_some() {
                    let clause = if is_appendix {
                        "6.2.2"
                    } else {
                        new_page_clause(body)
                    };
                    let name = if is_appendix {
                        format!("додаток {shown}")
                    } else {
                        shown.clone()
                    };
                    self.add_clause(
                        "new-page",
                        loc.clone(),
                        format!(
                            "{name} має починатися з нової сторінки, а стоїть на с. {} після тексту",
                            entry.pg
                        ),
                        Some(clause),
                    );
                }
            }

            // 7.1.25: заголовок в останньому рядку сторінки
            let last = runs.last().map_or(i, |(j, _)| *j);
            let last_pg = self.e[last].pg.max(entry.pg);
            if let Some(next) =
                (last + 1..self.e.len()).find(|&j| Self::is_content(&self.e[j].item))
                && self.e[next].pg > last_pg
                && !matches!(self.e[next].item, Item::Heading { level: 1, .. })
            {
                self.add(
                    "heading-orphan",
                    loc,
                    format!(
                        "заголовок {shown} стоїть унизу с. {last_pg}, а текст після нього — на с. {}",
                        self.e[next].pg
                    ),
                );
            }
        }

        let total = heads.len();
        self.report_spread(
            "heading-style",
            upper,
            (styled, "заголовків"),
            Place::In("heading", "text", &[]),
            "заголовки розділів і структурних елементів надруковано не великими літерами",
        );
        self.report_spread(
            "heading-style",
            bold,
            (styled, "заголовків"),
            Place::In("heading", "text", &["weight"]),
            "заголовки розділів і структурних елементів надруковано не напівжирним",
        );
        self.report_spread(
            "font-size",
            sizes,
            (total, "заголовків"),
            Place::In("heading", "text", &["size"]),
            "заголовки набрано не кеглем 14",
        );
        self.report_spread(
            "font-family",
            fonts,
            (total, "заголовків"),
            Place::In("heading", "text", &["font"]),
            "заголовки набрано не шрифтом Times New Roman",
        );
        self.report_spread(
            "heading-indent",
            indents,
            (indented, "заголовків"),
            Place::In("heading", "block", &[]),
            "заголовки підрозділів і пунктів надруковано не з абзацного відступу",
        );
    }

    /// Номери за порядком для надрукованих `printed` (запис, номер): наскрізно
    /// або в межах розділу (якщо так пронумеровано більшість), у додатках з
    /// літерою додатка. `None`: номер у межах розділу, якого не видно.
    fn expected_numbers(&self, printed: &[(usize, String)]) -> Vec<Option<String>> {
        let main: Vec<&(usize, String)> = printed
            .iter()
            .filter(|(i, _)| self.appendix_of[*i].is_none())
            .collect();
        let per_chapter = main.iter().filter(|(_, n)| n.contains('.')).count() * 2 > main.len();
        let mut count: HashMap<(Option<usize>, Option<i64>), i64> = HashMap::new();
        printed
            .iter()
            .map(|&(i, _)| {
                let (key, prefix) = match self.appendix_of[i] {
                    Some(a) => ((Some(a), None), Some(self.appendix_letter(a))),
                    None if per_chapter => {
                        let ch = self.chapter_of[i];
                        ((None, ch), ch.map(|c| c.to_string()))
                    }
                    None => ((None, None), None),
                };
                let n = count.entry(key).or_default();
                *n += 1;
                match prefix {
                    Some(p) => Some(format!("{p}.{n}")),
                    None if per_chapter => None,
                    None => Some(n.to_string()),
                }
            })
            .collect()
    }

    fn appendix_letter(&self, a: usize) -> String {
        self.appendix_letters
            .get(a)
            .cloned()
            .flatten()
            .unwrap_or_else(|| {
                UKR_LETTERS
                    .chars()
                    .nth(a)
                    .map(String::from)
                    .unwrap_or_default()
            })
    }

    /// «ДОДАТОК А» над заголовком і допустима літера за порядком (6.2.2, 7.15.1).
    fn appendix_letters(&mut self) {
        let mut latin = None;
        for (a, &i) in self.appendices.clone().iter().enumerate() {
            let loc = self.loc(i);
            let text = self.heading_text(i);
            let Some(c) = APPENDIX_WORD.captures(&text) else {
                if !self.heading_runs(i).is_empty() {
                    self.add(
                        "appendix-letter",
                        loc,
                        "над заголовком додатка немає слова «ДОДАТОК» з літерою".into(),
                    );
                }
                continue;
            };
            let word = c.get(1).unwrap().as_str();
            let printed_upper = word == word.to_uppercase()
                || self
                    .heading_runs(i)
                    .iter()
                    .any(|(_, r)| r.t.to_lowercase().contains("додаток") && r.upper == Some(true));
            if !printed_upper {
                self.add(
                    "appendix-letter",
                    loc.clone(),
                    format!("слово «{word}» над заголовком додатка друкують великими літерами: «ДОДАТОК»"),
                );
            }
            let Some(letter) = c.get(2).map(|m| m.as_str().to_uppercase()) else {
                self.add(
                    "appendix-letter",
                    loc,
                    "після слова «ДОДАТОК» немає літери додатка".into(),
                );
                continue;
            };
            let is_latin = letter.chars().all(|ch| ch.is_ascii_alphabetic());
            let use_latin = *latin.get_or_insert(is_latin);
            let expected: Option<char> = if use_latin {
                LAT_LETTERS.chars().nth(a)
            } else {
                UKR_LETTERS.chars().nth(a)
            };
            let one = letter.chars().count() == 1;
            if one && FORBIDDEN_LETTERS.contains(letter.as_str()) || letter == "I" || letter == "O"
            {
                self.add(
                    "appendix-letter",
                    loc,
                    format!("додаток позначено літерою «{letter}», яку не використовують"),
                );
            } else if expected.is_some_and(|e| letter != e.to_string())
                && !letter.chars().all(|ch| ch.is_ascii_digit())
            {
                self.add(
                    "appendix-letter",
                    loc,
                    format!(
                        "додаток позначено «{letter}», а за порядком має бути «{}»",
                        expected.unwrap()
                    ),
                );
            }
        }
    }

    // --- Рисунки й таблиці ---------------------------------------------------------------

    fn figures(&mut self) {
        let figs = self.indices(
            |i| matches!(i, Item::Figure { kind, .. } if kind == "image" || kind == "table"),
        );
        // Номери за видом: (запис, надрукований номер)
        let mut numbers: HashMap<bool, Vec<(usize, String)>> = HashMap::new();
        let mut separators = Vec::new();
        let mut captioned = 0;
        for (n, &i) in figs.iter().enumerate() {
            let entry = &self.e[i];
            let Item::Figure {
                kind,
                num,
                label,
                floating,
                caption,
                supplement,
            } = &entry.item
            else {
                continue;
            };
            let is_table = kind == "table";
            let (word, name) = if is_table {
                ("Таблиця", "таблиця")
            } else {
                ("Рисунок", "рисунок")
            };
            let named = match caption {
                Some(c) => format!("{name} {}", quote(c)),
                None => name.to_owned(),
            };
            let loc = self.loc(i);
            let end = figs.get(n + 1).copied().unwrap_or(self.e.len());
            let end = (i + 1..end)
                .find(|&j| matches!(self.e[j].item, Item::Heading { .. }))
                .unwrap_or(end);
            let caption_runs: Vec<(usize, &Run)> = (i + 1..end)
                .filter_map(|j| match &self.e[j].item {
                    Item::Run(r) if r.kind == "c" => Some((j, r)),
                    _ => None,
                })
                .collect();

            if num.is_some() {
                // 7.5.1, 7.6.1: «Рисунок», «Таблиця»
                match caption {
                    None => self.add(
                        "caption-supplement",
                        loc.clone(),
                        format!("{named} без підпису «{word} N»"),
                    ),
                    Some(_) => {
                        let first = caption_runs
                            .first()
                            .map_or(supplement.as_str(), |(_, r)| r.t.as_str())
                            .trim();
                        if !first.starts_with(word) {
                            self.add(
                                "caption-supplement",
                                loc.clone(),
                                format!(
                                    "підпис починається з «{}», а має зі слова «{word}»",
                                    first.split_whitespace().next().unwrap_or(first)
                                ),
                            );
                        }
                    }
                }
                // Надрукований номер: перший прогін-номер після слова-підпису
                // (хоч би яке воно було)
                if let Some(k) = caption_runs.iter().skip(1).take(3).position(|(_, r)| {
                    CAPTION_NUMBER.is_match(&r.t) && r.t.chars().any(|c| c.is_alphanumeric())
                }) {
                    let k = k + 1;
                    let number = caption_runs[k].1.t.trim().to_owned();
                    // Роздільник між номером і назвою (7.5.7, 7.6.5): тире
                    if caption.is_some()
                        && let Some((_, sep)) = caption_runs.get(k + 1)
                    {
                        let sep = sep.t.trim();
                        if !sep.is_empty()
                            && sep.chars().all(|c| !c.is_alphanumeric())
                            && !matches!(sep, "–" | "—")
                        {
                            let what = match sep {
                                ":" => "двокрапка",
                                "." => "крапка",
                                "-" | "‐" | "−" => "дефіс",
                                _ => "інший знак",
                            };
                            separators.push((
                                i,
                                format!(
                                    "у підписі «{word} {number}{sep} …» між номером і назвою {what} «{sep}», а ставлять тире: «{word} {number} – …»"
                                ),
                            ));
                        }
                    }
                    numbers.entry(is_table).or_default().push((i, number));
                }
                if caption.is_some() {
                    captioned += 1;
                }
            }

            // 7.5.2, 7.6.3: не далі за наступну сторінку від першого посилання
            if let Some(label) = label
                && let Some(&r) = self.first_ref.get(label.as_str())
                && r < i
                && !floating
                && self.appendix_of[i].is_none()
                && entry.pg > self.e[r].pg + 1
            {
                self.add(
                    "figure-far-from-ref",
                    loc.clone(),
                    format!(
                        "{named} на с. {}, а перше посилання на с. {}: подають не далі ніж на наступній сторінці",
                        entry.pg, self.e[r].pg
                    ),
                );
            }

            // 7.5.9, 7.6.8: розміщення назви
            if let Some((j, first)) = caption_runs.first() {
                let (left, right) = self.text_area(self.e[*j].pg);
                let x = self.e[*j].x;
                if is_table {
                    let table_before =
                        (i + 1..*j).any(|k| matches!(self.e[k].item, Item::Table { .. }));
                    if table_before {
                        self.add(
                            "caption-position",
                            loc.clone(),
                            format!(
                                "назва таблиці {} під таблицею, а розміщують над нею",
                                quote(caption.as_deref().unwrap_or(""))
                            ),
                        );
                    } else if self.indent > 0.0 && (x - (left + self.indent)).abs() > 2.5 {
                        self.add(
                            "caption-position",
                            loc.clone(),
                            format!(
                                "назва таблиці починається за {} від берега, а друкують з абзацного відступу ({})",
                                mm(x - left),
                                mm(self.indent)
                            ),
                        );
                    }
                } else if let Some(w) = (i + 1..end).find_map(|k| match self.e[k].item {
                    Item::Caption { w } => Some(w),
                    _ => None,
                }) && w < (right - left) - 5.0
                    // Назва в клітинці сітки (рисунки поруч) середини сторінки не перетинає
                    && x < (left + right) / 2.0
                    && x + w > (left + right) / 2.0
                    && ((x + w / 2.0) - (left + right) / 2.0).abs() > 3.0
                    && first.t.trim().starts_with(word)
                {
                    self.add(
                        "caption-position",
                        loc.clone(),
                        format!(
                            "назва {} не посередині рядка",
                            quote(caption.as_deref().unwrap_or(""))
                        ),
                    );
                }
            }
        }

        self.report_spread(
            "caption-separator",
            separators,
            (captioned, "підписів"),
            Place::Set("figure.caption", &["separator"]),
            "у підписах рисунків і таблиць номер і назву розділено не тире",
        );

        // 7.5.6–7.5.8, 7.6.4–7.6.6, 7.15.3: нумерація
        for (is_table, list) in numbers {
            let name = if is_table {
                "таблиці"
            } else {
                "рисунка"
            };
            let expected = self.expected_numbers(&list);
            for ((i, printed), expected) in list.iter().zip(expected) {
                let Some(expected) = expected else { continue };
                let loc = self.loc(*i);
                if printed.ends_with('.') && printed.len() > 1 {
                    self.add(
                        "object-numbering",
                        loc,
                        format!("після номера {name} «{printed}» крапку не ставлять"),
                    );
                } else if *printed != expected {
                    self.add(
                        "object-numbering",
                        loc,
                        format!("номер {name} «{printed}», за порядком має бути «{expected}»"),
                    );
                }
            }
        }
    }

    /// Перенесені таблиці: головка й напис «Продовження таблиці» (7.6.9).
    fn tables(&mut self) {
        let mut stack: Vec<usize> = Vec::new();
        let mut spans: Vec<(usize, usize)> = Vec::new();
        for (i, entry) in self.e.iter().enumerate() {
            match entry.item {
                Item::Table { .. } => stack.push(i),
                Item::TableEnd => {
                    if let Some(start) = stack.pop() {
                        spans.push((start, i));
                    }
                }
                _ => {}
            }
        }
        spans.sort();
        for (start, end) in spans {
            let (from, to) = (self.e[start].pg, self.e[end].pg);
            if to <= from {
                continue;
            }
            let Item::Table { header, repeat } = self.e[start].item else {
                continue;
            };
            // Таблиця у фігурі: зауваження стає на її назву
            let owner = (0..start)
                .rev()
                .take_while(|&j| !matches!(self.e[j].item, Item::Par(_) | Item::Heading { .. }))
                .find(|&j| matches!(self.e[j].item, Item::Figure { .. }))
                .unwrap_or(start);
            let loc = self.loc(owner);
            let name = match &self.e[owner].item {
                Item::Figure {
                    caption: Some(c), ..
                } => format!("таблиця {}", quote(c)),
                _ => "таблиця".into(),
            };
            if !header {
                self.add(
                    "table-no-header",
                    loc.clone(),
                    format!(
                        "{name} переходить на с. {to} без головки (table.header), тож вона не повторюється"
                    ),
                );
            } else if !repeat {
                self.add(
                    "table-no-header",
                    loc.clone(),
                    format!("{name} переходить на с. {to}, а головку не повторено (repeat: false)"),
                );
            }
            let missing: Vec<usize> = (from + 1..=to)
                .filter(|&pg| {
                    !self.e.iter().any(|x| {
                        x.pg == pg
                            && matches!(&x.item, Item::Run(r) if {
                                let t = r.t.trim_start();
                                t.starts_with("Продовження") || t.starts_with("Кінець")
                            })
                    })
                })
                .collect();
            if !missing.is_empty() {
                self.add(
                    "table-continuation",
                    loc,
                    format!(
                        "{name} переходить на с. {}, а над перенесеною частиною немає «Продовження таблиці N»",
                        list_pages(&missing)
                    ),
                );
            }
        }
    }

    // --- Формули ------------------------------------------------------------------------

    fn equations(&mut self) {
        let eqs = self.indices(|i| matches!(i, Item::Equation { .. }));
        let mut printed: Vec<(usize, String)> = Vec::new();
        let mut numbered = 0;
        // Нумеровані формули, номера яких не прочитано (номер з кількох прогонів)
        let mut unread = 0;
        // Номер без дужок видно з шаблону нумерації (7.10.4)
        if let Some(pattern) = self.e.iter().find_map(|x| match &x.item {
            Item::Equation { num: Some(p), .. } if p != "fn" && !p.contains('(') => Some(p),
            _ => None,
        }) {
            let loc = self.doc.setting("math.equation", &["numbering"]);
            self.add(
                "formula-numbering",
                loc,
                format!("формули нумеровано за зразком «{pattern}» без круглих дужок, має бути «({pattern})»"),
            );
        }
        let mut unreferenced = Vec::new();
        for (n, &i) in eqs.iter().enumerate() {
            let Item::Equation { num, label } = &self.e[i].item else {
                continue;
            };
            let loc = self.loc(i);
            let referenced = label
                .as_ref()
                .is_some_and(|l| self.first_ref.contains_key(l.as_str()));
            if num.is_some() {
                numbered += 1;
                if !referenced {
                    unreferenced.push(i);
                }
            }
            // Вирівнювання: стиль іде одразу за записом формули
            if let Some(Item::EqStyle { align }) = self.e.get(i + 1).map(|x| &x.item)
                && (align.contains("left")
                    || align.contains("right")
                    || align.contains("start")
                    || align.contains("end"))
                && !align.contains("center")
            {
                self.add(
                    "formula-center",
                    loc.clone(),
                    format!("формулу вирівняно «{align}», а подають посередині рядка"),
                );
            }
            // Прогони формули: числа тіла й номер
            let next = eqs.get(n + 1).copied().unwrap_or(self.e.len());
            let runs: Vec<(usize, &Run)> = (i + 1..next)
                .map_while(|j| match &self.e[j].item {
                    Item::Run(r) if r.kind == "e" => Some(Some((j, r))),
                    Item::Run(_) | Item::Ref { .. } | Item::Cite { .. } | Item::EqStyle { .. } => {
                        Some(None)
                    }
                    _ => None,
                })
                .flatten()
                .collect();
            // Номер складається із суцільного ланцюжка прогонів у кінці (кожен починається
            // там, де закінчився попередній)
            let mut tail: Vec<(usize, &Run)> = Vec::new();
            for &(j, r) in runs.iter().rev() {
                let joined = tail.last().is_none_or(|&(k, _)| {
                    let end = self.e[j].x + r.w.unwrap_or(0.0);
                    (end - self.e[k].x).abs() < 1.5 && (self.e[j].y - self.e[k].y).abs() < 1.0
                });
                if !joined
                    || !r
                        .t
                        .chars()
                        .all(|c| c.is_alphanumeric() || "().,".contains(c))
                {
                    break;
                }
                tail.push((j, r));
            }
            tail.reverse();
            if num.is_some() && !tail.is_empty() {
                let text: String = tail.iter().map(|(_, r)| r.t.as_str()).collect();
                match EQ_NUMBER.captures(&text) {
                    Some(c) if c.get(2).is_some() => {
                        printed.push((i, c.get(2).unwrap().as_str().trim().to_owned()));
                    }
                    Some(c) => {
                        let bare = c.get(3).unwrap().as_str();
                        self.add(
                            "formula-numbering",
                            loc.clone(),
                            format!(
                                "номер формули «{bare}» без круглих дужок, має бути «({bare})»"
                            ),
                        );
                    }
                    None => {}
                }
                // Номер праворуч у крайньому положенні
                let (j, r) = tail[tail.len() - 1];
                let (_, right) = self.text_area(self.e[j].pg);
                let end = self.e[j].x + r.w.unwrap_or(0.0);
                if right - end > 3.0 {
                    self.add(
                        "formula-numbering",
                        loc.clone(),
                        format!(
                            "номер формули закінчується за {} від правого берега, а друкують у крайньому правому положенні",
                            mm(right - end)
                        ),
                    );
                }
            }
            if num.is_some() && !printed.last().is_some_and(|(j, _)| *j == i) {
                unread += 1;
            }
            // 7.10.6: «де» безпосередньо під формулою, без абзацного відступу
            if let Some(j) = (i + 1..self.e.len()).find(|&j| {
                Self::is_content(&self.e[j].item)
                    && !matches!(&self.e[j].item, Item::Run(r) if r.kind == "e")
            }) && let Item::Par(p) = &self.e[j].item
                && starts_with_where(&p.head)
            {
                let loc = self.loc(j);
                // Відступ, як його надруковано: положення слова «де» від
                // лівого берега. Без запису відступ береться з налаштувань абзацу
                let printed = (j + 1..self.e.len())
                    .take_while(|&k| !matches!(self.e[k].item, Item::Par(_)))
                    .find(|&k| matches!(&self.e[k].item, Item::Run(r) if r.kind == "w"))
                    .map(|k| self.e[k].x - self.text_area(self.e[k].pg).0);
                let indent = printed.unwrap_or(if p.all { p.indent } else { 0.0 });
                if p.lead_break {
                    // `\` перед «де» дає порожній рядок, а «де» стоїть на другому
                    // рядку без відступу
                    self.add(
                        "formula-where-indent",
                        loc,
                        "`\\` перед «де» лишає порожній рядок між формулою й поясненням, а пояснення подають безпосередньо під формулою: приберіть `\\`, а відступ зніміть так: `#par(first-line-indent: 0pt)[де …]`".into(),
                    );
                } else if indent > 1.0 {
                    self.add(
                        "formula-where-indent",
                        loc,
                        format!(
                            "рядок «де …» під формулою надруковано з абзацним відступом {}, а пояснення познак починають від лівого берега: оберніть його в `#par(first-line-indent: 0pt)[де …]`",
                            mm(indent)
                        ),
                    );
                } else if indent < -1.0 {
                    self.add(
                        "formula-where-indent",
                        loc,
                        format!(
                            "рядок «де …» під формулою виступає за лівий берег на {}: пояснення познак починають від лівого берега, оберніть його в `#par(first-line-indent: 0pt)[де …]`",
                            mm(-indent)
                        ),
                    );
                }
            }
        }

        // 7.10.2: якщо без посилань більшість, це налаштування нумерації
        if unreferenced.len() > 5 && unreferenced.len() * 2 > numbered {
            let loc = self.doc.setting("math.equation", &["numbering"]);
            self.add(
                "formula-numbering",
                loc,
                format!(
                    "пронумеровано {numbered} формул, з них {} без посилань у тексті: нумерують лише формули, на які посилаються",
                    unreferenced.len()
                ),
            );
        } else {
            for i in unreferenced {
                let loc = self.loc(i);
                self.add(
                    "formula-numbering",
                    loc,
                    "формулу без посилання в тексті не нумерують".into(),
                );
            }
        }

        // 7.10.3–7.10.5: наскрізно або в межах розділу, у додатках з
        // літерою. Без усіх номерів порядок не перевірити
        if unread > 0 {
            return;
        }
        let expected = self.expected_numbers(&printed);
        for ((i, number), expected) in printed.iter().zip(expected) {
            if let Some(expected) = expected
                && *number != expected
            {
                let loc = self.loc(*i);
                self.add(
                    "formula-numbering",
                    loc,
                    format!("номер формули «({number})», за порядком має бути «({expected})»"),
                );
            }
        }
    }

    // --- Виноски --------------------------------------------------------------------------

    fn footnotes(&mut self) {
        let notes = self.indices(|i| matches!(i, Item::Footnote { again: false, .. }));
        if notes.is_empty() {
            return;
        }
        let mut by_page: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for &i in &notes {
            by_page.entry(self.e[i].pg).or_default().push(i);
        }
        for (pg, list) in &by_page {
            if list.len() > 4 {
                let loc = self.loc(list[0]);
                self.add(
                    "footnote-count",
                    loc,
                    format!(
                        "на с. {pg} виносок: {}, дозволено не більше ніж чотири",
                        list.len()
                    ),
                );
            }
        }
        // Позначення «1)» і нумерація в межах сторінки (7.9.2)
        if let Some(Item::Footnote { num: Some(n), .. }) = notes.first().map(|&i| &self.e[i].item)
            && n != "fn"
            && n != "1)"
            && !n.chars().all(|c| c == '*')
        {
            let loc = self.doc.setting("footnote", &["numbering"]);
            self.add(
                "footnote-numbering",
                loc,
                format!("виноски позначено за зразком «{n}», а позначають «1)»"),
            );
        }
        if let Some((pg, first)) = by_page.iter().find_map(|(pg, list)| {
            let i = list[0];
            match &self.e[i].item {
                Item::Footnote { counter, .. } if counter.first().is_some_and(|&c| c != 1) => {
                    Some((*pg, i))
                }
                _ => None,
            }
        }) {
            let loc = self.loc(first);
            self.add(
                "footnote-numbering",
                loc,
                format!("на с. {pg} нумерацію виносок не почато з 1: виноски нумерують у межах кожної сторінки"),
            );
        }
        // Лінія над виносками (7.9.5)
        let pg = self.e[notes[0]].pg;
        if let Some(page) = self.page(pg)
            && let Some((abs, ratio)) = page.sep
        {
            let (left, right) = page.text_area();
            let len = abs + ratio * (right - left);
            if !(30.0 * MM - 0.5..=40.0 * MM + 0.5).contains(&len) {
                let loc = self.doc.setting("footnote.entry", &["separator"]);
                self.add(
                    "footnote-separator",
                    loc,
                    format!(
                        "лінія над виносками завдовжки {}, має бути 30–40 мм",
                        mm(len)
                    ),
                );
            }
        }
        // Кегль і інтервал тексту виносок (7.9.6), по разу на сторінку
        let mut size_pages = HashSet::new();
        let mut spacing_pages = HashSet::new();
        for (i, entry) in self.e.iter().enumerate() {
            let page_note = by_page.get(&entry.pg).map(|l| l[0]);
            match &entry.item {
                Item::Par(p) if p.kind == "n" => {
                    if (p.size - 12.0).abs() > 0.05 && size_pages.insert(entry.pg) {
                        let loc = self.loc(page_note.unwrap_or(i));
                        self.add(
                            "font-size",
                            loc,
                            format!(
                                "текст виноски на с. {} кеглем {}, має бути 12",
                                entry.pg,
                                fmt_pt(p.size)
                            ),
                        );
                    }
                    let k = p.pitch / (SINGLE * p.size);
                    if !(0.85..=1.25).contains(&k) && spacing_pages.insert(entry.pg) {
                        let loc = self.loc(page_note.unwrap_or(i));
                        self.add(
                            "line-spacing",
                            loc,
                            format!(
                                "текст виноски на с. {} через {} інтервалу, має бути через один",
                                entry.pg,
                                fmt_num(k)
                            ),
                        );
                    }
                }
                _ => {}
            }
        }
    }

    // --- Абзаци й основний текст ------------------------------------------------------

    fn paragraphs(&mut self) {
        // Абзаци основного тексту після першого заголовка
        let pars: Vec<usize> = (self.body_start..self.e.len())
            .filter(|&i| matches!(&self.e[i].item, Item::Par(p) if p.kind.is_empty()))
            .collect();
        if pars.is_empty() {
            return;
        }
        let total = pars.len();

        // Абзацний відступ, кегль, інтервал і шрифт абзаців основного тексту
        // перевіряє статика: тут лише те, чого з коду не видно

        // Примітки (7.8.3)
        for &i in &pars {
            let Item::Par(p) = &self.e[i].item else {
                continue;
            };
            let k = p.pitch / (SINGLE * p.size);
            if p.head.trim_start().starts_with("Примітк")
                && ((p.size - 12.0).abs() > 0.05 || !(0.85..=1.25).contains(&k))
            {
                let loc = self.loc(i);
                self.add(
                    "note-size",
                    loc,
                    format!(
                        "примітку набрано кеглем {} через {} інтервалу, має бути кеглем 12 через один",
                        fmt_pt(p.size),
                        fmt_num(k)
                    ),
                );
            }
        }
        // Прогони тексту з відхиленнями: шрифт, кегль, накреслення
        let mut family = Vec::new();
        let mut sizes = Vec::new();
        let mut styles = Vec::new();
        let mut current_par: Option<usize> = None;
        let mut note_par = false;
        for i in self.body_start..self.e.len() {
            match &self.e[i].item {
                Item::Par(p) => {
                    current_par = Some(i);
                    note_par = p.head.trim_start().starts_with("Примітк");
                    // Стиль абзацу: прогони основного тексту окремо не записуються
                    if !matches!(p.kind.as_str(), "" | "l" | "b") || p.font.is_empty() {
                        continue;
                    }
                    let shown = quote(&p.head);
                    if p.kind != "b" && (p.style != "normal" || !p.black) {
                        let what = if p.style != "normal" {
                            "курсивом"
                        } else {
                            "не чорним кольором"
                        };
                        styles.push((i, format!("абзац {shown} надруковано {what}")));
                    }
                }
                Item::Heading { .. } | Item::Figure { .. } | Item::Equation { .. } => {
                    current_par = None
                }
                Item::Run(r) if r.kind.is_empty() || r.kind == "l" || r.kind == "b" => {
                    // Колонтитул (номер сторінки) не належить до основного тексту
                    if self.in_margin(i) {
                        continue;
                    }
                    // Перший прогін абзацу йде в потоці перед записом абзацу
                    let at = match self.e.get(i + 1) {
                        Some(next)
                            if matches!(next.item, Item::Par(_))
                                && next.pg == self.e[i].pg
                                && (next.y - self.e[i].y).abs() < 0.5 =>
                        {
                            if let Item::Par(p) = &next.item {
                                note_par = p.head.trim_start().starts_with("Примітк");
                            }
                            i + 1
                        }
                        _ => current_par.unwrap_or(i),
                    };
                    // Шрифт і кегль перевіряються лише як відхилення від свого абзацу: стиль
                    // усього абзацу вже записано з абзацу, а прогону без абзацу
                    // (позначка переліку) порівняти нема з чим
                    let par = match &self.e[at].item {
                        Item::Par(p) if !p.font.is_empty() => Some(p),
                        _ => None,
                    };
                    let same_font = par.is_none_or(|p| p.font == r.font);
                    let same_size = par.is_none_or(|p| (p.size - r.size).abs() < 0.05);
                    if r.font != "times new roman" && !same_font {
                        family.push((
                            at,
                            format!(
                                "текст {} шрифтом «{}», має бути Times New Roman",
                                quote(&r.t),
                                r.font
                            ),
                        ));
                    }
                    if (r.size - 14.0).abs() > 0.05 && !note_par && !same_size {
                        sizes.push((
                            at,
                            format!(
                                "текст {} кеглем {}, має бути 14",
                                quote(&r.t),
                                fmt_pt(r.size)
                            ),
                        ));
                    }
                    if r.kind != "b" && (r.style != "normal" || !r.black) {
                        let what = if r.style != "normal" {
                            "курсивом"
                        } else {
                            "не чорним кольором"
                        };
                        styles.push((at, format!("текст {} надруковано {what}", quote(&r.t))));
                    }
                }
                _ => {}
            }
        }
        self.report_spread(
            "font-family",
            family,
            (total, "абзаців"),
            Place::Set("text", &["font"]),
            "основний текст набрано не шрифтом Times New Roman",
        );
        self.report_spread(
            "font-size",
            sizes,
            (total, "абзаців"),
            Place::Set("text", &["size"]),
            "основний текст набрано не кеглем 14",
        );
        self.report_spread(
            "font-style",
            styles,
            (total, "абзаців"),
            Place::Set("text", &["style", "fill"]),
            "основний текст набрано не прямим чорним шрифтом",
        );
    }

    /// Зауваження по елементах (абзацах, заголовках, переліках) або, якщо
    /// порушення в більшості з них, одне на налаштуванні документа: причина
    /// тоді одна, і зауваження на кожному елементі лише заважають. В одному
    /// елементі не більше одного зауваження.
    fn report_spread(
        &mut self,
        code: &str,
        items: Vec<(usize, String)>,
        (total, noun): (usize, &str),
        setting: Place,
        global: &str,
    ) {
        let mut seen = HashSet::new();
        let items: Vec<(usize, String)> =
            items.into_iter().filter(|(i, _)| seen.insert(*i)).collect();
        if items.is_empty() {
            return;
        }
        if items.len() > 5 && items.len() * 2 > total {
            let loc = self.place(setting);
            let count = if items.len() <= total {
                format!("{} {noun} з {total}", items.len())
            } else {
                format!("{} місць", items.len())
            };
            self.add(code, loc, format!("{global} ({count})"));
            return;
        }
        for (i, msg) in items {
            let loc = self.loc(i);
            self.add(code, loc, msg);
        }
    }

    // --- Переліки -------------------------------------------------------------------------

    fn lists(&mut self) {
        let lists = self.indices(|i| matches!(i, Item::List { .. } | Item::Enum { .. }));
        let mut in_bib = vec![false; self.e.len()];
        let mut bib = false;
        for (i, entry) in self.e.iter().enumerate() {
            if let Item::Heading { level: 1, body, .. } = &entry.item {
                bib = structural(body).is_some_and(|(n, _)| n == "перелік джерел посилання");
            }
            in_bib[i] = bib;
        }
        let dash = ["–", "—", "-", "−", "‒"];
        let mut bullets = Vec::new();
        let mut enums = Vec::new();
        let (mut bullet_total, mut enum_total) = (0, 0);
        for &i in &lists {
            if in_bib[i] {
                continue;
            }
            let loc = self.loc(i);
            let rendered = (i + 1..self.e.len())
                .take_while(|&j| {
                    !matches!(
                        self.e[j].item,
                        Item::List { .. } | Item::Enum { .. } | Item::Heading { .. }
                    )
                })
                .find_map(|j| match &self.e[j].item {
                    Item::Run(r) if r.kind == "l" && r.t.trim().chars().count() <= 5 => {
                        Some(r.t.trim().to_owned())
                    }
                    _ => None,
                });
            let entry = &self.e[i];
            let (indent, em) = match &entry.item {
                Item::List {
                    marker,
                    indent,
                    indent_em,
                } => {
                    let shown = if marker == "fn" {
                        rendered.clone().unwrap_or_default()
                    } else {
                        marker.trim().to_owned()
                    };
                    bullet_total += 1;
                    if !shown.is_empty() && !dash.contains(&shown.as_str()) {
                        bullets.push((
                            i,
                            format!("позиції переліку позначено «{shown}», а позначають знаком «тире» (–)"),
                        ));
                    }
                    (*indent, *indent_em)
                }
                Item::Enum {
                    num,
                    indent,
                    indent_em,
                } => {
                    let shown = match num.as_deref() {
                        Some("fn") | None => rendered.clone(),
                        Some(pattern) => Some(pattern.to_owned()),
                    };
                    enum_total += 1;
                    if let Some(shown) = shown {
                        let latin_letter = shown.starts_with(|c: char| c.is_ascii_lowercase());
                        if !ENUM_MARK.is_match(&shown) || latin_letter && shown != "a)" {
                            enums.push((
                                i,
                                format!("позиції нумерованого переліку позначено «{shown}», а позначають «а)» або «1)»"),
                            ));
                        } else if shown == "a)" {
                            enums.push((
                                i,
                                "позиції переліку позначено латинськими літерами «a)», а позначають літерами української абетки «а)»".into(),
                            ));
                        }
                    }
                    (*indent, *indent_em)
                }
                _ => continue,
            };
            // 7.7.4: з абзацного відступу
            let (left, _) = self.text_area(entry.pg);
            let marker_x = entry.x + indent + em * 14.0;
            if self.indent > 0.0 && (marker_x - (left + self.indent)).abs() > 5.0 {
                self.add(
                    "list-indent",
                    loc,
                    format!(
                        "позиції переліку починаються за {} від берега, а абзацний відступ — {}",
                        mm(marker_x - left),
                        mm(self.indent)
                    ),
                );
            }
        }
        self.report_spread(
            "list-marker",
            bullets,
            (bullet_total, "переліків"),
            Place::Set("list", &["marker"]),
            "позиції переліків позначено не знаком «тире» (–)",
        );
        self.report_spread(
            "list-marker",
            enums,
            (enum_total, "переліків"),
            Place::Set("enum", &["numbering"]),
            "позиції нумерованих переліків позначено не «а)» чи «1)»",
        );
    }

    // --- Реферат і зміст ------------------------------------------------------------------

    fn abstract_and_contents(&mut self) {
        let heads: Vec<(usize, usize, String)> = self
            .e
            .iter()
            .enumerate()
            .filter_map(|(i, e)| match &e.item {
                Item::Heading { level, body, .. } => Some((i, *level, body.clone())),
                _ => None,
            })
            .collect();
        // Кількість сторінок береться з останнього запису: запис сторінки документ міг перекрити
        let pages = self
            .e
            .iter()
            .map(|x| x.pg)
            .chain(self.pages.keys().copied())
            .max()
            .unwrap_or(0);
        let structural_name = |b: &str| structural(b).map(|(n, _)| n);

        if let Some(&(a, _, _)) = heads
            .iter()
            .find(|(_, l, b)| *l == 1 && structural_name(b) == Some("реферат"))
        {
            let next = heads
                .iter()
                .find(|(i, l, _)| *i > a && *l == 1)
                .map(|(i, _, _)| *i);
            let loc = self.loc(a);
            if let Some(next) = next {
                let span = self.e[next].pg - self.e[a].pg;
                if span > 1 {
                    self.add(
                        "abstract-length",
                        loc.clone(),
                        format!("реферат займає {span} с., рекомендовано одну"),
                    );
                }
            }
            // Відомості про обсяг (4.3.3)
            let end = next.unwrap_or(self.e.len());
            let text: String = (a..end)
                .filter_map(|j| match &self.e[j].item {
                    Item::Run(r) if r.kind.is_empty() => Some(r.t.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" ");
            let figures = self
                .e
                .iter()
                .filter(|x| matches!(&x.item, Item::Figure { kind, num: Some(_), .. } if kind == "image"))
                .count();
            let tables = self
                .e
                .iter()
                .filter(|x| matches!(&x.item, Item::Figure { kind, num: Some(_), .. } if kind == "table"))
                .count();
            let sources = self
                .e
                .iter()
                .filter_map(|x| match &x.item {
                    Item::Cite { key } => Some(key.as_str()),
                    _ => None,
                })
                .collect::<HashSet<_>>()
                .len();
            let appendix_count = self.appendices.len();
            let actual = |what: &str| match what {
                "pages" => pages,
                "figures" => figures,
                "tables" => tables,
                "appendices" => appendix_count,
                _ => sources,
            };
            for (what, regex) in COUNTS.iter() {
                let Some(c) = regex.captures(&text) else {
                    continue;
                };
                let Ok(stated) = c[1].parse::<usize>() else {
                    continue;
                };
                let real = actual(what);
                // Джерела: у переліку можуть бути й ті, на які немає посилань
                let wrong = if *what == "sources" {
                    stated < real
                } else {
                    stated != real
                };
                if wrong {
                    let name = match *what {
                        "pages" => "сторінок",
                        "figures" => "рисунків",
                        "tables" => "таблиць",
                        "appendices" => "додатків",
                        _ => "джерел",
                    };
                    self.add(
                        "abstract-counts",
                        loc.clone(),
                        format!("у рефераті «{}», а у звіті {name}: {real}", c[0].trim()),
                    );
                }
            }
        }

        // Зміст (4.4.1): потрібен за двох розділів (або розділу й додатка) і ≥ 10 с.
        let chapters = heads
            .iter()
            .filter(|(i, l, b)| {
                *l == 1
                    && structural(b).is_none()
                    && !self.appendices.contains(i)
                    && matches!(self.e[*i].item, Item::Heading { num: Some(_), .. })
            })
            .count();
        let has_contents = heads
            .iter()
            .any(|(_, _, b)| structural_name(b) == Some("зміст"));
        if !has_contents
            && pages >= 10
            && (chapters >= 2 || chapters >= 1 && !self.appendices.is_empty())
        {
            let loc = self.doc.anchor();
            self.add(
                "contents-missing",
                loc,
                format!("у звіті {chapters} розділ(и), {pages} с., а змісту немає"),
            );
        }
        // Переноси у змісті (4.4.5)
        if let Some(i) = (0..self.e.len()).find(
            |&i| matches!(&self.e[i].item, Item::Run(r) if r.kind == "o" && r.hyph == Some(true)),
        ) {
            let at = heads
                .iter()
                .find(|(_, _, b)| structural_name(b) == Some("зміст"))
                .map_or(i, |(h, _, _)| *h);
            let loc = self.loc(at);
            self.add(
                "contents-hyphen",
                loc,
                "у змісті ввімкнено переноси слів".into(),
            );
        }
    }
}

impl Checker<'_> {
    /// Порядок номерів джерел (5.5.1), як їх надруковано: для `@ключ` з
    /// `bibliography` номери дає стиль, тож у коді їх не видно. Набрані
    /// вручну «[N]» перевіряє статична частина правила.
    fn citations(&mut self) {
        if !self.e.iter().any(|x| matches!(x.item, Item::Cite { .. })) {
            return;
        }
        // Надруковані посилання: прогони `z` підряд утворюють одну групу («[1, 2]»)
        let mut groups: Vec<(usize, String)> = Vec::new();
        let mut open = false;
        for i in 0..self.e.len() {
            if self.appendix_of[i].is_some() {
                break;
            }
            match &self.e[i].item {
                Item::Run(r) if r.kind == "z" => {
                    if !open {
                        groups.push((i, String::new()));
                        open = true;
                    }
                    groups.last_mut().unwrap().1.push_str(&r.t);
                }
                Item::Cite { .. } | Item::Ref { .. } => {}
                _ => open = false,
            }
        }
        let mut seen = 0u32;
        for (i, text) in groups {
            for c in rules::CITE_GROUP.captures_iter(&text) {
                if let Some((n, expected)) = rules::cite_out_of_order(&c, &mut seen) {
                    let loc = self.loc(i);
                    self.add(
                        "citation-order",
                        loc,
                        format!(
                            "джерело [{n}] надруковано раніше за [{expected}]: джерела в переліку нумерують у порядку першого посилання, а стиль переліку впорядковує їх інакше"
                        ),
                    );
                    return;
                }
            }
        }
    }
}

/// Номер заголовка: «1», «1.2», «А.1» (буква лише з номером після неї).
fn is_heading_number(t: &str) -> bool {
    NUMBER_RUN.captures(t).is_some_and(|c| {
        let head = &c[1];
        head.chars().all(|ch| ch.is_ascii_digit()) || !c[2].is_empty()
    })
}

fn starts_with_where(head: &str) -> bool {
    let t = head.trim_start();
    t.strip_prefix("де")
        .is_some_and(|rest| rest.chars().next().is_none_or(|c| !c.is_alphabetic()))
}

/// Пункт стандарту про нову сторінку для структурного елемента.
fn new_page_clause(body: &str) -> &'static str {
    match structural(body).map(|(n, _)| n) {
        Some("список авторів") => "4.2.1",
        Some("реферат") => "4.3.1",
        Some("зміст") => "4.4.1",
        Some("скорочення та умовні познаки") => "4.5.2",
        Some("передмова") => "4.6.2",
        Some("вступ") => "5.1.1",
        Some("висновки") => "5.3.1",
        Some("рекомендації") => "5.4.2",
        Some("перелік джерел посилання") => "5.5.1",
        _ => "6.2.2",
    }
}

fn list_pages(pages: &[usize]) -> String {
    let shown: Vec<String> = pages.iter().take(10).map(usize::to_string).collect();
    if pages.len() > 10 {
        format!("{}, …", shown.join(", "))
    } else {
        shown.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligns_by_key_then_order() {
        let s = |v: &[Option<&str>]| v.iter().map(|x| x.map(str::to_owned)).collect::<Vec<_>>();
        let stream = s(&[Some("a"), None, Some("c"), Some("x")]);
        let model = s(&[Some("a"), Some("b"), Some("c")]);
        assert_eq!(align(&stream, &model), [Some(0), Some(1), Some(2), None]);
    }

    #[test]
    fn heading_numbers() {
        assert!(is_heading_number("1"));
        assert!(is_heading_number("1.2."));
        assert!(is_heading_number("А.1"));
        assert!(!is_heading_number("А"));
        assert!(!is_heading_number("ООП"));
    }

    #[test]
    fn where_paragraph() {
        assert!(starts_with_where("де x -- змінна"));
        assert!(!starts_with_where("деталі"));
    }
}
