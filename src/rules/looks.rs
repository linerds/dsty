//! Оформлення елементів з коду ([`crate::look`]): заголовки, підписи,
//! формули, переліки, виноски, номери сторінок. Зауваження стоїть на правилі,
//! що задає оформлення (`set`, show-правило, рядок шаблону), одне на причину,
//! з кількістю елементів, яких воно стосується.

use std::sync::LazyLock;

use regex::Regex;

use super::{Checker, FixDraft, spacing, structural};
use crate::eval::MM;
use crate::model::Code;
use crate::text::{Loc, fmt_num, mm, quote};
use crate::work::{Align, At, Element, Fact, FigureKind, Heading, Lead, Scope, TextLook};

/// Літери додатків (7.15.1): українська абетка без Ґ, Є, З, І, Ї, Й, О, Ч, Ь
/// або латинська без I та O.
pub const UKR_LETTERS: &str = "АБВГДЕЖИКЛМНПРСТУФХЦШЩЮЯ";
pub const LAT_LETTERS: &str = "ABCDEFGHJKLMNPQRSTUVWXYZ";
pub const FORBIDDEN_LETTERS: &str = "ҐЄЗІЇЙОЧЬ";

/// Позначка нумерованого переліку: «а)», «1)» (латинська «a)» дає окреме зауваження).
pub static ENUM_MARK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([а-яґєіїa]|\d+)\)$").unwrap());
pub static APPENDIX_WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(додаток)\s*([^\s.]+)?").unwrap());
/// Номер заголовка, рисунка: арабські числа через крапку.
static ARABIC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+(\.\d+)*$").unwrap());
/// Номер у додатку: літера, далі числа через крапку.
static LETTERED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[А-ЯҐЄІЇA-Z](\.\d+)*$").unwrap());

/// Номери за порядком для надрукованих `printed` (літера додатка, розділ,
/// номер): наскрізно або в межах розділу (якщо так пронумеровано більшість),
/// у додатках з літерою додатка. `None`: номер у межах розділу, якого не видно.
pub fn expected_numbers(printed: &[(Option<String>, Option<i64>, String)]) -> Vec<Option<String>> {
    let main: Vec<_> = printed.iter().filter(|(a, _, _)| a.is_none()).collect();
    let per_chapter = main.iter().filter(|(_, _, n)| n.contains('.')).count() * 2 > main.len();
    let mut count: std::collections::HashMap<(Option<String>, Option<i64>), i64> =
        Default::default();
    printed
        .iter()
        .map(|(appendix, chapter, _)| {
            let (key, prefix) = match appendix {
                Some(a) => ((Some(a.clone()), None), Some(a.clone())),
                None if per_chapter => ((None, *chapter), chapter.map(|c| c.to_string())),
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

/// Номер елемента для перевірки порядку: (вид, місце, літера додатка,
/// розділ, номер).
type Numbered<'a> = (&'a str, Loc, Option<String>, Option<i64>, String);

/// Літера додатка з його заголовка: «ДОДАТОК Б» чи номер «Б».
fn appendix_letter(h: &Heading) -> Option<String> {
    let look = h.look.as_ref()?;
    if let Some(p) = &look.prefix
        && let Some(l) = APPENDIX_WORD.captures(p).and_then(|c| c.get(2))
    {
        return Some(l.as_str().to_uppercase());
    }
    let n = look.number.known()?.as_ref()?;
    n.split('.')
        .next()
        .map(|l| l.trim().to_uppercase())
        .filter(|l| !l.is_empty())
}

/// Пункт стандарту про нову сторінку для структурного елемента.
pub fn new_page_clause(body: &str) -> &'static str {
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

/// Зауваження до оформлення: (правило, місце, повідомлення, пункт, скільки
/// елементів, з-поміж скількох, іменник).
struct Grouped {
    code: &'static str,
    /// Однакові причини зводяться в одне зауваження.
    key: String,
    loc: Loc,
    msg: String,
    clause: Option<&'static str>,
    count: usize,
    total: usize,
    noun: &'static str,
    fixes: Vec<FixDraft>,
}

struct Collect<'a> {
    code: &'a Code,
    /// Типові значення typst стосуються документа.
    scope: Scope,
    list: Vec<Grouped>,
}

/// Місце зауваження для `at`: (місце, приписка до повідомлення, значення
/// для виправлення); `None` — типове значення у фрагменті, про нього мовчимо.
pub(super) fn place(
    code: &Code,
    scope: Scope,
    at: &At,
) -> Option<(Loc, Option<String>, Option<Loc>)> {
    Some(match at {
        At::Rule(o) => (
            o.loc.clone(),
            o.template
                .as_ref()
                .map(|t| format!(" (так задає шаблон, {t})")),
            o.value.clone(),
        ),
        At::Default {
            target,
            args,
            within,
        } => {
            if scope == Scope::Fragment {
                return None;
            }
            match within {
                None => (
                    code.setting(target, args),
                    Some(" (типове значення typst)".into()),
                    None,
                ),
                Some(w) => (
                    code.setting_in(w, target, args),
                    Some(" (типове оформлення typst)".into()),
                    None,
                ),
            }
        }
        At::Element(loc) => (loc.clone(), None, None),
    })
}

impl Collect<'_> {
    fn add(
        &mut self,
        code: &'static str,
        at: &At,
        msg: String,
        clause: Option<&'static str>,
        count: (usize, &'static str),
        fix: Option<(&str, &str)>,
    ) {
        let key = msg.clone();
        self.add_keyed(key, code, at, msg, clause, count, fix);
    }

    /// Як [`Self::add`], але зауваження з тим самим `key` зводяться в одне
    /// (повідомлення — від першого).
    #[allow(clippy::too_many_arguments)]
    fn add_keyed(
        &mut self,
        key: String,
        code: &'static str,
        at: &At,
        msg: String,
        clause: Option<&'static str>,
        (total, noun): (usize, &'static str),
        fix: Option<(&str, &str)>,
    ) {
        let Some((loc, suffix, value)) = place(self.code, self.scope, at) else {
            return;
        };
        let msg = format!("{msg}{}", suffix.unwrap_or_default());
        if let Some(p) = self
            .list
            .iter_mut()
            .find(|p| p.code == code && p.loc == loc && p.key == key)
        {
            p.count += 1;
            return;
        }
        let fixes = match (value, fix) {
            (Some(loc), Some((title, text))) => vec![FixDraft {
                title: title.into(),
                edits: vec![(loc, text.into())],
            }],
            _ => Vec::new(),
        };
        self.list.push(Grouped {
            code,
            key,
            loc,
            msg,
            clause,
            count: 1,
            total,
            noun,
            fixes,
        });
    }
}

/// Чи є в тексті малі літери.
fn has_lower(s: &str) -> bool {
    s.chars().any(|c| c.is_lowercase())
}

fn is_times(font: &str) -> bool {
    font.eq_ignore_ascii_case("times new roman")
}

impl Checker<'_> {
    /// Оформлення елементів з коду.
    pub(super) fn looks(&mut self) {
        let mut c = Collect {
            code: self.code,
            scope: self.work.scope,
            list: Vec::new(),
        };
        self.headings_look(&mut c);
        self.figures_look(&mut c);
        self.equations_look(&mut c);
        self.numbers_order();
        self.lists_look(&mut c);
        self.footnotes_look(&mut c);
        self.pages_look(&mut c);
        self.contents_look(&mut c);
        self.inlines();
        for p in c.list {
            let msg = if p.total > 1 && p.count < p.total {
                format!("{} ({} {} з {})", p.msg, p.count, p.noun, p.total)
            } else {
                p.msg
            };
            if let Some(f) = self.add(p.code, Some(p.loc), msg, p.fixes) {
                f.clause = p.clause;
            }
        }
    }

    fn headings_look(&mut self, c: &mut Collect) {
        let work = self.work;
        let heads: Vec<&Heading> = work
            .elements
            .iter()
            .filter_map(|e| match e {
                Element::Heading(h) => Some(h),
                _ => None,
            })
            .collect();
        let total = heads.iter().filter(|h| h.look.is_some()).count();
        let styled = heads
            .iter()
            .filter(|h| {
                h.level == 1
                    && !h.appendix
                    && (structural(h.title.trimmed()).is_some() || h.numbered)
                    && h.look.is_some()
            })
            .count();
        let sub = heads
            .iter()
            .filter(|h| h.level >= 2 && h.look.is_some())
            .count();
        let of_level = |level: usize| {
            heads
                .iter()
                .filter(|h| h.level == level && h.look.is_some())
                .count()
        };
        let numbered = heads
            .iter()
            .filter(|h| {
                h.numbered
                    && h.look.is_some()
                    && !(h.level == 1 && (h.appendix || structural(h.title.trimmed()).is_some()))
            })
            .count();
        let appendices = heads.iter().filter(|h| h.level == 1 && h.appendix).count();
        let mut appendix_no = 0;
        let mut latin = None;
        // Кегль і шрифт основного тексту: якщо в заголовках вони ті самі, досить
        // зауваження до основного тексту
        let first = work.first_heading().unwrap_or(0);
        let body: Vec<&TextLook> = work.elements[first..]
            .iter()
            .filter_map(|e| match e {
                Element::Par(p) => Some(&work.looks[p.look].text),
                _ => None,
            })
            .collect();
        for h in &heads {
            let title = h.title.trimmed();
            let shown = quote(title);
            let is_structural = h.level == 1 && structural(title).is_some();
            let is_appendix = h.level == 1 && h.appendix;
            if is_appendix {
                appendix_no += 1;
            }
            // З нової сторінки (структурні елементи й додатки)
            if (is_structural || is_appendix) && h.new_page.is(&false) {
                let clause = if is_appendix {
                    "6.2.2"
                } else {
                    new_page_clause(title)
                };
                let name = if is_appendix {
                    format!("додаток {shown}")
                } else {
                    shown.clone()
                };
                let before = self.out.len();
                self.add(
                    "new-page",
                    Some(h.loc.clone()),
                    format!("{name} має починатися з нової сторінки: перед ним немає розриву сторінки (`#pagebreak()`)"),
                    Vec::new(),
                );
                if let Some(f) = self.out.get_mut(before) {
                    f.clause = Some(clause);
                }
            }
            let Some(look) = &h.look else { continue };
            if let Some(text) = &look.text {
                // 7.1.20: великими літерами, напівжирним
                if h.level == 1 && !is_appendix && (is_structural || h.numbered) {
                    if let Fact::Known(false, why) = &text.upper
                        && has_lower(title)
                    {
                        c.add(
                            "heading-style",
                            &why.at,
                            "заголовки розділів і структурних елементів друкують великими літерами"
                                .into(),
                            None,
                            (styled, "заголовків"),
                            None,
                        );
                    }
                    if let Fact::Known(w, why) = &text.weight
                        && *w < 600
                    {
                        c.add(
                            "heading-style",
                            &why.at,
                            format!("заголовки розділів і структурних елементів надруковано не напівжирним (накреслення {w})"),
                            None,
                            (styled, "заголовків"),
                            None,
                        );
                    }
                }
                // 6.2.2: заголовок додатка малими з першої великої
                if is_appendix
                    && title.chars().filter(|c| c.is_alphabetic()).count() > 1
                    && (text.upper.is(&true) || !has_lower(title))
                {
                    if has_lower(title) {
                        c.add(
                            "heading-style",
                            text.upper.at().expect("відоме"),
                            "заголовки додатків надруковано великими літерами, а друкують малими з першої великої".into(),
                            Some("6.2.2"),
                            (appendices, "додатків"),
                            None,
                        );
                    } else {
                        let before = self.out.len();
                        self.add(
                            "heading-style",
                            Some(h.loc.clone()),
                            format!("заголовок додатка {shown} набрано великими літерами, а друкують малими з першої великої"),
                            Vec::new(),
                        );
                        if let Some(f) = self.out.get_mut(before) {
                            f.clause = Some("6.2.2");
                        }
                    }
                }
                // Кегль і шрифт (7.1.5)
                if let Fact::Known(size, why) = &text.size
                    && !body.iter().any(|b| b.size.is(size))
                    && (size - 14.0).abs() > 0.05
                {
                    c.add(
                        "font-size",
                        &why.at,
                        format!(
                            "заголовки рівня {} набрано кеглем {}, має бути 14",
                            h.level,
                            fmt_num(*size)
                        ),
                        None,
                        (of_level(h.level), "заголовків"),
                        Some(("Замінити на 14pt", "14pt")),
                    );
                }
                if let Fact::Known(font, why) = &text.font
                    && !body.iter().any(|b| b.font.is(font))
                    && !is_times(font)
                {
                    c.add(
                        "font-family",
                        &why.at,
                        format!("заголовки набрано шрифтом «{font}», має бути Times New Roman"),
                        None,
                        (total, "заголовків"),
                        Some(("Замінити на Times New Roman", "\"Times New Roman\"")),
                    );
                }
                // 7.1.23: переноси
                if let Fact::Known(true, why) = &text.hyphenate {
                    c.add(
                        "heading-hyphen",
                        &why.at,
                        "у заголовках увімкнено переноси слів: задайте `show heading: set text(hyphenate: false)`".into(),
                        None,
                        (total, "заголовків"),
                        None,
                    );
                }
            }
            // 7.1.21: підзаголовки з абзацного відступу
            if h.level >= 2
                && let Fact::Known(align, why) = &look.align
            {
                let indent = h.par_indent.known().copied().unwrap_or(0.0);
                if *align != Align::Left {
                    c.add(
                        "heading-indent",
                        &why.at,
                        "заголовки підрозділів і пунктів вирівняно посередині, а друкують з абзацного відступу".into(),
                        None,
                        (sub, "заголовків"),
                        None,
                    );
                } else if let Some(&off) = look.offset.known()
                    && indent > 0.0
                    && (off - indent).abs() > 2.5
                {
                    c.add(
                        "heading-indent",
                        &why.at,
                        format!(
                            "заголовки рівня {} починаються за {} від берега, а друкують з абзацного відступу ({})",
                            h.level,
                            mm(off),
                            mm(indent)
                        ),
                        None,
                        (sub, "заголовків"),
                        None,
                    );
                }
            }
            // 7.4: формат номера
            if h.numbered
                && !is_structural
                && !is_appendix
                && let Fact::Known(Some(number), why) = &look.number
            {
                let number = number.trim();
                if number.ends_with('.') && number.len() > 1 {
                    c.add_keyed(
                        "period".into(),
                        "heading-number-format",
                        &why.at,
                        format!("після номера заголовка крапку не ставлять: «{number}»"),
                        None,
                        (numbered, "заголовків"),
                        None,
                    );
                } else if h.appendix && !LETTERED.is_match(number) {
                    c.add_keyed(
                        "letter".into(),
                        "heading-number-format",
                        &why.at,
                        format!("заголовок {shown} у додатку має номер «{number}», а номери в додатках — з літерою додатка: «А.1»"),
                        Some("7.15.2"),
                        (numbered, "заголовків"),
                        None,
                    );
                } else if !h.appendix && !ARABIC.is_match(number) {
                    c.add_keyed(
                        "digits".into(),
                        "heading-number-format",
                        &why.at,
                        format!("заголовок {shown} має номер «{number}», а номери розділів і підрозділів — арабськими цифрами через крапку"),
                        None,
                        (numbered, "заголовків"),
                        None,
                    );
                }
            }
            // 6.2.2, 7.15.1: «ДОДАТОК А» над заголовком додатка
            if is_appendix && let Some(prefix) = &look.prefix {
                let printed = format!("{prefix} {title}");
                let upper = look.text.as_ref().and_then(|t| t.upper.known().copied());
                let Some(cap) = APPENDIX_WORD.captures(&printed) else {
                    self.add(
                        "appendix-letter",
                        Some(h.loc.clone()),
                        "над заголовком додатка немає слова «ДОДАТОК» з літерою".into(),
                        Vec::new(),
                    );
                    continue;
                };
                let word = cap.get(1).unwrap().as_str();
                if word != word.to_uppercase() && upper != Some(true) {
                    self.add(
                        "appendix-letter",
                        Some(h.loc.clone()),
                        format!("слово «{word}» над заголовком додатка друкують великими літерами: «ДОДАТОК»"),
                        Vec::new(),
                    );
                }
                let Some(letter) = cap.get(2).map(|m| m.as_str().to_uppercase()) else {
                    self.add(
                        "appendix-letter",
                        Some(h.loc.clone()),
                        "після слова «ДОДАТОК» немає літери додатка".into(),
                        Vec::new(),
                    );
                    continue;
                };
                let is_latin = letter.chars().all(|ch| ch.is_ascii_alphabetic());
                let use_latin = *latin.get_or_insert(is_latin);
                let expected = if use_latin {
                    LAT_LETTERS.chars().nth(appendix_no - 1)
                } else {
                    UKR_LETTERS.chars().nth(appendix_no - 1)
                };
                let one = letter.chars().count() == 1;
                if one && FORBIDDEN_LETTERS.contains(letter.as_str())
                    || letter == "I"
                    || letter == "O"
                {
                    self.add(
                        "appendix-letter",
                        Some(h.loc.clone()),
                        format!("додаток позначено літерою «{letter}», яку не використовують"),
                        Vec::new(),
                    );
                } else if expected.is_some_and(|e| letter != e.to_string())
                    && !letter.chars().all(|ch| ch.is_ascii_digit())
                {
                    self.add(
                        "appendix-letter",
                        Some(h.loc.clone()),
                        format!(
                            "додаток позначено «{letter}», а за порядком має бути «{}»",
                            expected.unwrap()
                        ),
                        Vec::new(),
                    );
                }
            }
        }
    }

    fn figures_look(&mut self, c: &mut Collect) {
        let figs: Vec<_> = self
            .work
            .elements
            .iter()
            .filter_map(|e| match e {
                Element::Figure(f) => Some(f),
                _ => None,
            })
            .collect();
        let captioned = figs.iter().filter(|f| f.caption.is_some()).count();
        for f in &figs {
            let look = &f.look;
            let is_table = f.kind == FigureKind::Table;
            let (word, name, names) = if is_table {
                ("Таблиця", "таблиця", "таблиць")
            } else {
                ("Рисунок", "рисунок", "рисунків")
            };
            let same = figs.iter().filter(|g| g.kind == f.kind).count();
            if look.numbering.is(&None) {
                continue;
            }
            // 7.5.1, 7.6.1: «Рисунок», «Таблиця»
            match &f.caption {
                None => {
                    if look.supplement.known().is_some() {
                        let named = name.to_owned();
                        self.add(
                            "caption-supplement",
                            Some(f.loc.clone()),
                            format!("{named} без підпису «{word} N»"),
                            Vec::new(),
                        );
                    }
                }
                Some(_) => {
                    if let Fact::Known(s, why) = &look.supplement
                        && !s.trim().starts_with(word)
                    {
                        c.add(
                            "caption-supplement",
                            &why.at,
                            format!(
                                "підписи {names} починаються з «{}», а мають зі слова «{word}»",
                                s.split_whitespace().next().unwrap_or(s)
                            ),
                            None,
                            (same, names),
                            None,
                        );
                    }
                    // 7.5.7, 7.6.5: тире між номером і назвою
                    if let Fact::Known(sep, why) = &look.separator {
                        let sep = sep.trim();
                        if !matches!(sep, "–" | "—") {
                            let what = match sep {
                                ":" => "двокрапка",
                                "." => "крапка",
                                "-" | "‐" | "−" => "дефіс",
                                "" => "лише пробіл",
                                _ => "інший знак",
                            };
                            c.add_keyed(
                                format!("sep {sep}"),
                                "caption-separator",
                                &why.at,
                                format!(
                                    "у підписах між номером і назвою {what} «{sep}», а ставлять тире: «Рисунок 1 – Назва»; задайте `set figure.caption(separator: [ -- ])`"
                                ),
                                None,
                                (captioned, "підписів"),
                                None,
                            );
                        }
                    }
                }
            }
            // 7.5.9, 7.6.8: назва таблиці над нею, рисунка під ним
            if f.caption.is_some()
                && let Fact::Known(top, why) = &look.top
                && *top != is_table
            {
                let msg = if is_table {
                    "назви таблиць розміщено під таблицями, а розміщують над ними: `show figure.where(kind: table): set figure.caption(position: top)`"
                } else {
                    "назви рисунків розміщено над рисунками, а розміщують під ними"
                };
                c.add(
                    "caption-position",
                    &why.at,
                    msg.into(),
                    None,
                    (same, names),
                    None,
                );
            }
            // 7.5.6, 7.6.4, 7.15.3: номер арабськими цифрами без крапки, у
            // додатках з літерою
            if let Fact::Known(Some(first), why) = &look.numbering {
                let first = first.trim();
                let from = || &why.at;
                let noun = if is_table {
                    "таблиць"
                } else {
                    "рисунків"
                };
                if first.ends_with('.') && first.len() > 1 {
                    c.add(
                        "object-numbering",
                        from(),
                        format!("після номера {noun} («{first}») крапку не ставлять"),
                        None,
                        (same, noun),
                        None,
                    );
                } else if f.appendix && !LETTERED.is_match(first) {
                    c.add(
                        "object-numbering",
                        from(),
                        format!("{} у додатках нумеровано «{first}», а нумерують з літерою додатка: «А.1»", if is_table { "таблиці" } else { "рисунки" }),
                        Some("7.15.3"),
                        (same, noun),
                        None,
                    );
                } else if !f.appendix && !ARABIC.is_match(first) {
                    c.add(
                        "object-numbering",
                        from(),
                        format!(
                            "{} нумеровано «{first}», а нумерують арабськими цифрами",
                            if is_table {
                                "таблиці"
                            } else {
                                "рисунки"
                            }
                        ),
                        None,
                        (same, noun),
                        None,
                    );
                }
            }
        }
    }

    /// 7.5.6–7.5.8, 7.6.4–7.6.6, 7.10.3–7.10.5, 7.15.3: порядок номерів
    /// рисунків, таблиць і формул, як їх надрукує typst.
    fn numbers_order(&mut self) {
        // (вид, місце, літера додатка, розділ, номер)
        let mut all: Vec<Numbered> = Vec::new();
        let mut letter: Option<Option<String>> = None;
        let mut unknown = std::collections::HashSet::new();
        for e in &self.work.elements {
            let (kind, loc, chapter, number, numbering) = match e {
                Element::Heading(h) if h.level == 1 => {
                    letter = h.appendix.then(|| appendix_letter(h));
                    continue;
                }
                Element::Figure(f) => (
                    if f.kind == FigureKind::Table {
                        "table"
                    } else {
                        "image"
                    },
                    f.caption
                        .as_ref()
                        .and_then(|c| c.whole())
                        .unwrap_or_else(|| f.loc.clone()),
                    f.chapter,
                    f.number.known().cloned(),
                    &f.look.numbering,
                ),
                Element::Equation(q) => (
                    "equation",
                    q.loc.clone(),
                    q.chapter,
                    q.number
                        .known()
                        .map(|n| n.trim_start_matches('(').trim_end_matches(')').to_owned()),
                    &q.look.numbering,
                ),
                _ => continue,
            };
            match number {
                Some(n) => all.push((kind, loc, letter.clone().flatten(), chapter, n)),
                None if !numbering.is(&None) => {
                    unknown.insert(kind);
                }
                None => {}
            }
        }
        for kind in ["image", "table", "equation"] {
            // Без усіх номерів порядок не перевірити
            if unknown.contains(kind) {
                continue;
            }
            let list: Vec<_> = all.iter().filter(|e| e.0 == kind).collect();
            let printed: Vec<_> = list
                .iter()
                .map(|e| (e.2.clone(), e.3, e.4.clone()))
                .collect();
            let (code, name) = match kind {
                "image" => ("object-numbering", "рисунка"),
                "table" => ("object-numbering", "таблиці"),
                _ => ("formula-numbering", "формули"),
            };
            for (e, expected) in list.iter().zip(expected_numbers(&printed)) {
                let printed = &e.4;
                let Some(expected) = expected else { continue };
                if printed.ends_with('.') || *printed == expected {
                    continue;
                }
                let msg = if kind == "equation" {
                    format!("номер формули «({printed})», за порядком має бути «({expected})»")
                } else {
                    format!("номер {name} «{printed}», за порядком має бути «{expected}»")
                };
                self.add(code, Some(e.1.clone()), msg, Vec::new());
            }
        }
    }

    fn equations_look(&mut self, c: &mut Collect) {
        let work = self.work;
        let eqs: Vec<_> = work
            .elements
            .iter()
            .filter_map(|e| match e {
                Element::Equation(q) => Some(q),
                _ => None,
            })
            .collect();
        let total = eqs.len();
        let mut unreferenced = Vec::new();
        let mut numbered = 0;
        for q in &eqs {
            let look = &q.look;
            if let Fact::Known(Some(first), why) = &look.numbering {
                numbered += 1;
                let first = first.trim();
                if !first.contains('(') {
                    c.add(
                        "formula-numbering",
                        &why.at,
                        format!(
                            "формули нумеровано «{first}» без круглих дужок, має бути «({first})»"
                        ),
                        None,
                        (total, "формул"),
                        None,
                    );
                } else if q.appendix
                    && !first
                        .trim_start_matches('(')
                        .starts_with(|ch: char| ch.is_alphabetic())
                {
                    c.add(
                        "formula-numbering",
                        &why.at,
                        format!("формули в додатках нумеровано «{first}», а нумерують з літерою додатка: «(А.1)»"),
                        Some("7.15.3"),
                        (total, "формул"),
                        None,
                    );
                }
                if let Fact::Known(a, why) = &look.number_align
                    && (a.contains("left") || a.contains("start"))
                {
                    c.add(
                        "formula-numbering",
                        &why.at,
                        "номери формул розміщено ліворуч, а друкують у крайньому правому положенні"
                            .into(),
                        None,
                        (total, "формул"),
                        None,
                    );
                }
                // Нумерують лише формули з посиланнями (7.10.2): лише коли
                // show-правил формули немає (вони могли б прибрати номер)
                if !q.referenced && look.align.is_some() {
                    unreferenced.push(q.loc.clone());
                }
            }
            if let Some(a) = &look.align
                && a != "center"
            {
                c.add(
                    "formula-center",
                    &At::default_in("math.equation", "align", &[]),
                    format!("формули вирівняно «{a}», а подають посередині рядка"),
                    None,
                    (total, "формул"),
                    None,
                );
            }
        }
        if unreferenced.len() > 5 && unreferenced.len() * 2 > numbered {
            let loc = self.code.setting("math.equation", &["numbering"]);
            self.add(
                "formula-numbering",
                Some(loc),
                format!(
                    "пронумеровано {numbered} формул, з них {} без посилань у тексті: нумерують лише формули, на які посилаються",
                    unreferenced.len()
                ),
                Vec::new(),
            );
        } else {
            for loc in unreferenced {
                self.add(
                    "formula-numbering",
                    Some(loc),
                    "формулу без посилання в тексті не нумерують".into(),
                    Vec::new(),
                );
            }
        }
        // 7.10.6: «де …» під формулою без абзацного відступу
        for e in &work.elements {
            let Element::Par(p) = e else { continue };
            let head = p.text.trimmed();
            if !p.after_equation || !(head == "де" || head.starts_with("де ")) {
                continue;
            }
            let look = &work.looks[p.look];
            // Відступ першого рядка: за `all: false` після формули його немає
            let indent = if look.indent_all {
                look.indent.known().copied()
            } else {
                Some(0.0)
            };
            let msg = match (&p.lead, indent) {
                (Some(Lead::Break), _) => Some(
                    "`\\` перед «де» лишає порожній рядок між формулою й поясненням, а пояснення подають безпосередньо під формулою: приберіть `\\`, а відступ зніміть так: `#par(first-line-indent: 0pt)[де …]`".to_owned(),
                ),
                (Some(Lead::Space(Some(h))), Some(d)) => {
                    let at = d + h;
                    if at > 1.0 {
                        Some(format!(
                            "рядок «де …» під формулою надруковано з відступом {}, а пояснення познак починають від лівого берега: `#par(first-line-indent: 0pt)[де …]`",
                            mm(at)
                        ))
                    } else if at < -1.0 {
                        Some(format!(
                            "рядок «де …» під формулою виступає за лівий берег на {}: пояснення познак починають від лівого берега, оберніть його в `#par(first-line-indent: 0pt)[де …]`",
                            mm(-at)
                        ))
                    } else {
                        None
                    }
                }
                (Some(Lead::Space(None)), _) => None,
                (None, Some(d)) if d > 1.0 => Some(format!(
                    "рядок «де …» під формулою надруковано з абзацним відступом {}, а пояснення познак починають від лівого берега: оберніть його в `#par(first-line-indent: 0pt)[де …]`",
                    mm(d)
                )),
                _ => None,
            };
            if let Some(msg) = msg {
                self.add("formula-where-indent", p.text.whole(), msg, Vec::new());
            }
        }
    }

    /// Текст у рядку з власними налаштуваннями (`#text(…)[…]`, `_…_`) і
    /// примітки: кегль, шрифт, накреслення (7.1.5, 7.8.3).
    pub(super) fn inlines(&mut self) {
        let work = self.work;
        // До першого заголовка — титульний аркуш: там свої кеглі
        let body = work.first_heading().unwrap_or(usize::MAX);
        for i in &work.runs {
            if i.element <= body {
                continue;
            }
            let (style, par) = (&work.looks[i.look].text, &work.looks[i.par].text);
            let shown = quote(&i.text);
            if i.emph || style.italic.is(&true) && !par.italic.is(&true) {
                self.add(
                    "font-style",
                    Some(i.loc.clone()),
                    format!("текст {shown} надруковано курсивом, а друкують прямим накресленням"),
                    Vec::new(),
                );
            } else if style.black.is(&false) && !par.black.is(&false) {
                self.add(
                    "font-style",
                    Some(i.loc.clone()),
                    format!("текст {shown} надруковано не чорним кольором"),
                    Vec::new(),
                );
            }
            if i.emph {
                continue;
            }
            let note = i.text.trim_start().starts_with("Примітк");
            if let Some(&size) = style.size.known()
                && !note
                && style.size.known() != par.size.known()
                && (size - 14.0).abs() > 0.05
            {
                self.add(
                    "font-size",
                    Some(i.loc.clone()),
                    format!(
                        "текст {shown} набрано кеглем {}, має бути 14",
                        fmt_num(size)
                    ),
                    Vec::new(),
                );
            }
            if let Some(font) = style.font.known()
                && par.font.known() != Some(font)
                && !is_times(font)
            {
                self.add(
                    "font-family",
                    Some(i.loc.clone()),
                    format!("текст {shown} набрано шрифтом «{font}», має бути Times New Roman"),
                    Vec::new(),
                );
            }
        }
        // 7.8.3: примітку друкують кеглем 12 через один інтервал
        for (k, e) in work.elements.iter().enumerate() {
            let Element::Par(p) = e else { continue };
            if k <= body || !p.text.trimmed().starts_with("Примітк") {
                continue;
            }
            let size = work.looks[p.first_look.unwrap_or(p.look)]
                .text
                .size
                .known()
                .copied();
            let em = work.looks[p.look].text.leading.known().copied();
            let single = em.map(|em| !(spacing(em, 0.6) > 1.25 || spacing(em, 0.75) < 0.85));
            if size.is_some_and(|s| (s - 12.0).abs() > 0.05) || single == Some(false) {
                let what = match (size, em) {
                    (Some(s), Some(em)) => format!(
                        "кеглем {} через {} інтервалу",
                        fmt_num(s),
                        fmt_num(spacing(em, 0.662))
                    ),
                    (Some(s), None) => format!("кеглем {}", fmt_num(s)),
                    (None, Some(em)) => format!("через {} інтервалу", fmt_num(spacing(em, 0.662))),
                    (None, None) => continue,
                };
                self.add(
                    "note-size",
                    p.text.whole(),
                    format!("примітку набрано {what}, має бути кеглем 12 через один"),
                    Vec::new(),
                );
            }
        }
    }

    fn lists_look(&mut self, c: &mut Collect) {
        let work = self.work;
        let lists: Vec<_> = work
            .elements
            .iter()
            .filter_map(|e| match e {
                Element::List(l) if l.section != Some(super::BIBLIOGRAPHY) => Some(l),
                _ => None,
            })
            .collect();
        let bullets = lists.iter().filter(|l| !l.enumerated).count();
        let enums = lists.len() - bullets;
        let dash = ["–", "—", "-", "−", "‒"];
        for l in &lists {
            if let Fact::Known(m, why) = &l.marker {
                if !l.enumerated && !dash.contains(&m.as_str()) {
                    c.add(
                        "list-marker",
                        &why.at,
                        format!("позиції переліків позначено «{m}», а позначають знаком «тире» (–): `set list(marker: [--])`"),
                        None,
                        (bullets, "переліків"),
                        None,
                    );
                }
                if l.enumerated {
                    let latin = m.starts_with(|ch: char| ch.is_ascii_lowercase());
                    let msg = if !ENUM_MARK.is_match(m) || latin && m != "a)" {
                        Some(format!(
                            "позиції нумерованих переліків позначено «{m}», а позначають «а)» або «1)»"
                        ))
                    } else if m == "a)" {
                        Some("позиції переліків позначено латинськими літерами «a)», а позначають літерами української абетки «а)»".to_owned())
                    } else {
                        None
                    };
                    if let Some(msg) = msg {
                        c.add(
                            "list-marker",
                            &why.at,
                            msg,
                            None,
                            (enums, "переліків"),
                            None,
                        );
                    }
                }
            }
            // 7.7.4: з абзацного відступу
            if !l.nested
                && let Some(&par) = work.looks[l.look].indent.known()
                && par > 0.0
                && let Fact::Known(at, why) = &l.indent
                && (at - par).abs() > 5.0
            {
                c.add(
                    "list-indent",
                    &why.at,
                    format!(
                        "позиції переліку починаються за {} від берега, а абзацний відступ — {}",
                        mm(*at),
                        mm(par)
                    ),
                    None,
                    (lists.len(), "переліків"),
                    None,
                );
            }
        }
    }

    fn footnotes_look(&mut self, c: &mut Collect) {
        let notes = &self.work.footnotes;
        let Some(look) = notes.look.as_ref().filter(|_| !notes.at.is_empty()) else {
            return;
        };
        if let Fact::Known(m, why) = &look.marker
            && m != "1)"
            && !m.chars().all(|ch| ch == '*')
        {
            c.add(
                "footnote-numbering",
                &why.at,
                format!(
                    "виноски позначено «{m}», а позначають «1)»: `set footnote(numbering: \"1)\")`"
                ),
                None,
                (0, ""),
                None,
            );
        }
        if let Fact::Known(len, why) = &look.separator {
            let len = *len;
            if !(30.0 * MM - 0.5..=40.0 * MM + 0.5).contains(&len) {
                let msg = if len == 0.0 {
                    "над виносками немає лінії, а відокремлюють лінією завдовжки 30–40 мм"
                        .to_owned()
                } else {
                    format!(
                        "лінія над виносками завдовжки {}, має бути 30–40 мм: `set footnote.entry(separator: line(length: 35mm))`",
                        mm(len)
                    )
                };
                c.add("footnote-separator", &why.at, msg, None, (0, ""), None);
            }
        }
        if let Some(text) = &look.text {
            if let Fact::Known(size, why) = &text.size
                && (size - 12.0).abs() > 0.05
            {
                c.add(
                    "font-size",
                    &why.at,
                    format!(
                        "текст виносок набрано кеглем {}, має бути 12",
                        fmt_num(*size)
                    ),
                    Some("7.9.6"),
                    (0, ""),
                    None,
                );
            }
            if let Fact::Known(em, why) = &text.leading
                && (spacing(*em, 0.6) > 1.25 || spacing(*em, 0.75) < 0.85)
            {
                let em = *em;
                c.add(
                    "line-spacing",
                    &why.at,
                    format!(
                        "текст виносок набрано приблизно через {} інтервалу (leading {} em), має бути через один",
                        fmt_num(spacing(em, 0.662)),
                        fmt_num(em)
                    ),
                    Some("7.9.6"),
                    (0, ""),
                    None,
                );
            }
        }
    }

    fn pages_look(&mut self, c: &mut Collect) {
        let work = self.work;
        // Лічильник сторінок змінено вручну: нумерація вже не наскрізна
        for loc in &work.page_updates {
            self.add(
                "page-number",
                Some(loc.clone()),
                "лічильник сторінок змінено (`counter(page).update`), а сторінки нумерують наскрізно, враховуючи титульний аркуш".into(),
                Vec::new(),
            );
        }
        let Some(look) = &work.pages else { return };
        // Лічильник виносок має починатися з 1 на кожній сторінці (7.9.2)
        if look.footnote_reset == Some(false) && work.footnotes.at.len() >= 2 {
            c.add(
                "footnote-numbering",
                &At::default("page", &["background"]),
                "лічильник виносок не скидається на кожній сторінці, а виноски нумерують у межах сторінки: `set page(background: counter(footnote).update(0))`".into(),
                None,
                (0, ""),
                None,
            );
        }
        let body_at = look.body.at();
        match look.body.known() {
            Some(None) => c.add(
                "page-number",
                body_at.unwrap(),
                "сторінки не пронумеровано: `set page(numbering: \"1\", number-align: top + right)`".into(),
                None,
                (0, ""),
                None,
            ),
            Some(Some(n)) if !n.chars().all(|ch| ch.is_ascii_digit()) => c.add(
                "page-number",
                body_at.unwrap(),
                format!("номер сторінки надруковано «{n}», а друкують арабськими цифрами без крапки"),
                None,
                (0, ""),
                None,
            ),
            Some(Some(n)) => {
                if look.first_page > 2 {
                    c.add(
                        "page-number",
                        body_at.unwrap(),
                        format!(
                            "сторінки 2–{} без номера, а номер не ставлять лише на титульному аркуші",
                            look.first_page - 1
                        ),
                        None,
                        (0, ""),
                        None,
                    );
                }
                if n.parse::<i64>().is_ok_and(|n| n != look.first_page) {
                    c.add(
                        "page-number",
                        body_at.unwrap(),
                        format!(
                            "на с. {} надруковано номер {n}: сторінки нумерують наскрізно, враховуючи титульний аркуш",
                            look.first_page
                        ),
                        None,
                        (0, ""),
                        None,
                    );
                }
            }
            None => {}
        }
        if matches!(look.body.known(), Some(Some(_)))
            && let Fact::Known(a, why) = &look.align
            && !((a.contains("right") || a.contains("end")) && a.contains("top"))
        {
            c.add(
                "page-number",
                &why.at,
                format!(
                    "номер сторінки розміщено «{}», а ставлять праворуч у верхньому куті",
                    a.replace('+', " + ")
                ),
                None,
                (0, ""),
                None,
            );
        }
        if let Some(Fact::Known(Some(n), why)) = &look.title {
            c.add(
                "page-number",
                &why.at,
                format!("на титульному аркуші проставлено номер «{n}», а його там не ставлять"),
                Some("7.3.3"),
                (0, ""),
                None,
            );
        }
    }

    fn contents_look(&mut self, c: &mut Collect) {
        let Some(text) = &self.work.contents else {
            return;
        };
        if let Fact::Known(true, why) = &text.hyphenate {
            c.add(
                "contents-hyphen",
                &why.at,
                "у змісті увімкнено переноси слів: `show outline.entry: set text(hyphenate: false)`".into(),
                None,
                (0, ""),
                None,
            );
        }
    }
}

/// Курсив і колір основного тексту (7.1.5): для правила `font-style`.
pub fn body_style_issue(text: &TextLook) -> Option<(String, &At)> {
    if let Fact::Known(true, why) = &text.italic {
        return Some((
            "основний текст набрано курсивом, а друкують прямим накресленням".into(),
            &why.at,
        ));
    }
    if let Fact::Known(false, why) = &text.black {
        return Some(("основний текст набрано не чорним кольором".into(), &why.at));
    }
    None
}
