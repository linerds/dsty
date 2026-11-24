//! Правила над фактами розкладки (повна перевірка): що де стоїть на
//! сторінках, як typst чи шаблон надрукували номери й назви.

use std::sync::LazyLock;

use regex::Regex;

use super::{
    CITE_GROUP, Checker, Msg, STRUCTURE_VARIANT, canonical_name, cite_out_of_order, structural,
    structural_variant, with_help,
};
use crate::text::{mm, quote};
use crate::work::{At, Element, FigureKind, Heading, HeadingPrint, TableSplit};

/// Відомості про обсяг у рефераті (4.3.3).
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

impl Checker<'_> {
    pub(super) fn layout(&mut self) {
        // Шрифту немає в системі: typst попереджає й набирає запасним
        for loc in self.work.missing_fonts.known().into_iter().flatten() {
            self.add(
                "font-missing",
                Some(loc.clone()),
                "шрифту Times New Roman немає в системі: typst набирає текст запасним",
                Vec::new(),
            );
        }
        self.printed_headings();
        self.printed_figures();
        self.printed_tables();
        self.printed_equations();
        // 7.9.3: не більше ніж чотири виноски на сторінці
        for (pg, count, loc) in self.work.footnotes.pages.known().into_iter().flatten() {
            if *count > 4 {
                self.add(
                    "footnote-count",
                    Some(loc.clone()),
                    format!("на с. {pg} виносок: {count}, дозволено не більше ніж чотири"),
                    Vec::new(),
                );
            }
        }
        self.printed_abstract();
        self.printed_citations();
    }

    /// Заголовки з тим, як їх надруковано.
    fn printed(&self) -> impl Iterator<Item = (&Heading, &HeadingPrint)> {
        self.work.elements.iter().filter_map(|e| match e {
            Element::Heading(h) => h.printed.known().map(|p| (h, &**p)),
            _ => None,
        })
    }

    /// Заголовки, як їх надруковано: номер на структурному елементі, який
    /// друкує шаблон, назва переліку джерел від typst, заголовок унизу сторінки.
    fn printed_headings(&mut self) {
        let found: Vec<_> = self
            .printed()
            .map(|(h, p)| {
                // Заголовок, який друкує шаблон, у коді не набрано: у
                // повідомленні він стоїть так, як надруковано
                let title = if h.generated {
                    p.body.as_str()
                } else {
                    h.title.trimmed()
                };
                // Назву, набрану в коді, уже перевірила статична частина
                let named_in_code =
                    !h.generated && h.level <= 2 && canonical_name(h.title.trimmed()).is_some();
                (h.loc.clone(), quote(title), h.numbered, named_in_code, p)
            })
            .collect();
        let mut out = Vec::new();
        for (loc, shown, numbered_in_code, named_in_code, p) in found {
            let is_structural = p.level == 1 && structural(&p.body).is_some();
            // 7.1.18: заголовок, який шаблон друкує з номером
            if is_structural
                && p.numbering
                && let Some(number) = &p.number
                && !numbered_in_code
            {
                let msg = Msg::from(format!(
                    "структурний елемент {shown} надруковано з номером «{number}»"
                ));
                out.push((
                    "heading-numbered",
                    loc.clone(),
                    if structural_variant(&p.body) {
                        msg.guessed(STRUCTURE_VARIANT)
                    } else {
                        msg
                    },
                ));
            }
            // 7.1.18: надрукована назва структурного елемента (заголовок, який
            // друкує typst чи шаблон: «Бібліографія»)
            if p.level <= 2
                && !named_in_code
                && let Some(canonical) = canonical_name(&p.body)
            {
                out.push((
                    "structure-name",
                    loc.clone(),
                    with_help(
                        format!("структурний елемент надруковано як {}", quote(&p.body)),
                        format!("Перейменувати на «{canonical}»"),
                    )
                    .guessed(STRUCTURE_VARIANT),
                ));
            }
            // 7.1.25: заголовок в останньому рядку сторінки
            if let Some((pg, chapter)) = p.next
                && pg > p.last_page
                && !chapter
            {
                out.push((
                    "heading-orphan",
                    loc.clone(),
                    format!(
                        "заголовок {shown} стоїть унизу с. {}, а текст після нього — на с. {pg}",
                        p.last_page
                    )
                    .into(),
                ));
            }
        }
        for (code, loc, msg) in out {
            self.add(code, Some(loc), msg, Vec::new());
        }
    }

    /// 7.5.2, 7.6.3: рисунок і таблицю подають не далі ніж на наступній
    /// сторінці після першого посилання.
    fn printed_figures(&mut self) {
        let mut out = Vec::new();
        for e in &self.work.elements {
            let Element::Figure(f) = e else { continue };
            let Some(p) = f.printed.known() else { continue };
            let name = match f.kind {
                FigureKind::Table => "таблиця",
                FigureKind::Image => "рисунок",
            };
            let named = match &p.caption {
                Some(c) => format!("{name} {}", quote(c)),
                None => name.to_owned(),
            };
            if let Some(r) = p.ref_page
                && p.page > r + 1
            {
                out.push((
                    f.printed.at().cloned(),
                    with_help(
                        format!("{named} на с. {}, а перше посилання на с. {r}", p.page),
                        "Подати не далі ніж на наступній сторінці після посилання",
                    ),
                ));
            }
        }
        for (at, msg) in out {
            self.add("figure-far-from-ref", at_loc(at), msg, Vec::new());
        }
    }

    /// Перенесені таблиці: головка й напис «Продовження таблиці» (7.6.9).
    fn printed_tables(&mut self) {
        let splits: Vec<(Option<String>, Option<At>, &TableSplit)> = self
            .work
            .elements
            .iter()
            .filter_map(|e| match e {
                Element::Figure(f) => match f.split.known() {
                    Some(Some(t)) => Some((
                        f.printed.known().and_then(|p| p.caption.clone()),
                        f.split.at().cloned(),
                        t,
                    )),
                    _ => None,
                },
                Element::Table(t) => t
                    .split
                    .known()
                    .map(|s| (t.caption.clone(), t.split.at().cloned(), s)),
                _ => None,
            })
            .collect();
        let mut out = Vec::new();
        for (caption, at, t) in splits {
            let name = match &caption {
                Some(c) => format!("таблиця {}", quote(c)),
                None => "таблиця".into(),
            };
            let to = t.to;
            if !t.header {
                out.push((
                    "table-no-header",
                    at.clone(),
                    format!(
                        "{name} переходить на с. {to} без головки (table.header), тож вона не повторюється"
                    ),
                ));
            } else if !t.repeat {
                out.push((
                    "table-no-header",
                    at.clone(),
                    format!("{name} переходить на с. {to}, а головку не повторено (repeat: false)"),
                ));
            }
            if !t.unmarked.is_empty() {
                out.push((
                    "table-continuation",
                    at,
                    format!(
                        "{name} переходить на с. {}, а над перенесеною частиною немає «Продовження таблиці N»",
                        list_pages(&t.unmarked)
                    ),
                ));
            }
        }
        for (code, at, msg) in out {
            self.add(code, at_loc(at), msg, Vec::new());
        }
    }

    /// 7.10.2: номер формули в крайньому правому положенні.
    fn printed_equations(&mut self) {
        // Номер ліворуч статика вже назвала
        let left = self.work.elements.iter().any(|b| {
            matches!(b, Element::Equation(q) if q.look
                .number_align
                .known()
                .is_some_and(|a| a.contains("left") || a.contains("start")))
        });
        if left {
            return;
        }
        let gaps: Vec<_> = self
            .work
            .elements
            .iter()
            .filter_map(|e| match e {
                Element::Equation(q) => q.number_gap.known().map(|g| (q.loc.clone(), *g)),
                _ => None,
            })
            .collect();
        for (loc, gap) in gaps {
            if gap > 3.0 {
                self.add(
                    "formula-numbering",
                    Some(loc),
                    with_help(
                        format!(
                            "номер формули закінчується за {} від правого берега",
                            mm(gap)
                        ),
                        "Друкувати в крайньому правому положенні",
                    ),
                    Vec::new(),
                );
            }
        }
    }

    /// Обсяг реферату й відомості в ньому (4.3.1, 4.3.3); зміст (4.4.1).
    fn printed_abstract(&mut self) {
        let (Some(&pages), Some(counts)) = (self.work.page_count.known(), self.work.counts.known())
        else {
            return;
        };
        let abstract_ = self
            .printed()
            .find_map(|(h, p)| Some((h.loc.clone(), p.span, p.text.clone()?)));
        let mut out = Vec::new();
        if let Some((loc, span, text)) = &abstract_ {
            if let Some(span) = *span
                && span > 1
            {
                out.push((
                    "abstract-length",
                    loc.clone(),
                    format!("реферат займає {span} с., рекомендовано одну"),
                ));
            }
            let actual = |what: &str| match what {
                "pages" => pages,
                "figures" => counts.images,
                "tables" => counts.tables,
                "appendices" => counts.appendices,
                _ => counts.sources,
            };
            for (what, regex) in COUNTS.iter() {
                let Some(c) = regex.captures(text) else {
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
                    out.push((
                        "abstract-counts",
                        loc.clone(),
                        format!("у рефераті «{}», а у звіті {name}: {real}", c[0].trim()),
                    ));
                }
            }
        }
        // Зміст: потрібен за двох розділів (або розділу й додатка) і ≥ 10 с.
        let chapters = counts.chapters;
        let has_contents = self
            .printed()
            .any(|(_, p)| structural(&p.body).map(|(n, _)| n) == Some("зміст"));
        if !has_contents && pages >= 10 && (chapters >= 2 || chapters >= 1 && counts.appendices > 0)
        {
            out.push((
                "contents-missing",
                self.code.anchor(),
                format!("у звіті {chapters} розділ(и), {pages} с., а змісту немає"),
            ));
        }
        for (code, loc, msg) in out {
            self.add(code, Some(loc), msg, Vec::new());
        }
    }

    /// Порядок номерів джерел (5.5.1), як їх надруковано: для `@ключ` з
    /// `bibliography` номери дає стиль. Набрані вручну «[N]» перевіряє
    /// статична частина правила.
    fn printed_citations(&mut self) {
        let work = self.work;
        let Some(citations) = work.citations.known() else {
            return;
        };
        let mut seen = 0u32;
        for (loc, text) in citations {
            for c in CITE_GROUP.captures_iter(text) {
                if let Some((n, expected)) = cite_out_of_order(&c, &mut seen) {
                    self.add(
                        "citation-order",
                        Some(loc.clone()),
                        format!(
                            "джерело [{n}] надруковано раніше за [{expected}]: джерела в переліку нумерують у порядку першого посилання, а стиль переліку впорядковує їх інакше"
                        ),
                        Vec::new(),
                    );
                    return;
                }
            }
        }
    }
}

/// Місце факту розкладки: елемент, на якому він стоїть.
fn at_loc(at: Option<At>) -> Option<crate::text::Loc> {
    match at? {
        At::Element(loc) => Some(loc),
        _ => None,
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
