//! Розкладка з фактів компіляції: те, що видно лише на сторінках, стає
//! фактами моделі ([`crate::work`]) з підставою [`Basis::Layout`].
//!
//! Typst не віддає рядків коду, тож кожен елемент потоку прив'язується до
//! елемента моделі: мітка, текст заголовка чи назви, початок абзацу, порядок.
//! Надруковане, чого в коді не видно (заголовок, який друкує шаблон, таблиця
//! поза фігурою), стає новим елементом моделі після найближчого
//! прив'язаного, з його місцем у коді. Правила над цим — у
//! [`crate::rules::layout`].

use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::sync::LazyLock;

use regex::Regex;

use super::facts::{Entry, Facts, Item, Page, Run};
use crate::model::Code;
use crate::rules::structural;
use crate::text::Loc;
use crate::text::Text;
use crate::work::{
    At, Basis, Counts, Element, Equation, Fact, Figure, FigureKind, FigurePrint, Heading,
    HeadingPrint, Label, List, Paper, Par, Table, TableSplit, Work,
};

static NUMBER_RUN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(\d+|[А-ЯҐЄІЇA-Z])((?:\.\d+)*)(\.?)\s*$").unwrap());

/// Кладе в модель те, що надрукувала компіляція.
pub fn lay_out(work: &mut Work, code: &Code, facts: &Facts) {
    let out = {
        let mut cx = Checker::new(work, code, facts);
        cx.fonts_missing();
        cx.headings();
        cx.figures();
        cx.tables();
        cx.equations();
        cx.footnotes();
        cx.abstract_and_counts();
        cx.citations();
        cx.out
    };
    out.apply(work, code);
}

/// Титульний аркуш і невідомі статиці налаштування сторінки: з коду титульний
/// аркуш — усе до першого заголовка, а розкладка каже напевно; так само на
/// непрозорому шаблоні розкладка підставляє формат і береги (`Basis::Layout`),
/// щоб їх перевірили ті самі статичні правила без дублювання.
pub fn confirm(work: &mut Work, facts: &Facts) {
    // Формат і береги сторінки для непрозорих шаблонів (де жоден елемент не
    // має відомого з коду формату чи берегів, щоб не дублювати відоме з коду)
    let has_code_paper = work
        .looks
        .iter()
        .any(|l| l.paper.basis() == Some(Basis::Code));
    let has_code_margins = work
        .looks
        .iter()
        .any(|l| l.margins.iter().any(|m| m.basis() == Some(Basis::Code)));
    if (!has_code_paper || !has_code_margins)
        && let Some(page) = facts.entries.iter().find_map(|e| match &e.item {
            Item::Page(p) => Some(p),
            _ => None,
        })
    {
        let (t, r, b, l) = page.margins();
        let m_vals = [t, r, b, l];
        for look in &mut work.looks {
            if !has_code_paper && look.paper.known().is_none() {
                look.paper = Fact::new(
                    Paper {
                        name: None,
                        w: Some(page.w),
                        h: Some(page.h),
                    },
                    Basis::Layout,
                    At::default("page", &["paper", "width", "height", "flipped"]),
                );
            }
            if !has_code_margins {
                for (slot, val) in look.margins.iter_mut().zip(m_vals) {
                    if slot.known().is_none() {
                        *slot = Fact::new(val, Basis::Layout, At::default("page", &["margin"]));
                    }
                }
            }
        }
    }

    let Some(k) = work.first_heading() else {
        return;
    };
    let Some(page) = facts.entries.iter().find_map(|e| match e.item {
        Item::Heading { .. } => Some(e.pg),
        _ => None,
    }) else {
        return;
    };
    let Element::Heading(h) = &work.elements[k] else {
        return;
    };
    let title_page = if page > 1 { k } else { 0 };
    work.title_page = laid(title_page, &h.loc);
    if title_page == 0 {
        for pages in &mut work.pages {
            pages.title = None;
        }
    }
}

/// Факт розкладки, що стоїть на елементі в `loc`.
fn laid<T>(value: T, loc: &Loc) -> Fact<T> {
    Fact::new(value, Basis::Layout, At::Element(loc.clone()))
}

/// Що компіляція додає до моделі.
#[derive(Default)]
struct Out {
    /// Елемент моделі (чи новий, якщо `None`: після якого елемента) і факти.
    headings: Vec<(Target, Loc, usize, HeadingPrint)>,
    /// Рисунки й таблиці: запис потоку, куди, місце, вид, як надруковано.
    figures: Vec<(usize, Target, Loc, FigureKind, FigurePrint)>,
    /// Перенесені таблиці у фігурах: запис фігури -> перенесення.
    splits: HashMap<usize, TableSplit>,
    /// Перенесені таблиці поза фігурами.
    tables: Vec<(Target, Loc, Option<String>, TableSplit)>,
    equations: Vec<(Target, Loc, f64)>,
    /// Розкладка дійшла до фігури-таблиці моделі: вона не переходить, якщо
    /// перенесення не знайдено.
    table_figures: Vec<usize>,
    footnotes: Vec<(usize, usize, Loc)>,
    page_count: usize,
    missing_fonts: Vec<Loc>,
    counts: Counts,
    citations: Vec<(Loc, String)>,
    /// Реферат: запис заголовка, сторінок до наступного розділу, текст.
    abstract_: Option<(usize, Option<usize>, String)>,
}

/// Куди факт: наявний елемент моделі чи новий після елемента (`None`: на
/// початку), у порядку запису потоку.
#[derive(Clone, Copy)]
enum Target {
    Element(usize),
    After(Option<usize>, usize),
}

impl Out {
    fn apply(self, work: &mut Work, code: &Code) {
        let anchor = code.anchor();
        let mut new: Vec<(Option<usize>, usize, Element)> = Vec::new();
        let abstract_at = self.abstract_.as_ref().map(|(i, ..)| *i);
        let (mut span, mut text) = match self.abstract_ {
            Some((_, s, t)) => (s, Some(t)),
            None => (None, None),
        };
        for (target, loc, stream, mut print) in self.headings {
            if abstract_at == Some(stream) {
                print.span = span.take();
                print.text = text.take();
            }
            match target {
                Target::Element(k) => {
                    if let Element::Heading(h) = &mut work.elements[k] {
                        h.printed = laid(Box::new(print), &loc);
                    }
                }
                Target::After(after, order) => {
                    let mut title = Text::default();
                    title.push_mapped(&print.body, loc.file, loc.range.clone());
                    new.push((
                        after,
                        order,
                        Element::Heading(Heading {
                            level: print.level,
                            title,
                            label: None,
                            numbered: false,
                            generated: true,
                            markup: None,
                            appendix: Fact::Unknown,
                            new_page: Fact::Unknown,
                            par_indent: Fact::Unknown,
                            look: None,
                            printed: laid(Box::new(print), &loc),
                            loc,
                        }),
                    ));
                }
            }
        }
        for k in self.table_figures {
            if let Element::Figure(f) = &mut work.elements[k] {
                f.split = laid(None, &f.loc);
            }
        }
        let mut splits = self.splits;
        for (stream, target, loc, kind, print) in self.figures {
            let split = splits.remove(&stream);
            match target {
                Target::Element(k) => {
                    if let Element::Figure(f) = &mut work.elements[k] {
                        f.printed = laid(print, &loc);
                        if let Some(split) = split {
                            f.split = laid(Some(split), &loc);
                        }
                    }
                }
                Target::After(after, order) => new.push((
                    after,
                    order,
                    Element::Figure(Figure {
                        kind,
                        label: None,
                        tick: 0,
                        floating: false,
                        header: None,
                        caption: None,
                        appendix: Fact::Unknown,
                        look: Rc::default(),
                        chapter: None,
                        number: Fact::Unknown,
                        printed: laid(print, &loc),
                        split: match split {
                            Some(split) => laid(Some(split), &loc),
                            None => Fact::Unknown,
                        },
                        loc,
                    }),
                )),
            }
        }
        for (target, loc, caption, split) in self.tables {
            let (after, order) = match target {
                Target::Element(k) => (Some(k), usize::MAX),
                Target::After(a, o) => (a, o),
            };
            new.push((
                after,
                order,
                Element::Table(Table {
                    split: laid(split, &loc),
                    caption,
                    loc,
                }),
            ));
        }
        for (target, loc, gap) in self.equations {
            match target {
                Target::Element(k) => {
                    if let Element::Equation(q) = &mut work.elements[k] {
                        q.number_gap = laid(gap, &loc);
                    }
                }
                Target::After(after, order) => new.push((
                    after,
                    order,
                    Element::Equation(Equation {
                        label: None,
                        follows: false,
                        referenced: false,
                        appendix: Fact::Unknown,
                        look: Rc::default(),
                        chapter: None,
                        number: Fact::Unknown,
                        number_gap: laid(gap, &loc),
                        loc,
                    }),
                )),
            }
        }
        insert(work, new);
        work.footnotes.pages = laid(self.footnotes, &anchor);
        work.page_count = laid(self.page_count, &anchor);
        work.missing_fonts = laid(self.missing_fonts, &anchor);
        work.counts = laid(self.counts, &anchor);
        work.citations = laid(self.citations, &anchor);
    }
}

/// Вставляє нові елементи після зазначених (у порядку потоку) і переносить
/// індекси, що вказують на елементи.
fn insert(work: &mut Work, mut new: Vec<(Option<usize>, usize, Element)>) {
    if new.is_empty() {
        return;
    }
    new.sort_by_key(|(after, order, _)| (after.map_or(0, |a| a + 1), *order));
    let old = std::mem::take(&mut work.elements);
    let mut map = Vec::with_capacity(old.len() + 1);
    let mut new = new.into_iter().peekable();
    let mut out = Vec::new();
    for (k, e) in old.into_iter().enumerate() {
        while let Some(x) = new.next_if(|(after, ..)| after.map_or(0, |a| a + 1) <= k) {
            out.push(x.2);
        }
        map.push(out.len());
        out.push(e);
    }
    out.extend(new.map(|x| x.2));
    map.push(out.len());
    for e in &mut out {
        if let Element::List(l) = e {
            l.prev_par = l.prev_par.map(|p| map[p]);
        }
    }
    for r in &mut work.runs {
        r.element = map[r.element];
    }
    if let Fact::Known(k, _) = &mut work.title_page {
        *k = map[*k];
    }
    work.elements = out;
}

/// Ключ прив'язки: текст без розмітки й розділових знаків, малими літерами.
fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Зіставляє елементи потоку з елементами моделі через найдовшу спільну
/// підпослідовність (LCS) непустих ключів, а в проміжках між зіставленими
/// якорями — за порядком, якщо в них порівну елементів. `None` або порожній
/// ключ у LCS не збігається.
fn align(stream: &[Option<String>], model: &[Option<String>]) -> Vec<Option<usize>> {
    let mut out = vec![None; stream.len()];
    let mut used = vec![false; model.len()];
    let matches = |i: usize, j: usize| -> bool {
        match (&stream[i], &model[j]) {
            (Some(a), Some(b)) if !a.is_empty() => a == b,
            _ => false,
        }
    };

    // Фільтруємо до позицій з непустими ключами: LCS по них менший і швидший
    let s_idx: Vec<usize> = stream
        .iter()
        .enumerate()
        .filter_map(|(i, k)| k.as_deref().filter(|s| !s.is_empty()).map(|_| i))
        .collect();
    let m_idx: Vec<usize> = model
        .iter()
        .enumerate()
        .filter_map(|(j, k)| k.as_deref().filter(|s| !s.is_empty()).map(|_| j))
        .collect();

    // Зрізаємо спільний префікс і суфікс
    let mut pre = 0;
    while pre < s_idx.len() && pre < m_idx.len() && matches(s_idx[pre], m_idx[pre]) {
        out[s_idx[pre]] = Some(m_idx[pre]);
        used[m_idx[pre]] = true;
        pre += 1;
    }
    let mut s_end = s_idx.len();
    let mut m_end = m_idx.len();
    while s_end > pre && m_end > pre && matches(s_idx[s_end - 1], m_idx[m_end - 1]) {
        s_end -= 1;
        m_end -= 1;
        out[s_idx[s_end]] = Some(m_idx[m_end]);
        used[m_idx[m_end]] = true;
    }

    let sa = &s_idx[pre..s_end];
    let mb = &m_idx[pre..m_end];
    let (n, m) = (sa.len(), mb.len());
    if n > 0 && m > 0 {
        if n * m <= 250_000 {
            let stride = m + 1;
            let mut dp = vec![0u32; (n + 1) * stride];
            for i in (0..n).rev() {
                for j in (0..m).rev() {
                    dp[i * stride + j] = if matches(sa[i], mb[j]) {
                        dp[(i + 1) * stride + j + 1] + 1
                    } else {
                        dp[(i + 1) * stride + j].max(dp[i * stride + j + 1])
                    };
                }
            }
            let (mut i, mut j) = (0, 0);
            while i < n && j < m {
                if matches(sa[i], mb[j]) {
                    out[sa[i]] = Some(mb[j]);
                    used[mb[j]] = true;
                    i += 1;
                    j += 1;
                } else if dp[(i + 1) * stride + j] >= dp[i * stride + j + 1] {
                    i += 1;
                } else {
                    j += 1;
                }
            }
        } else {
            // Смуговий LCS для дуже великих документів (тисячі абзаців)
            let band = n.abs_diff(m) + 64;
            let width = 2 * band + 1;
            let mut dp = vec![0u32; (n + 1) * width];
            let at = |i: usize, j: usize| -> Option<usize> {
                let d = j as isize - i as isize;
                if d.unsigned_abs() <= band {
                    Some(i * width + (d + band as isize) as usize)
                } else {
                    None
                }
            };
            let get = |dp: &[u32], i: usize, j: usize| -> u32 {
                if i >= n || j >= m {
                    return 0;
                }
                at(i, j).map_or(0, |k| dp[k])
            };
            for i in (0..n).rev() {
                let j_lo = i.saturating_sub(band);
                let j_hi = (i + band + 1).min(m);
                for j in (j_lo..j_hi).rev() {
                    let val = if matches(sa[i], mb[j]) {
                        get(&dp, i + 1, j + 1) + 1
                    } else {
                        get(&dp, i + 1, j).max(get(&dp, i, j + 1))
                    };
                    if let Some(k) = at(i, j) {
                        dp[k] = val;
                    }
                }
            }
            let (mut i, mut j) = (0, 0);
            while i < n && j < m {
                if matches(sa[i], mb[j]) {
                    out[sa[i]] = Some(mb[j]);
                    used[mb[j]] = true;
                    i += 1;
                    j += 1;
                } else if get(&dp, i + 1, j) >= get(&dp, i, j + 1) {
                    i += 1;
                } else {
                    j += 1;
                }
            }
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

/// Обхід потоку компіляції: прив'язує записи до елементів моделі й збирає
/// факти розкладки ([`Out`]).
struct Checker<'a> {
    work: &'a Work,
    code: &'a Code,
    e: &'a [Entry],
    facts: &'a Facts,
    out: Out,
    /// Прив'язка записів потоку до коду.
    locs: Vec<Option<Loc>>,
    /// Для кожного запису: найближчий прив'язаний не пізніше за нього.
    nearest: Vec<Option<usize>>,
    /// Запис потоку -> елемент моделі.
    elem_of: Vec<Option<usize>>,
    /// Запис потоку заголовка -> заголовок моделі.
    heading_model: HashMap<usize, &'a Heading>,
    /// Запис -> рівень-1 заголовок додатка, у якому він стоїть (номер додатка з 0).
    appendix_of: Vec<Option<usize>>,
    /// Записи заголовків додатків першого рівня.
    appendices: Vec<usize>,
    pages: BTreeMap<usize, &'a Page>,
    /// Перший заголовок: усе до нього належить титульному аркушу.
    body_start: usize,
    /// Перше посилання на мітку: індекс запису.
    first_ref: HashMap<&'a str, usize>,
}

impl<'a> Checker<'a> {
    fn new(work: &'a Work, code: &'a Code, facts: &'a Facts) -> Self {
        let e = &facts.entries[..];
        let mut cx = Checker {
            work,
            code,
            e,
            facts,
            out: Out::default(),
            locs: vec![None; e.len()],
            nearest: Vec::new(),
            elem_of: vec![None; e.len()],
            heading_model: HashMap::new(),
            appendix_of: vec![None; e.len()],
            appendices: Vec::new(),
            pages: BTreeMap::new(),
            body_start: e.len(),
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
        cx
    }

    // --- Прив'язка до коду ------------------------------------------------------

    fn map_headings(&mut self) {
        let model: Vec<(usize, &Heading)> = self
            .work
            .elements
            .iter()
            .enumerate()
            .filter_map(|(k, b)| match b {
                Element::Heading(h) => Some((k, h)),
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
        let mkeys: Vec<_> = model.iter().map(|(_, h)| keys(h.title.trimmed())).collect();
        for (k, m) in align(&skeys, &mkeys).into_iter().enumerate() {
            if let Some(m) = m {
                let (j, h) = model[m];
                self.locs[stream[k]] = Some(h.loc.clone());
                self.elem_of[stream[k]] = Some(j);
                self.heading_model.insert(stream[k], h);
            } else if let Item::Heading { body, .. } = &self.e[stream[k]].item {
                self.locs[stream[k]] = self.search(body);
            }
        }
    }

    fn map_figures(&mut self) {
        let model: Vec<(usize, &Figure)> = self
            .work
            .elements
            .iter()
            .enumerate()
            .filter_map(|(k, b)| match b {
                Element::Figure(f) => Some((k, f)),
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
            .map(|(_, f)| match &f.label {
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
                let (j, f) = model[m];
                self.elem_of[stream[k]] = Some(j);
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
        for (file, f) in self.code.files.iter().enumerate() {
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
        let model: Vec<(usize, &Loc, Option<&Label>)> = self
            .work
            .elements
            .iter()
            .enumerate()
            .filter_map(|(k, b)| match b {
                Element::Equation(q) => Some((k, &q.loc, q.label.as_ref())),
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
            .map(|(_, _, l)| match l {
                Some(Label::Name(n)) => Some(n.clone()),
                _ => None,
            })
            .collect();
        for (k, m) in align(&skeys, &mkeys).into_iter().enumerate() {
            if let Some(m) = m {
                self.locs[stream[k]] = Some(model[m].1.clone());
                self.elem_of[stream[k]] = Some(model[m].0);
            }
        }
    }

    fn map_footnotes(&mut self) {
        let stream = self.indices(|i| matches!(i, Item::Footnote { again: false, .. }));
        let key = |s: &str| {
            let n: String = norm(s).chars().take(24).collect();
            (n.chars().count() >= 4).then_some(n)
        };
        let skeys: Vec<_> = stream
            .iter()
            .map(|&i| match &self.e[i].item {
                Item::Footnote { head: Some(h), .. } => key(h),
                _ => None,
            })
            .collect();
        let mkeys: Vec<_> = self.work.footnotes.heads.iter().map(|h| key(h)).collect();
        for (k, m) in align(&skeys, &mkeys).into_iter().enumerate() {
            if let Some(m) = m {
                self.locs[stream[k]] = Some(self.work.footnotes.at[m].clone());
            }
        }
    }

    fn map_pars(&mut self) {
        let model: Vec<(usize, &Par)> = self
            .work
            .elements
            .iter()
            .enumerate()
            .filter_map(|(k, b)| match b {
                Element::Par(p) => Some((k, p)),
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
        let mkeys: Vec<_> = model.iter().map(|(_, p)| key(p.text.as_str())).collect();
        for (k, m) in align(&skeys, &mkeys).into_iter().enumerate() {
            // Абзац без ключа за порядком не зіставляється: шаблон додає свої
            if let (Some(m), Some(_)) = (m, &skeys[k]) {
                self.locs[stream[k]] = model[m].1.text.whole();
                self.elem_of[stream[k]] = Some(model[m].0);
            }
        }
    }

    fn map_lists(&mut self) {
        let model: Vec<(usize, &List)> = self
            .work
            .elements
            .iter()
            .enumerate()
            .filter_map(|(k, b)| match b {
                Element::List(l) => Some((k, l)),
                _ => None,
            })
            .collect();
        let stream = self.indices(|i| matches!(i, Item::List { .. } | Item::Enum { .. }));
        let key = |s: &str| {
            let n: String = norm(s).chars().take(24).collect();
            (n.chars().count() >= 4).then_some(n)
        };
        let skeys: Vec<_> = stream
            .iter()
            .map(|&i| match &self.e[i].item {
                Item::List { head } | Item::Enum { head } => key(head),
                _ => None,
            })
            .collect();
        let mkeys: Vec<_> = model
            .iter()
            .map(|(_, l)| l.items.first().and_then(|it| key(it.text.as_str())))
            .collect();
        for (k, m) in align(&skeys, &mkeys).into_iter().enumerate() {
            if let Some(m) = m {
                self.locs[stream[k]] = model[m].1.items.first().map(|it| it.loc.clone());
                self.elem_of[stream[k]] = Some(model[m].0);
            }
        }
    }

    /// Додатки й розділи: де кожен запис.
    fn regions(&mut self) {
        let mut appendix: Option<usize> = None;
        for i in 0..self.e.len() {
            if let Item::Heading {
                level: 1,
                num,
                body,
                ..
            } = &self.e[i].item
            {
                let model_appendix = self.heading_model.get(&i).map(|h| h.appendix.is(&true));
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
                } else if structural(body).is_some() || num.is_some() {
                    appendix = None;
                }
            }
            self.appendix_of[i] = appendix;
        }
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
            .unwrap_or_else(|| self.code.anchor())
    }

    /// Куди факт про запис `i`: його елемент моделі, а якщо елемента немає —
    /// новий після елемента найближчого прив'язаного запису перед ним.
    fn target(&self, i: usize) -> Target {
        match self.elem_of[i] {
            Some(k) => Target::Element(k),
            None => Target::After((0..i).rev().find_map(|j| self.elem_of[j]), i),
        }
    }

    fn page(&self, pg: usize) -> Option<&'a Page> {
        self.pages
            .get(&pg)
            .copied()
            .or_else(|| self.pages.values().next().copied())
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

    /// Чи запис належить до вмісту сторінки (а не службовий запис, виноска чи номер).
    fn is_content(item: &Item) -> bool {
        match item {
            Item::Par(p) => p.kind != "n",
            Item::Run(r) => !matches!(r.kind.as_str(), "n" | "s"),
            Item::Heading { .. }
            | Item::Figure { .. }
            | Item::Equation { .. }
            | Item::Table { .. }
            | Item::List { .. }
            | Item::Enum { .. }
            | Item::Raw
            | Item::Image => true,
            _ => false,
        }
    }

    // --- Сторінки -------------------------------------------------------------------

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
                .unwrap_or_else(|| self.code.setting("text", &["font"]));
            if seen.insert((loc.file, loc.range.start)) {
                self.out.missing_fonts.push(loc);
            }
        }
    }

    /// Рядок попередження typst у файлі документа.
    fn warning_loc(&self, file: Option<&str>, line: Option<usize>) -> Option<Loc> {
        let (file, line) = (file?, line?);
        let idx = self
            .code
            .files
            .iter()
            .position(|f| f.path.ends_with(file.trim_start_matches('/')))?;
        let source = &self.code.files[idx].source;
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

    /// Заголовки, як їх надруковано: номер, назва, сторінка й що за ними.
    fn headings(&mut self) {
        let heads = self.indices(|i| matches!(i, Item::Heading { .. }));
        for &i in &heads {
            let entry = &self.e[i];
            let Item::Heading {
                level, num, body, ..
            } = &entry.item
            else {
                continue;
            };
            let runs = self.heading_runs(i);
            let number = runs
                .first()
                .filter(|(_, r)| is_heading_number(&r.t))
                .map(|(_, r)| r.t.trim().to_owned());
            let last = runs.last().map_or(i, |(j, _)| *j);
            let last_page = self.e[last].pg.max(entry.pg);
            let next = (last + 1..self.e.len())
                .find(|&j| Self::is_content(&self.e[j].item))
                .map(|j| {
                    (
                        self.e[j].pg,
                        matches!(self.e[j].item, Item::Heading { level: 1, .. }),
                    )
                });
            self.out.headings.push((
                self.target(i),
                self.loc(i),
                i,
                HeadingPrint {
                    level: *level,
                    body: body.clone(),
                    numbering: num.is_some(),
                    number,
                    last_page,
                    next,
                    span: None,
                    text: None,
                },
            ));
        }
    }

    // --- Рисунки й таблиці ---------------------------------------------------------------

    /// Рисунки й таблиці: сторінка й сторінка першого посилання перед ними.
    fn figures(&mut self) {
        let figs = self.indices(
            |i| matches!(i, Item::Figure { kind, .. } if kind == "image" || kind == "table"),
        );
        for &i in &figs {
            let entry = &self.e[i];
            let Item::Figure {
                kind,
                label,
                floating,
                caption,
                ..
            } = &entry.item
            else {
                continue;
            };
            let ref_page = label
                .as_ref()
                .and_then(|l| self.first_ref.get(l.as_str()))
                .filter(|&&r| r < i && !floating && self.appendix_of[i].is_none())
                .map(|&r| self.e[r].pg);
            let kind = if kind == "table" {
                FigureKind::Table
            } else {
                FigureKind::Image
            };
            if let (Some(k), FigureKind::Table) = (self.elem_of[i], kind) {
                self.out.table_figures.push(k);
            }
            self.out.figures.push((
                i,
                self.target(i),
                self.loc(i),
                kind,
                FigurePrint {
                    caption: caption.clone(),
                    page: entry.pg,
                    ref_page,
                },
            ));
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
            let caption = match &self.e[owner].item {
                Item::Figure {
                    caption: Some(c), ..
                } => Some(c.clone()),
                _ => None,
            };
            // Номер таблиці (якщо відомий з моделі): для перевірки відповідності
            // напису «Продовження таблиці N»
            let table_num = self.elem_of[owner].and_then(|k| match &self.work.elements[k] {
                Element::Figure(f) => f.number.known().map(|s| s.trim().to_owned()),
                _ => None,
            });
            let unmarked: Vec<usize> = (from + 1..=to)
                .filter(|&pg| {
                    let top_margin = self.page(pg).map_or(56.7, |p| p.margins().0);
                    let slice_bottom = (pg == to).then_some(self.e[end].y);
                    !self.e.iter().enumerate().any(|(j, x)| {
                        if x.pg != pg {
                            return false;
                        }
                        let Item::Run(r) = &x.item else {
                            return false;
                        };
                        let t = r.t.trim_start();
                        if !(t.starts_with("Продовження") || t.starts_with("Кінець"))
                        {
                            return false;
                        }
                        // Напис має стояти над перенесеною частиною таблиці (у її
                        // головці/фігурі або вгорі сторінки над нею), а не в
                        // тексті нижче за таблицю
                        let inside_table =
                            j > start && j < end && matches!(r.kind.as_str(), "t" | "f");
                        let above_slice =
                            slice_bottom.is_none_or(|b| x.y <= b + 2.0) && x.y <= top_margin + 60.0;
                        if !(inside_table || above_slice) {
                            return false;
                        }
                        // Якщо в написі зазначено номер таблиці і ми знаємо номер
                        // цієї таблиці, вони мають збігатися
                        if let (Some(want), Some(found)) = (&table_num, continuation_number(t))
                            && !want.eq_ignore_ascii_case(found)
                        {
                            return false;
                        }
                        true
                    })
                })
                .collect();
            let split = TableSplit {
                to,
                header,
                repeat,
                unmarked,
            };
            // Таблиця в рисунку чи таблиці: факт фігури (перша таблиця в ній)
            let figure = matches!(&self.e[owner].item,
                Item::Figure { kind, .. } if kind == "image" || kind == "table");
            if figure && !self.out.splits.contains_key(&owner) {
                self.out.splits.insert(owner, split);
            } else {
                self.out
                    .tables
                    .push((self.target(owner), self.loc(owner), caption, split));
            }
        }
    }

    // --- Формули ------------------------------------------------------------------------

    /// Де закінчується номер формули: видно лише з розкладки (формула в
    /// блоці, переліку).
    fn equations(&mut self) {
        let eqs = self.indices(|i| matches!(i, Item::Equation { .. }));
        for (n, &i) in eqs.iter().enumerate() {
            let Item::Equation { num, .. } = &self.e[i].item else {
                continue;
            };
            if num.is_none() {
                continue;
            }
            // Прогони формули: числа тіла й номер
            let next = eqs.get(n + 1).copied().unwrap_or(self.e.len());
            let runs: Vec<(usize, &Run)> = (i + 1..next)
                .map_while(|j| match &self.e[j].item {
                    Item::Run(r) if r.kind == "e" => Some(Some((j, r))),
                    Item::Run(_) | Item::Ref { .. } | Item::Cite { .. } => Some(None),
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
            let Some(&(j, r)) = tail.first() else {
                continue;
            };
            let (_, right) = self.text_area(self.e[j].pg);
            let end = self.e[j].x + r.w.unwrap_or(0.0);
            self.out
                .equations
                .push((self.target(i), self.loc(i), right - end));
        }
    }

    // --- Виноски --------------------------------------------------------------------------

    /// Скільки виносок на кожній сторінці.
    fn footnotes(&mut self) {
        let notes = self.indices(|i| matches!(i, Item::Footnote { again: false, .. }));
        let mut by_page: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for &i in &notes {
            by_page.entry(self.e[i].pg).or_default().push(i);
        }
        for (pg, list) in &by_page {
            self.out
                .footnotes
                .push((*pg, list.len(), self.loc(list[0])));
        }
    }

    // --- Реферат і зміст ------------------------------------------------------------------

    fn abstract_and_counts(&mut self) {
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
        self.out.page_count = self
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
            let end = next.unwrap_or(self.e.len());
            let text: String = (a..end)
                .filter_map(|j| match &self.e[j].item {
                    Item::Run(r) if r.kind.is_empty() || r.kind == "l" || r.kind == "t" => {
                        Some(r.t.as_str())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" ");
            self.out.abstract_ = Some((a, next.map(|next| self.e[next].pg - self.e[a].pg), text));
        }
        let numbered = |k: &str| {
            self.e
                .iter()
                .filter(|x| matches!(&x.item, Item::Figure { kind, num: Some(_), .. } if kind == k))
                .count()
        };
        self.out.counts.images = numbered("image");
        self.out.counts.tables = numbered("table");
        self.out.counts.sources = self
            .e
            .iter()
            .filter_map(|x| match &x.item {
                Item::Cite { key } => Some(key.as_str()),
                _ => None,
            })
            .collect::<HashSet<_>>()
            .len();
        self.out.counts.appendices = self.appendices.len();
        self.out.counts.chapters = heads
            .iter()
            .filter(|(i, l, b)| {
                *l == 1
                    && structural(b).is_none()
                    && !self.appendices.contains(i)
                    && matches!(self.e[*i].item, Item::Heading { num: Some(_), .. })
            })
            .count();
    }

    /// Надруковані посилання на джерела до додатків: для `@ключ` з
    /// `bibliography` номери дає стиль, тож у коді їх не видно.
    fn citations(&mut self) {
        if !self.e.iter().any(|x| matches!(x.item, Item::Cite { .. })) {
            return;
        }
        // Прогони `z` підряд утворюють одну групу («[1, 2]»)
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
        self.out.citations = groups
            .into_iter()
            .map(|(i, text)| (self.loc(i), text))
            .collect();
    }
}

/// Номер таблиці в написі «Продовження таблиці 2.1» / «Кінець табл. А.1».
fn continuation_number(t: &str) -> Option<&str> {
    let mut parts = t.split_whitespace();
    let _first = parts.next()?;
    let second = parts.next()?;
    if !second.to_lowercase().starts_with("табл") {
        return None;
    }
    let num = parts.next()?.trim_end_matches('.');
    (!num.is_empty()).then_some(num)
}

/// Номер заголовка: «1», «1.2», «А.1» (буква лише з номером після неї).
fn is_heading_number(t: &str) -> bool {
    NUMBER_RUN.captures(t).is_some_and(|c| {
        let head = &c[1];
        head.chars().all(|ch| ch.is_ascii_digit()) || !c[2].is_empty()
    })
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
    fn lcs_align_survives_repeated_template_entry() {
        let s = |v: &[Option<&str>]| v.iter().map(|x| x.map(str::to_owned)).collect::<Vec<_>>();
        // Шаблон вставляє повторюваний заголовок "d" на початку: жадібний пошук
        // стрибнув би на кінець і втратив би "a", "b", "c", а LCS лишає їх на місці
        let stream = s(&[Some("d"), Some("a"), Some("b"), Some("c"), Some("d")]);
        let model = s(&[Some("a"), Some("b"), Some("c"), Some("d")]);
        assert_eq!(
            align(&stream, &model),
            [None, Some(0), Some(1), Some(2), Some(3)]
        );
    }

    #[test]
    fn heading_numbers() {
        assert!(is_heading_number("1"));
        assert!(is_heading_number("1.2."));
        assert!(is_heading_number("А.1"));
        assert!(!is_heading_number("А"));
        assert!(!is_heading_number("ООП"));
    }
}
