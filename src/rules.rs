//! Правила ДСТУ 3008:2015 над моделлю документа.
//!
//! Статичні правила перевіряють те, що видно з вихідного коду. Правила з
//! позначкою `full` потребують компіляції (сторінки, стилі, відрендерені
//! номери) і працюють лише в повній перевірці ([`crate::full`]).

use std::sync::LazyLock;

use regex::Regex;

use crate::eval::MM;
use crate::model::{Block, Doc, FigureKind, Heading, Label};
use crate::style::Origin;
use crate::text::{Case, Loc, Text, ends_with_period, first_case, fmt_num, mm, quote, tail};

/// Рівень зауваження в редакторі.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Hint,
    Info,
    Warning,
    Error,
}

impl Severity {
    pub fn name(self) -> &'static str {
        match self {
            Severity::Hint => "hint",
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Rule {
    pub code: &'static str,
    /// Пункт стандарту.
    pub clause: &'static str,
    pub desc: &'static str,
    /// Рекомендація чи перевірка з можливими хибними спрацюваннями: лише через `enable`.
    pub optional: bool,
    /// Працює лише в повній перевірці (компіляції нативним `typst`).
    pub full: bool,
    /// Проби повної перевірки, потрібні правилу ([`probe`]): з них збирається
    /// обгортка. Буває й у статичного правила з повною частиною.
    pub probes: u8,
    /// Статичне правило з повною частиною: без компіляції бачить не все.
    pub partial: bool,
    /// Статична й повна частини перевіряють різне (основний текст і
    /// заголовки), тож зауваження в одному рядку не дублюють одне одне.
    pub split: bool,
    /// Рекомендація стандарту («рекомендовано»), а не вимога: у редакторі це
    /// інформація, а не попередження.
    pub advice: bool,
    /// Типовий рівень зауважень (налаштовується `severity`).
    pub severity: Severity,
}

/// Секції проб обгортки повної перевірки (`probe.typ`).
pub mod probe {
    /// Прогони тексту: шрифт, кегль, накреслення, надруковані номери.
    pub const TEXT: u8 = 1;
    /// Абзаци: кегль, відступ, інтервал.
    pub const PAR: u8 = 2;
    /// Межі таблиць і їхні головки.
    pub const TABLE: u8 = 4;
    /// Вирівнювання виокремлених формул.
    pub const EQUATION: u8 = 8;
    /// Кожен прогін основного тексту: відхилення посеред абзацу. Найдорожча
    /// проба, тож лише для необов'язкового правила.
    pub const RUNS: u8 = 16;
}

use probe::{EQUATION, PAR, RUNS, TABLE, TEXT};

const fn rule(code: &'static str, clause: &'static str, desc: &'static str) -> Rule {
    Rule {
        code,
        clause,
        desc,
        optional: false,
        full: false,
        probes: 0,
        partial: false,
        split: false,
        advice: false,
        severity: Severity::Warning,
    }
}

impl Rule {
    const fn optional(mut self) -> Self {
        self.optional = true;
        self
    }

    /// Правило повної перевірки з пробами `probes` (0, якщо вистачає сторінок і
    /// позицій елементів).
    const fn full(mut self, probes: u8) -> Self {
        self.full = true;
        self.probes = probes;
        self
    }

    /// Статичне правило, що має й повну частину.
    const fn probes(mut self, probes: u8) -> Self {
        self.probes = probes;
        self.partial = true;
        self
    }

    /// Статичне правило, повна частина якого перевіряє інше ([`Rule::split`]).
    const fn split(mut self, probes: u8) -> Self {
        self = self.probes(probes);
        self.split = true;
        self
    }

    const fn advice(mut self) -> Self {
        self.advice = true;
        self.severity = Severity::Info;
        self
    }

    const fn hint(mut self) -> Self {
        self.severity = Severity::Hint;
        self
    }

    /// Опис правила з текстом пунктів стандарту (Markdown): для підказки в
    /// редакторі й `dsty explain`.
    pub fn explain(&self) -> String {
        let mut out = format!("**{}** — {}\n\n", self.code, self.desc);
        let marks = self.marks();
        if self.clause.is_empty() {
            out.push_str("Службове правило dsty\n");
        } else {
            out.push_str(&format!(
                "ДСТУ 3008:2015, п. {} · {}\n",
                self.clause,
                marks.join(" · ")
            ));
        }
        for (id, text) in crate::standard::clauses_of(self.clause) {
            out.push_str(&format!("\n> **{id}** {text}\n"));
        }
        out.push_str(&format!(
            "\nВимкнути для рядка: `// dstu-ignore-next-line: {}`",
            self.code
        ));
        out
    }

    /// Позначки правила (Markdown): вид перевірки, чи вмикається окремо,
    /// чи це рекомендація.
    pub fn marks(&self) -> Vec<&'static str> {
        let mut marks = vec![if self.full {
            "повна перевірка (`--full`)"
        } else {
            "статична перевірка"
        }];
        if self.optional {
            marks.push("вмикається через `enable`");
        }
        if self.advice {
            marks.push("рекомендація стандарту");
        }
        marks
    }
}

pub const RULES: &[Rule] = &[
    rule(
        "heading-period",
        "7.1.20, 7.1.21",
        "Заголовки друкують без крапки в кінці",
    ),
    rule(
        "heading-case",
        "7.1.21",
        "Заголовки підрозділів, пунктів і підпунктів друкують з великої літери",
    ),
    rule(
        "heading-numbered",
        "7.1.18",
        "Структурні елементи («Вступ», «Висновки», «Перелік джерел посилання» тощо) не нумерують",
    )
    .probes(TEXT),
    rule(
        "heading-single",
        "7.4.6",
        "Єдиний пункт розділу чи підрозділу (єдиний підпункт пункту) не нумерують",
    ),
    rule(
        "heading-abbrev",
        "7.1.17",
        "У заголовках слова не скорочують («т. д.», «рис.», «напр.»)",
    )
    .optional(),
    rule(
        "structure-name",
        "7.1.18",
        "Структурні елементи мають назви зі стандарту: «Висновки», «Перелік джерел посилання», «Вступ»",
    ),
    rule(
        "figure-unreferenced",
        "7.5.2",
        "Рисунок подають після тексту, де на нього вперше посилаються",
    ),
    rule(
        "table-unreferenced",
        "7.6.3",
        "На кожну таблицю має бути посилання в тексті",
    ),
    rule(
        "figure-before-ref",
        "7.5.2, 7.6.3",
        "Рисунок і таблицю подають після тексту, де їх згадано вперше",
    ),
    rule(
        "caption-case",
        "7.5.9, 7.6.8",
        "Назву рисунка й таблиці друкують з великої літери",
    ),
    rule(
        "table-header-case",
        "7.6.10",
        "Заголовки колонок таблиці починають з великої літери",
    ),
    rule(
        "table-header-period",
        "7.6.11",
        "У кінці заголовків і підзаголовків таблиць крапки не ставлять",
    ),
    rule(
        "list-case",
        "7.7.4",
        "Текст кожної позиції переліку починають з малої літери",
    ),
    rule(
        "list-punct",
        "7.7.2",
        "Позиції переліку закінчують «;», останню — «.» (за прикладом у стандарті)",
    ),
    rule("list-colon", "7.7.1", "Перед переліком ставлять двокрапку"),
    rule(
        "formula-where",
        "7.10.6",
        "Пояснення познак формули починають словом «де» без двокрапки",
    ),
    rule(
        "keywords-count",
        "4.3.8",
        "Перелік ключових слів реферату містить 5–15 слів (словосполучень)",
    ),
    rule(
        "keywords-order",
        "4.3.8",
        "Ключові слова реферату рекомендовано розташовувати за абеткою",
    )
    .optional()
    .advice(),
    rule(
        "abbrev-order",
        "7.14",
        "Скорочення й умовні познаки розташовують за абеткою: спершу українські, потім інші",
    ),
    rule(
        "appendix-unreferenced",
        "6.1.2",
        "Додатки розміщують у порядку посилання на них у тексті",
    ),
    rule(
        "appendix-order",
        "6.1.2",
        "Додатки розміщують у порядку посилання на них у тексті",
    ),
    rule(
        "appendix-case",
        "6.2.2",
        "Заголовок додатка друкують з великої літери",
    ),
    rule(
        "structure-order",
        "3.2.1, 3.3.1, 3.4.1",
        "Структурні елементи йдуть у порядку стандарту: реферат, зміст, …, вступ, суть, висновки, …, додатки",
    ),
    rule(
        "bibliography-missing",
        "3.5.2",
        "Якщо в тексті є посилання на публікації, обов'язковий «Перелік джерел посилання»",
    ),
    rule(
        "bibliography-full",
        "5.5.1",
        "У переліку джерел посилання лише джерела, на які є посилання в тексті",
    ),
    rule(
        "bibliography-style",
        "5.5.1, 7.11.3",
        "На джерела посилаються номерами в квадратних дужках, а не автором і роком",
    ),
    rule(
        "heading-empty",
        "7.1.19",
        "Розділи й підрозділи мають заголовок",
    ),
    rule(
        "note-format",
        "7.8.3, 7.8.4",
        "Примітку починають словом «Примітка.» («Примітка 1.»), її текст — з великої літери",
    ),
    rule(
        "note-numbering",
        "7.8.3, 7.8.4",
        "Одну примітку не нумерують, кілька — нумерують: «Примітка 1.», «Примітка 2.»",
    ),
    rule(
        "structure-required",
        "3.5.1",
        "Обов'язкові структурні елементи: «Реферат», «Вступ», суть звіту, «Висновки»",
    )
    .optional(),
    rule(
        "unit-tolerance",
        "7.10.12.1",
        "Значення з допуском: «(80 ± 2) мм» або «80 мм ± 2 мм», а не «80 ± 2 мм»",
    ),
    rule(
        "unit-space",
        "7.10.7",
        "Між числом і одиницею виміру залишають проміжок",
    )
    .optional(),
    rule(
        "unit-range",
        "7.10.12.2",
        "Діапазон значень наводять зі словами «від» і «до» та одиницею біля кожного числа",
    )
    .optional(),
    rule(
        "unit-dimensions",
        "7.10.12.3",
        "Розміри наводять з одиницею біля кожного числа: 80 мм × 25 мм",
    )
    .optional(),
    rule(
        "ref-supplement",
        "7.11.2",
        "Посилання на рисунок, таблицю, формулу пишуть словом з малої літери в потрібному відмінку: «на рисунку 1.3», «(рис. 10)», «згідно з формулою (3.1)»",
    ),
    rule(
        "citation-order",
        "5.5.1",
        "Джерела в переліку нумерують у порядку першого посилання на них у тексті",
    )
    .probes(TEXT),
    rule(
        "formula-comma",
        "7.10.11",
        "Кілька формул, не відокремлених текстом, розділяють комами",
    ),
    // --- Коментарі `dstu-ignore` ---
    rule(
        "ignore-unknown",
        "",
        "Коментар dstu-ignore називає правило, якого немає (помилка в назві?)",
    ),
    rule(
        "ignore-unused",
        "",
        "Коментар dstu-ignore вимикає правило, яке в цьому рядку не спрацьовує",
    )
    .hint(),
    // --- Повна перевірка: сторінки, стилі, відрендерені номери ---
    rule(
        "figure-far-from-ref",
        "7.5.2, 7.6.3",
        "Рисунок і таблицю подають не далі ніж на наступній сторінці після першого посилання",
    )
    .full(0),
    rule(
        "table-no-header",
        "7.6.9",
        "У кожній частині перенесеної таблиці повторюють її головку",
    )
    .full(TABLE),
    rule(
        "table-continuation",
        "7.6.9",
        "Над перенесеною частиною таблиці пишуть «Продовження таблиці N» («Кінець таблиці N»)",
    )
    .full(TEXT | TABLE),
    rule(
        "footnote-count",
        "7.9.3",
        "На одній сторінці — не більше ніж чотири виноски",
    )
    .full(0),
    rule(
        "footnote-numbering",
        "7.9.2",
        "Виноски позначають «1)» і нумерують у межах кожної сторінки",
    )
    .full(0),
    rule(
        "footnote-separator",
        "7.9.5",
        "Виноску відокремлюють від тексту лінією завдовжки 30–40 мм",
    )
    .full(0),
    rule(
        "abstract-length",
        "4.3.5",
        "Реферат рекомендовано подавати на одній сторінці",
    )
    .full(0)
    .optional()
    .advice(),
    rule(
        "abstract-counts",
        "4.3.3",
        "Обсяг звіту в рефераті (сторінки, рисунки, таблиці, додатки, джерела) відповідає звіту",
    )
    .full(TEXT),
    rule(
        "contents-missing",
        "4.4.1",
        "Зміст складають, якщо є два розділи (або розділ і додаток) і не менше десяти сторінок",
    )
    .full(0)
    .optional(),
    rule(
        "new-page",
        "4.2.1, 4.3.1, 4.4.1, 4.5.2, 4.6.2, 5.1.1, 5.3.1, 5.4.2, 5.5.1, 6.2.2",
        "Структурні елементи й додатки починають з нової сторінки",
    )
    .full(TEXT | PAR),
    rule(
        "heading-orphan",
        "7.1.25",
        "Заголовок не розміщують в останньому рядку сторінки",
    )
    .full(TEXT | PAR),
    rule(
        "heading-style",
        "7.1.20",
        "Заголовки структурних елементів і розділів друкують великими літерами напівжирним",
    )
    .full(TEXT),
    rule(
        "heading-indent",
        "7.1.21",
        "Заголовки підрозділів, пунктів і підпунктів друкують з абзацного відступу",
    )
    .full(TEXT),
    rule(
        "heading-hyphen",
        "7.1.23",
        "Переносити слова в заголовках заборонено",
    )
    .full(TEXT),
    rule(
        "contents-hyphen",
        "4.4.5",
        "Переносити слова у змісті не рекомендовано",
    )
    .full(TEXT)
    .optional()
    .advice(),
    rule(
        "heading-number-format",
        "7.4.1–7.4.5, 7.15.2",
        "Номери розділів і підрозділів — арабськими цифрами через крапку без крапки в кінці; у додатках — з літерою",
    )
    .full(TEXT),
    rule(
        "caption-supplement",
        "7.5.1, 7.6.1",
        "Підпис рисунка починають словом «Рисунок», таблиці — «Таблиця»",
    )
    .full(TEXT),
    rule(
        "object-numbering",
        "7.5.6–7.5.8, 7.6.4–7.6.6, 7.15.3",
        "Рисунки й таблиці нумерують наскрізно або в межах розділу, у додатках — з літерою додатка",
    )
    .full(TEXT),
    rule(
        "formula-numbering",
        "7.10.2–7.10.5",
        "Нумерують лише формули з посиланнями; номер у дужках у крайньому правому положенні, у додатках — з літерою",
    )
    .full(TEXT),
    rule(
        "formula-center",
        "7.10.1",
        "Формули подають посередині рядка",
    )
    .full(EQUATION),
    rule(
        "formula-where-indent",
        "7.10.6",
        "Пояснення познак подають безпосередньо під формулою, з нового рядка без абзацного відступу",
    )
    .full(PAR),
    rule(
        "list-marker",
        "7.7.2",
        "Позиції переліку позначають «–», а переліки з посиланнями — «а)», «1)»",
    )
    .full(TEXT),
    rule(
        "list-indent",
        "7.7.4",
        "Позиції переліку починають з абзацного відступу",
    )
    .full(0)
    .optional(),
    rule(
        "page-size",
        "7.1.6",
        "Звіт друкують на аркушах формату А4 (за потреби — А3)",
    ),
    rule(
        "page-margins",
        "7.1.11",
        "Береги: верхній і нижній — не менше 20 мм, лівий — 25 мм, правий — 10 мм",
    )
    .optional()
    .advice(),
    rule(
        "page-number",
        "7.3.1, 7.3.3",
        "Номер сторінки — праворуч угорі арабськими цифрами без крапки, наскрізно; на титульному аркуші його не ставлять",
    )
    .full(0),
    rule(
        "font-family",
        "7.1.5",
        "Звіт друкують шрифтом Times New Roman",
    )
    .split(TEXT),
    rule(
        "font-missing",
        "7.1.5",
        "Шрифт Times New Roman має бути встановлено, інакше typst набирає запасним",
    )
    .full(0)
    .optional(),
    rule(
        "font-size",
        "7.1.5, 7.9.6",
        "Основний текст і заголовки — кеглем 14, виноски — 12",
    )
    .split(TEXT | PAR),
    rule(
        "font-style",
        "7.1.5",
        "Текст друкують чорним кольором прямого накреслення",
    )
    .full(TEXT | RUNS)
    .optional(),
    rule(
        "line-spacing",
        "7.1.5, 7.9.6",
        "Текст друкують через 1,5–2 міжрядкові інтервали, виноски — через один",
    )
    .split(PAR),
    rule(
        "par-indent",
        "7.1.22",
        "Абзацний відступ однаковий у всьому тексті й дорівнює п'яти знакам",
    ),
    rule(
        "note-size",
        "7.8.3",
        "Примітку друкують кеглем 12 через один міжрядковий інтервал",
    )
    .full(PAR),
    rule(
        "appendix-letter",
        "6.2.2, 7.15.1",
        "Над заголовком додатка — «ДОДАТОК» і літера: А, Б, В… (крім Ґ, Є, З, І, Ї, Й, О, Ч, Ь)",
    )
    .full(TEXT),
    rule(
        "caption-position",
        "7.5.9, 7.6.8",
        "Назву рисунка розміщують під ним посередині рядка, таблиці — над нею з абзацного відступу",
    )
    .full(TEXT)
    .optional(),
    rule(
        "caption-separator",
        "7.5.7, 7.6.5",
        "Номер і назву в підписі розділяють тире: «Рисунок 2.1 — Схема устатковання»",
    )
    .full(TEXT),
];

pub fn find(code: &str) -> Option<&'static Rule> {
    RULES.iter().find(|r| r.code == code)
}

/// Знайдене порушення до перетворення на позиції.
pub struct Found {
    pub rule: &'static Rule,
    pub loc: Loc,
    pub msg: String,
    pub fixes: Vec<FixDraft>,
    /// Пункт стандарту саме цього зауваження, якщо правило охоплює кілька.
    pub clause: Option<&'static str>,
    /// Однакові зауваження в межах одного елемента (переліку): якщо їх
    /// багато, [`crate::lint`] зводить їх в одне.
    pub group: Option<Group>,
    /// Пов'язані місця з поясненням: перше посилання, заголовок-батько.
    pub related: Vec<(Loc, String)>,
    /// Елемент, який варто показати в терміналі навколо зауваження: уся
    /// позиція переліку, обидві формули.
    pub context: Option<Loc>,
}

impl Found {
    pub fn new(rule: &'static Rule, loc: Loc, msg: String, fixes: Vec<FixDraft>) -> Self {
        Found {
            rule,
            loc,
            msg,
            fixes,
            clause: None,
            group: None,
            related: Vec::new(),
            context: None,
        }
    }
}

/// Група однакових зауважень: перелік, у якому кілька позицій мають ту саму
/// ваду.
#[derive(Debug, Clone)]
pub struct Group {
    /// Номер елемента в документі.
    pub id: usize,
    /// Усього позицій в елементі.
    pub total: usize,
    /// Суть для зведеного зауваження: «позицій переліку з великої літери».
    pub summary: &'static str,
    /// Текст цієї позиції для зведеного зауваження.
    pub example: String,
}

pub struct FixDraft {
    pub title: String,
    /// Заміни в одному файлі.
    pub edits: Vec<(Loc, String)>,
}

/// Заголовки структурних елементів, які не нумерують (7.1.18), і їхній ранг у
/// порядку стандарту (3.2.1, 3.3.1, 3.4.1). Нумеровані розділи суті мають ранг
/// 6, додатки 10.
const STRUCTURAL: &[(&str, u8)] = &[
    ("список авторів", 0),
    ("реферат", 1),
    ("зміст", 2),
    ("скорочення та умовні познаки", 3),
    ("передмова", 4),
    ("вступ", 5),
    ("висновки", 7),
    ("рекомендації", 8),
    ("перелік джерел посилання", 9),
];

pub const BIBLIOGRAPHY: &str = "перелік джерел посилання";
pub const ABBREVIATIONS: &str = "скорочення та умовні познаки";
pub const ABSTRACT: &str = "реферат";

/// Поширені назви структурних елементів, яких стандарт не знає (7.1.18), і
/// канонічна назва.
const VARIANTS: &[(&str, &str)] = &[
    ("висновок", "висновки"),
    ("загальні висновки", "висновки"),
    ("рекомендація", "рекомендації"),
    ("введення", "вступ"),
    ("перелік джерел посилань", BIBLIOGRAPHY),
    ("перелік посилань", BIBLIOGRAPHY),
    ("перелік використаних джерел", BIBLIOGRAPHY),
    ("перелік використаної літератури", BIBLIOGRAPHY),
    ("список джерел", BIBLIOGRAPHY),
    ("список джерел посилання", BIBLIOGRAPHY),
    ("список використаних джерел", BIBLIOGRAPHY),
    ("список використаної літератури", BIBLIOGRAPHY),
    ("список літератури", BIBLIOGRAPHY),
    ("використані джерела", BIBLIOGRAPHY),
    ("використана література", BIBLIOGRAPHY),
    ("використані ресурси", BIBLIOGRAPHY),
    ("література", BIBLIOGRAPHY),
    ("бібліографія", BIBLIOGRAPHY),
    ("джерела", BIBLIOGRAPHY),
];

/// Заголовок малими літерами, без крайніх пробілів і крапки в кінці.
fn title_key(title: &str) -> String {
    let title = title.trim();
    title
        .strip_suffix('.')
        .unwrap_or(title)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Назва структурного елемента за заголовком, зокрема неканонічним
/// («Висновок», «Список літератури»). Назву переліку скорочень виконавець
/// добирає сам (4.5.3), тож береться за початком.
pub(crate) fn structural(title: &str) -> Option<(&'static str, u8)> {
    let key = title_key(title);
    let name = VARIANTS
        .iter()
        .find(|(variant, _)| *variant == key)
        .map_or(key.as_str(), |(_, canonical)| canonical);
    if let Some(&found) = STRUCTURAL.iter().find(|(n, _)| *n == name) {
        return Some(found);
    }
    let abbreviations = [
        "скорочення",
        "перелік скорочень",
        "список скорочень",
        "перелік умовних",
        "умовні позначення",
        "умовні познаки",
    ];
    if abbreviations.iter().any(|p| key.starts_with(p)) {
        return Some(STRUCTURAL[3]);
    }
    None
}

/// Канонічна назва для неканонічного заголовка структурного елемента
/// («Висновок» -> «Висновки»), у регістрі оригіналу.
pub(crate) fn canonical_name(title: &str) -> Option<String> {
    let key = title_key(title);
    let (_, canonical) = VARIANTS.iter().find(|(variant, _)| *variant == key)?;
    let letters: String = title.chars().filter(|c| c.is_alphabetic()).collect();
    Some(
        if letters.chars().count() > 1 && letters == letters.to_uppercase() {
            canonical.to_uppercase()
        } else {
            let mut chars = canonical.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        },
    )
}

/// Ключ для порядку за абеткою (7.14, 4.3.8): українська абетка, потім
/// латинська, потім решта; розділові знаки й апостроф не враховуються.
fn collation_key(s: &str) -> Vec<u32> {
    const UKR: &str = "абвгґдеєжзиіїйклмнопрстуфхцчшщьюя";
    s.chars()
        .flat_map(char::to_lowercase)
        .filter_map(|c| {
            if let Some(i) = UKR.chars().position(|u| u == c) {
                Some(1000 + i as u32)
            } else if c.is_ascii_alphabetic() {
                Some(2000 + c as u32)
            } else if c.is_ascii_digit() {
                Some(100 + c as u32)
            } else if c.is_whitespace() || c == '-' {
                Some(1)
            } else if c.is_alphabetic() {
                Some(3000 + c as u32)
            } else {
                None
            }
        })
        .collect()
}

/// Чи слово набрано українською (перша літера з кирилиці).
fn is_cyrillic(s: &str) -> bool {
    s.chars()
        .find(|c| c.is_alphabetic())
        .is_some_and(|c| ('\u{400}'..='\u{4ff}').contains(&c))
}

/// Перше порушення абеткового порядку: (індекс, попередній).
fn out_of_order(keys: &[Vec<u32>]) -> Option<usize> {
    (1..keys.len()).find(|&i| keys[i] < keys[i - 1])
}

/// Регістр першої літери тексту, якщо його варто перевіряти: літера стоїть у
/// коді дослівно (не з `raw`, формули чи виклику) і слово не латиницею
/// (латиницею пишуть назви й ідентифікатори: Windows, Python, `Dorm`).
fn case_of(text: &Text) -> Option<Case> {
    let s = text.trimmed();
    let first = s.chars().next()?;
    if first.is_ascii_alphabetic() || !text.is_exact_at(text.trimmed_range().start) {
        return None;
    }
    first_case(s)
}

/// Стилі `bibliography`, що посилаються автором і роком чи виноскою, а не
/// номером (5.5.1, 7.11.3).
fn is_author_date(style: &str) -> bool {
    let style = style.to_lowercase();
    let style = style.trim_end_matches(".csl");
    [
        "author-date",
        "-note",
        "harvard",
        "alphanumeric",
        "american-psychological",
        "modern-language",
        "modern-humanities",
        "american-anthropological",
        "american-political-science",
        "american-sociological",
        "mohr-siebeck",
        "psychologie",
    ]
    .iter()
    .any(|p| style.contains(p))
        || matches!(style, "apa" | "mla" | "mla8" | "chicago-notes")
}

const UNITS: &str = "мм|см|дм|км|мкм|нм|м|кг|мг|мкс|мс|нс|с|хв|год|ГГц|МГц|кГц|Гц|кВ|мВ|мкВ|В|мА|мкА|А|МОм|кОм|Ом|МВт|кВт|мВт|Вт|кДж|Дж|кН|Н|МПа|кПа|Па|дБ|°C|мл|л|Гбіт|Мбіт|Кбіт|біт|байт|ГБ|МБ|КБ|ТБ|%";
const NUM: &str = r"\d+(?:[,.]\d+)?";

static UNIT_CHECKS: LazyLock<Vec<(&'static str, Regex, &'static str)>> = LazyLock::new(|| {
    let end = format!("(?:{UNITS})(?:[^\\p{{L}}]|$)");
    vec![
        (
            "unit-space",
            Regex::new(&format!(r"\d{end}")).unwrap(),
            "між числом і одиницею немає проміжку: ",
        ),
        (
            "unit-range",
            Regex::new(&format!(
                r"(?:від\s+{NUM}\s+до\s+{NUM}|{NUM}\s*[-–—]\s*{NUM})\s*{end}"
            ))
            .unwrap(),
            "діапазон без «від … до» чи з однією одиницею: ",
        ),
        (
            "unit-tolerance",
            Regex::new(&format!(r"{NUM}\s*±\s*{NUM}\s*{end}")).unwrap(),
            "значення з допуском без дужок: ",
        ),
        (
            "unit-dimensions",
            Regex::new(&format!(
                r"{NUM}\s*[xх×]\s*{NUM}(?:\s*[xх×]\s*{NUM})?\s*{end}"
            ))
            .unwrap(),
            "розміри з однією одиницею на всі числа: ",
        ),
    ]
});

/// `[3]`, `[1, 4]`, `[2–5]`, але не `Keccak-f[1600]` чи `masiv[8]`.
static CITATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[^\p{L}\p{N}_\-])(\[\d+(?:\s*[,–-]\s*\d+)*\])").unwrap());
/// Група номерних посилань: `[3]`, `[1, 4–6]`, `[2]–[5]` (номери в групі 1,
/// кінець діапазону з дужками в групі 2).
pub(crate) static CITE_GROUP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[^\p{L}\p{N}_\-])\[(\d+(?:\s*[,;–—-]\s*\d+)*)\](?:\s*[–—-]\s*\[(\d+)\])?")
        .unwrap()
});
/// Друга половина допуску: `80 ± 2` у «80 ± 2 мм».
static TOLERANCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^{NUM}\s*±\s*{NUM}")).unwrap());
/// Скорочення, яких не вживають у заголовках (7.1.17).
static HEADING_ABBREV: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:^|[^\p{L}])((?:і\s+)?т\.\s*[дпч]|та\s+ін|напр|рис|табл|див|зокр|розд|підрозд|мал|ін|ст|рр)\.",
    )
    .unwrap()
});
/// Роздільник терміна й розшифрування в переліку скорочень.
static TERM_SEPARATOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+[–—-]+\s+|:\s").unwrap());
static WHERE_COLON: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[Дд]е\s*:").unwrap());
/// Початок примітки: слово, номер, знак після них і перша літера тексту.
static NOTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(Примітк[аи]|ПРИМІТК[АИ])(\s+\d+)?(\s*[.:\-–—]?)\s*(\p{L})?").unwrap()
});

pub struct Checker<'a> {
    doc: &'a Doc,
    active: &'a dyn Fn(&str) -> bool,
    out: Vec<Found>,
}

pub fn run(doc: &Doc, active: &dyn Fn(&str) -> bool) -> Vec<Found> {
    let mut checker = Checker {
        doc,
        active,
        out: Vec::new(),
    };
    checker.headings();
    checker.figures();
    checker.lists();
    checker.paragraphs();
    checker.notes();
    checker.keywords();
    checker.abbreviations();
    checker.bibliographies();
    checker.refs();
    checker.citations();
    checker.formulas();
    checker.styles();
    checker.out
}

impl Checker<'_> {
    fn add(&mut self, code: &str, loc: Option<Loc>, msg: String, fixes: Vec<FixDraft>) {
        self.add_in(None, code, loc, msg, fixes);
    }

    fn add_in(
        &mut self,
        group: Option<Group>,
        code: &str,
        loc: Option<Loc>,
        msg: String,
        fixes: Vec<FixDraft>,
    ) {
        if !(self.active)(code) {
            return;
        }
        let (Some(rule), Some(loc)) = (find(code), loc) else {
            return;
        };
        let mut found = Found::new(rule, loc, msg, fixes);
        found.group = group;
        self.out.push(found);
    }

    /// Пов'язане місце для щойно доданого зауваження правила `code`.
    fn relate(&mut self, code: &str, loc: Loc, note: &str) {
        if let Some(last) = self.out.last_mut()
            && last.rule.code == code
        {
            last.related.push((loc, note.into()));
        }
    }

    /// Перше посилання на елемент; `Err`, якщо мітку з коду не дізнатися.
    fn first_ref(&self, label: Option<&Label>) -> Result<Option<usize>, ()> {
        match label {
            None => Ok(None),
            Some(Label::Dynamic) => Err(()),
            Some(Label::Name(name)) => Ok(self
                .doc
                .refs
                .get(name)
                .and_then(|refs| refs.first().copied())),
        }
    }

    fn headings(&mut self) {
        // Нумеровані заголовки, під якими зараз стоїмо: (заголовок, прямі нащадки)
        let mut open: Vec<(&Heading, Vec<&Heading>)> = Vec::new();
        let mut latest: Option<(u8, String, Loc)> = None;
        let mut appendices: Vec<&Heading> = Vec::new();
        let doc = self.doc;

        let mut present: Vec<&str> = Vec::new();
        let mut chapters = false;
        for block in &doc.blocks {
            let Block::Heading(h) = block else { continue };
            let title = h.title.trimmed().to_owned();

            if title.is_empty() && h.level <= 2 && !h.generated {
                self.add(
                    "heading-empty",
                    Some(h.loc.clone()),
                    if h.level == 1 {
                        "розділ без заголовка".into()
                    } else {
                        "підрозділ без заголовка".into()
                    },
                    Vec::new(),
                );
            }
            // Розділ може друкувати шаблон (`pz-lb.with(title: …)`): тоді в коді
            // є лише нумеровані підрозділи
            match structural(&title) {
                Some((name, _)) if h.level == 1 => present.push(name),
                None if h.numbered && !h.appendix => chapters = true,
                _ => {}
            }

            if ends_with_period(&title) {
                let fixes = (!h.generated)
                    .then(|| last_char_fix(&h.title, "Прибрати крапку", ""))
                    .flatten();
                self.add(
                    "heading-period",
                    Some(h.loc.clone()),
                    format!("крапка в кінці заголовка {}", quote(&title)),
                    fixes.into_iter().collect(),
                );
            }
            if !h.generated
                && h.level <= 2
                && let Some(canonical) = canonical_name(&title)
            {
                let range = h.title.trimmed_range();
                let fix = h.title.exact_loc(range).map(|loc| FixDraft {
                    title: format!("Перейменувати на «{canonical}»"),
                    edits: vec![(loc, canonical.clone())],
                });
                self.add(
                    "structure-name",
                    Some(h.loc.clone()),
                    format!(
                        "структурний елемент називають «{canonical}», а не {}",
                        quote(&title)
                    ),
                    fix.into_iter().collect(),
                );
            }
            if !h.generated
                && let Some(m) = HEADING_ABBREV.captures(&title).and_then(|c| c.get(1))
            {
                let start = h.title.trimmed_range().start + m.start();
                self.add(
                    "heading-abbrev",
                    h.title
                        .loc(start..start + m.len() + 1)
                        .or(Some(h.loc.clone())),
                    format!(
                        "скорочення «{}.» у заголовку {}: у заголовках слова не скорочують",
                        m.as_str(),
                        quote(&title)
                    ),
                    Vec::new(),
                );
            }
            if h.level > 1 && case_of(&h.title) == Some(Case::Lower) {
                self.add(
                    "heading-case",
                    Some(h.loc.clone()),
                    format!("заголовок з малої літери: {}", quote(&title)),
                    case_fix(&h.title, Case::Upper).into_iter().collect(),
                );
            }

            let structural = structural(&title);
            let appendix = h.level == 1 && h.appendix;
            if appendix {
                appendices.push(h);
                if case_of(&h.title) == Some(Case::Lower) {
                    self.add(
                        "appendix-case",
                        Some(h.loc.clone()),
                        format!("заголовок додатка з малої літери: {}", quote(&title)),
                        case_fix(&h.title, Case::Upper).into_iter().collect(),
                    );
                }
            } else if h.level == 1 && h.numbered && structural.is_some() {
                // `= Вступ` -> `#heading(numbering: none)[Вступ]`, мітка
                // зсередини переходить за виклик
                let fix = h.markup.as_ref().map(|(node, body)| {
                    let label = match &h.label {
                        Some(Label::Name(l))
                            if doc.files[node.file].source.text()[node.range.clone()]
                                .contains(&format!("<{l}>")) =>
                        {
                            format!(" <{l}>")
                        }
                        _ => String::new(),
                    };
                    FixDraft {
                        title: "Без номера: #heading(numbering: none)[…]".into(),
                        edits: vec![(
                            node.clone(),
                            format!("#heading(numbering: none)[{body}]{label}"),
                        )],
                    }
                });
                self.add(
                    "heading-numbered",
                    Some(h.loc.clone()),
                    format!("структурний елемент {} має бути без номера", quote(&title)),
                    fix.into_iter().collect(),
                );
            }

            let rank = if h.level != 1 {
                None
            } else if appendix {
                Some(10)
            } else if let Some((_, rank)) = structural {
                Some(rank)
            } else if h.numbered {
                Some(6)
            } else {
                None
            };
            if let Some(rank) = rank {
                match &latest {
                    Some((latest_rank, latest_title, latest_loc)) if rank < *latest_rank => {
                        let name = if rank == 6 {
                            format!("розділ {}", quote(&title))
                        } else {
                            quote(&title)
                        };
                        let msg = format!(
                            "{name} стоїть після {}, а за стандартом має бути перед ним",
                            quote(latest_title)
                        );
                        let before = latest_loc.clone();
                        self.add("structure-order", Some(h.loc.clone()), msg, Vec::new());
                        self.relate("structure-order", before, "має стояти перед цим елементом");
                    }
                    _ => latest = Some((rank, title.clone(), h.loc.clone())),
                }
            }

            self.close(&mut open, h.level);
            if h.numbered {
                if let Some((_, children)) = open.last_mut() {
                    children.push(h);
                }
                open.push((h, Vec::new()));
            }
        }
        self.close(&mut open, 0);

        // Обов'язкові елементи (3.5.1): титульний аркуш і суть з коду видно не завжди,
        // тож перевіряються реферат, вступ, висновки й наявність розділів
        let anchor = doc.anchor();
        for (name, shown) in [
            ("реферат", "«Реферат»"),
            ("вступ", "«Вступ»"),
            ("висновки", "«Висновки»"),
        ] {
            if !present.contains(&name) {
                self.add(
                    "structure-required",
                    Some(anchor.clone()),
                    format!("немає обов'язкового структурного елемента {shown}"),
                    Vec::new(),
                );
            }
        }
        if !chapters {
            self.add(
                "structure-required",
                Some(anchor),
                "немає нумерованих розділів суті звіту".into(),
                Vec::new(),
            );
        }

        let mut last_ref: Option<usize> = None;
        for app in appendices {
            let title = app.title.trimmed();
            let Ok(first) = self.first_ref(app.label.as_ref()) else {
                continue;
            };
            let Some(first) = first else {
                self.add(
                    "appendix-unreferenced",
                    Some(app.loc.clone()),
                    format!("на додаток {} немає посилання в тексті", quote(title)),
                    Vec::new(),
                );
                continue;
            };
            if last_ref.is_some_and(|last| first < last) {
                self.add(
                    "appendix-order",
                    Some(app.loc.clone()),
                    format!(
                        "на додаток {} вперше посилаються раніше, ніж на попередній",
                        quote(title)
                    ),
                    Vec::new(),
                );
            }
            last_ref = Some(first);
        }
    }

    /// Закриває заголовки рівня `level` і глибше: єдиний нумерований нащадок
    /// порушує 7.4.6.
    fn close(&mut self, open: &mut Vec<(&Heading, Vec<&Heading>)>, level: usize) {
        while open.last().is_some_and(|(h, _)| h.level >= level) {
            let (parent, children) = open.pop().unwrap();
            if let [only] = children[..] {
                self.add(
                    "heading-single",
                    Some(only.loc.clone()),
                    format!(
                        "єдиний підзаголовок у {}: {}",
                        quote(parent.title.trimmed()),
                        quote(only.title.trimmed())
                    ),
                    Vec::new(),
                );
                self.relate(
                    "heading-single",
                    parent.loc.clone(),
                    "заголовок, у якому підзаголовок єдиний",
                );
            }
        }
    }

    fn figures(&mut self) {
        let doc = self.doc;
        for block in &doc.blocks {
            let Block::Figure(f) = block else { continue };
            let (mut name, mut name_acc) = match f.kind {
                FigureKind::Image => ("рисунок".to_owned(), "рисунок".to_owned()),
                FigureKind::Table => ("таблиця".to_owned(), "таблицю".to_owned()),
            };
            let loc = f
                .caption
                .as_ref()
                .and_then(Text::whole)
                .unwrap_or_else(|| f.loc.clone());
            if let Some(caption) = &f.caption {
                let text = caption.trimmed();
                name = format!("{name} {}", quote(text));
                name_acc = format!("{name_acc} {}", quote(text));
                if case_of(caption) == Some(Case::Lower) {
                    self.add(
                        "caption-case",
                        caption.whole(),
                        format!("назва з малої літери: {name}"),
                        case_fix(caption, Case::Upper).into_iter().collect(),
                    );
                }
            }

            match self.first_ref(f.label.as_ref()) {
                Err(()) => {}
                Ok(None) => {
                    let rule = match f.kind {
                        FigureKind::Table => "table-unreferenced",
                        FigureKind::Image => "figure-unreferenced",
                    };
                    self.add(
                        rule,
                        Some(loc.clone()),
                        format!("на {name_acc} немає посилання в тексті"),
                        Vec::new(),
                    );
                }
                Ok(Some(first)) if f.tick < first && !f.floating => {
                    self.add(
                        "figure-before-ref",
                        Some(loc.clone()),
                        format!("{name} стоїть раніше за перше посилання на нього"),
                        Vec::new(),
                    );
                    if let Some(Label::Name(label)) = &f.label
                        && let Some(r) = doc.ref_uses.iter().find(|r| &r.target == label)
                    {
                        self.relate("figure-before-ref", r.loc.clone(), "перше посилання");
                    }
                }
                Ok(Some(_)) => {}
            }

            let Some(header) = &f.header else { continue };
            for cell in &header.cells {
                let text = cell.trimmed();
                if ends_with_period(text) {
                    self.add(
                        "table-header-period",
                        cell.whole(),
                        format!("{name}: крапка в кінці заголовка колонки {}", quote(text)),
                        last_char_fix(cell, "Прибрати крапку", "")
                            .into_iter()
                            .collect(),
                    );
                }
                // Підзаголовки (другий рядок головки) можуть починатися з малої
                // літери, тож регістр перевіряється лише в однорядковій головці
                if header.single_row && case_of(cell) == Some(Case::Lower) {
                    self.add(
                        "table-header-case",
                        cell.whole(),
                        format!("{name}: заголовок колонки з малої літери {}", quote(text)),
                        case_fix(cell, Case::Upper).into_iter().collect(),
                    );
                }
            }
        }
    }

    fn lists(&mut self) {
        let doc = self.doc;
        for (id, block) in doc.blocks.iter().enumerate() {
            let Block::List(list) = block else { continue };
            // Записи переліку джерел оформлюють за ДСТУ 8302, а перелік
            // скорочень за 7.14, а не за 7.7
            if matches!(list.section, Some(BIBLIOGRAPHY | ABBREVIATIONS)) {
                continue;
            }
            let last = list
                .items
                .iter()
                .rposition(|i| !i.text.trimmed().is_empty());
            let total = list
                .items
                .iter()
                .filter(|i| !i.text.trimmed().is_empty())
                .count();
            let case_group = |text: &str| Group {
                id,
                total,
                summary: "позицій переліку з великої літери",
                example: quote(text),
            };
            let punct_group = |text: &str| Group {
                id,
                total,
                summary: "позицій переліку з неправильним знаком у кінці: позиції закінчують «;», останню — «.»",
                example: tail(text),
            };
            for (n, item) in list.items.iter().enumerate() {
                let text = item.text.trimmed();
                if text.is_empty() {
                    continue;
                }
                let before = self.out.len();
                // Підсвічується місце порушення: перше слово (регістр) чи
                // останнє (розділовий знак), а не вся багаторядкова позиція
                let range = item.text.trimmed_range();
                let first_end = text
                    .find(char::is_whitespace)
                    .map_or(range.end, |i| range.start + i);
                let last_start = text
                    .rfind(char::is_whitespace)
                    .map_or(range.start, |i| range.start + i + 1);
                if case_of(&item.text) == Some(Case::Upper) {
                    self.add_in(
                        Some(case_group(text)),
                        "list-case",
                        item.text
                            .loc(range.start..first_end)
                            .or(Some(item.loc.clone())),
                        format!("позиція переліку з великої літери: {}", quote(text)),
                        case_fix(&item.text, Case::Lower).into_iter().collect(),
                    );
                }
                let is_last = Some(n) == last;
                let mark = if is_last { "." } else { ";" };
                let loc = item
                    .text
                    .loc(last_start..range.end)
                    .or(Some(item.loc.clone()));
                let period = ends_with_period(text);
                if item.has_nested {
                    // Позиція вводить вкладений перелік: перед ним, як і перед
                    // будь-яким переліком, двокрапка (7.7.1)
                    if !text.ends_with(':') {
                        let fix = if text.ends_with([';', ',']) || period {
                            let last = text.chars().last().unwrap_or(' ');
                            last_char_fix(&item.text, &format!("Замінити «{last}» на «:»"), ":")
                        } else if ends_with_sentence_mark(text) {
                            None
                        } else {
                            item.text.after(range.end).map(|at| FixDraft {
                                title: "Поставити двокрапку".into(),
                                edits: vec![(at, ":".into())],
                            })
                        };
                        self.add(
                            "list-colon",
                            loc,
                            format!(
                                "перед вкладеним переліком немає двокрапки: позиція закінчується {}",
                                tail(text)
                            ),
                            fix.into_iter().collect(),
                        );
                    }
                } else if !(text.ends_with(';') || period || text.ends_with(':')) {
                    // Кома в кінці замінюється, інакше знак дописується; після
                    // «?», «!», «…» дописаний знак дав би «?;», тож без виправлення
                    let fix = if text.ends_with(',') {
                        last_char_fix(&item.text, &format!("Замінити «,» на «{mark}»"), mark)
                    } else if ends_with_sentence_mark(text) {
                        None
                    } else {
                        item.text.after(range.end).map(|at| FixDraft {
                            title: format!("Поставити «{mark}»"),
                            edits: vec![(at, mark.to_owned())],
                        })
                    };
                    self.add_in(
                        Some(punct_group(text)),
                        "list-punct",
                        loc,
                        format!("позиція переліку без «;» або «.» в кінці: {}", tail(text)),
                        fix.into_iter().collect(),
                    );
                } else if period && !is_last {
                    self.add_in(
                        Some(punct_group(text)),
                        "list-punct",
                        loc,
                        format!(
                            "крапка в кінці позиції посеред переліку, має бути «;»: {}",
                            tail(text)
                        ),
                        last_char_fix(&item.text, "Замінити «.» на «;»", ";")
                            .into_iter()
                            .collect(),
                    );
                } else if text.ends_with(';') && is_last && !list.nested {
                    // Останню позицію вкладеного переліку можна закінчити й «;»:
                    // зовнішній перелік триває
                    self.add_in(
                        Some(punct_group(text)),
                        "list-punct",
                        loc,
                        format!(
                            "остання позиція переліку закінчується «;», має бути «.»: {}",
                            tail(text)
                        ),
                        last_char_fix(&item.text, "Замінити «;» на «.»", ".")
                            .into_iter()
                            .collect(),
                    );
                }
                // У терміналі показується вся позиція (без вкладеного переліку)
                let context = Loc {
                    file: item.loc.file,
                    range: item.loc.range.start
                        ..item
                            .text
                            .whole()
                            .filter(|w| w.file == item.loc.file)
                            .map_or(item.loc.range.end, |w| w.range.end),
                };
                for f in &mut self.out[before..] {
                    f.context.get_or_insert_with(|| context.clone());
                }
            }

            let Some(Block::Par(par)) = list.prev_par.map(|i| &doc.blocks[i]) else {
                continue;
            };
            let text = par.text.trimmed();
            if text.ends_with(':') {
                continue;
            }
            let end = par.text.trimmed_range().end;
            let last = text.chars().last();
            let last_char = last.map_or(0, char::len_utf8);
            let fix = if ends_with_sentence_mark(text) {
                // Для «Що це?:», «…:» виправлення немає: речення краще переписати самому
                None
            } else if text.ends_with('.') {
                last_char_fix(&par.text, "Замінити крапку на двокрапку", ":")
            } else if let Some(c @ (',' | ';')) = last {
                last_char_fix(&par.text, &format!("Замінити «{c}» на двокрапку"), ":")
            } else {
                par.text.after(end).map(|at| FixDraft {
                    title: "Поставити двокрапку".into(),
                    edits: vec![(at, ":".into())],
                })
            };
            self.add(
                "list-colon",
                par.text.loc(end - last_char..end),
                format!(
                    "перед переліком немає двокрапки: абзац закінчується {}",
                    tail(text)
                ),
                fix.into_iter().collect(),
            );
            if let Some(first) = list.items.first() {
                self.relate("list-colon", first.loc.clone(), "перелік");
            }
        }
    }

    fn paragraphs(&mut self) {
        let doc = self.doc;
        let has_bibliography = doc.has_bibliography
            || doc.blocks.iter().any(|b| {
                matches!(b, Block::Heading(h) if structural(h.title.trimmed()).is_some_and(|(n, _)| n == BIBLIOGRAPHY))
            });
        let mut cited = false;
        for block in &doc.blocks {
            let Block::Par(par) = block else { continue };
            if par.section == Some(BIBLIOGRAPHY) {
                continue;
            }
            let raw = par.text.as_str();
            let start = par.text.trimmed_range().start;

            if par.after_equation
                && let Some(m) = WHERE_COLON.find(par.text.trimmed())
            {
                let colon = start + m.end() - 1;
                let next = raw[colon + 1..].chars().next();
                let with = if next.is_some_and(char::is_whitespace) {
                    ""
                } else {
                    " "
                };
                let fix = par.text.exact_loc(colon..colon + 1).map(|at| FixDraft {
                    title: "Прибрати двокрапку після «де»".into(),
                    edits: vec![(at, with.into())],
                });
                self.add(
                    "formula-where",
                    par.text.loc(start..start + m.end()),
                    format!(
                        "пояснення познак з двокрапкою після «де»: {}",
                        quote(par.text.trimmed())
                    ),
                    fix.into_iter().collect(),
                );
            }

            self.note(par);

            // Посилання «[N]», набрані вручну, без переліку джерел (3.5.2)
            if !has_bibliography
                && !cited
                && let Some(m) = CITATION
                    .captures_iter(raw)
                    .filter_map(|c| c.get(1))
                    .find(|m| par.text.is_exact_at(m.start()))
            {
                cited = true;
                self.add(
                    "bibliography-missing",
                    par.text.loc(m.range()),
                    format!(
                        "у тексті є посилання «{}», а переліку джерел посилання немає",
                        m.as_str()
                    ),
                    Vec::new(),
                );
            }

            for (rule, regex, msg) in UNIT_CHECKS.iter() {
                if !(self.active)(rule) {
                    continue;
                }
                for m in regex.find_iter(raw) {
                    let found = m.as_str().trim();
                    let found = found
                        .trim_end_matches(|c: char| !(c.is_alphanumeric() || c == '%' || c == '°'));
                    let begin = m.start() + (m.as_str().len() - m.as_str().trim_start().len());
                    let range = begin..begin + found.len();
                    let mut fixes = Vec::new();
                    if *rule == "unit-tolerance"
                        && let Some(t) = TOLERANCE.find(found)
                        && let Some(open) = par.text.exact_loc(begin..begin)
                        && let Some(close) = par.text.after(begin + t.end())
                    {
                        fixes.push(FixDraft {
                            title: format!("Взяти в дужки: «({}) …»", t.as_str()),
                            edits: vec![(open, "(".into()), (close, ")".into())],
                        });
                    }
                    // Розміри й діапазон: одиниця біля кожного числа
                    if matches!(*rule, "unit-dimensions" | "unit-range")
                        && let Some(exact) = par.text.exact_loc(range.clone())
                        && let Some(new) = unit_rewrite(rule, found)
                    {
                        fixes.push(FixDraft {
                            title: format!("Записати «{new}»"),
                            edits: vec![(exact, new)],
                        });
                    }
                    if *rule == "unit-space" {
                        // Нерозривний проміжок між числом і одиницею: `5~мм`
                        let digit = begin + found.chars().next().map_or(1, char::len_utf8);
                        if let Some(at) = par.text.exact_loc(digit..digit) {
                            fixes.push(FixDraft {
                                title: "Додати нерозривний проміжок (~)".into(),
                                edits: vec![(at, "~".into())],
                            });
                        }
                    }
                    self.add(
                        rule,
                        par.text.loc(range),
                        format!("{msg}{}", quote(found)),
                        fixes,
                    );
                }
            }
        }
    }

    /// Примітка (7.8.3, 7.8.4): «Примітка.» або «Примітка 1.», далі текст з
    /// великої літери.
    fn note(&mut self, par: &crate::model::Par) {
        let start = par.text.trimmed_range().start;
        let Some(c) = NOTE.captures(par.text.trimmed()) else {
            return;
        };
        let word = c.get(1).unwrap();
        let at = |m: regex::Match| start + m.start()..start + m.end();
        if word.as_str().ends_with(['и', 'И']) {
            self.add(
                "note-format",
                par.text.loc(at(word)),
                "кожну примітку подають окремо: «Примітка 1.», «Примітка 2.», а не «Примітки»"
                    .into(),
                Vec::new(),
            );
            return;
        }
        let mark = c.get(3).unwrap();
        let sign = mark.as_str().trim();
        if sign != "." {
            let range = at(mark);
            let fix = if sign.is_empty() {
                par.text.after(range.start).map(|loc| FixDraft {
                    title: "Поставити крапку після «Примітка»".into(),
                    edits: vec![(loc, ".".into())],
                })
            } else {
                let sign_start = range.start + mark.as_str().find(sign).unwrap_or(0);
                par.text
                    .exact_loc(sign_start..sign_start + sign.len())
                    .map(|loc| FixDraft {
                        title: format!("Замінити «{sign}» на крапку"),
                        edits: vec![(loc, ".".into())],
                    })
            };
            self.add(
                "note-format",
                par.text.loc(at(word)),
                format!(
                    "після слова «Примітка» ставлять крапку, а не {}",
                    if sign.is_empty() {
                        "нічого".into()
                    } else {
                        format!("«{sign}»")
                    }
                ),
                fix.into_iter().collect(),
            );
        }
        if let Some(letter) = c.get(4)
            && letter
                .as_str()
                .chars()
                .next()
                .is_some_and(char::is_lowercase)
        {
            let range = at(letter);
            let fix = par.text.exact_loc(range.clone()).map(|loc| FixDraft {
                title: format!(
                    "Почати з великої літери: «{}»",
                    letter.as_str().to_uppercase()
                ),
                edits: vec![(loc, letter.as_str().to_uppercase())],
            });
            self.add(
                "note-format",
                par.text.loc(range),
                "текст примітки починають з великої літери".into(),
                fix.into_iter().collect(),
            );
        }
    }

    /// Нумерація приміток (7.8.3, 7.8.4): одну не нумерують, кілька поспіль
    /// нумерують по порядку.
    fn notes(&mut self) {
        let doc = self.doc;
        // Примітки поспіль: (абзац, слово, номер)
        let mut runs: Vec<Vec<(&crate::model::Par, regex::Match, Option<regex::Match>)>> =
            Vec::new();
        let mut prev: Option<usize> = None;
        for (i, block) in doc.blocks.iter().enumerate() {
            let Block::Par(par) = block else { continue };
            let Some(c) = NOTE.captures(par.text.trimmed()) else {
                continue;
            };
            let word = c.get(1).unwrap();
            if word.as_str().ends_with(['и', 'И']) {
                continue;
            }
            if prev != Some(i.wrapping_sub(1)) {
                runs.push(Vec::new());
            }
            prev = Some(i);
            runs.last_mut().unwrap().push((par, word, c.get(2)));
        }
        for run in runs {
            let single = run.len() == 1;
            for (k, (par, word, number)) in run.into_iter().enumerate() {
                let start = par.text.trimmed_range().start;
                let expected = k + 1;
                match number {
                    Some(n) if single => {
                        let range = start + n.start()..start + n.end();
                        let fix = par.text.exact_loc(range.clone()).map(|loc| FixDraft {
                            title: "Прибрати номер примітки".into(),
                            edits: vec![(loc, String::new())],
                        });
                        self.add(
                            "note-numbering",
                            par.text.loc(start + word.start()..range.end),
                            "одну примітку не нумерують: «Примітка.»".into(),
                            fix.into_iter().collect(),
                        );
                    }
                    Some(n) if n.as_str().trim() != expected.to_string() => {
                        let range = start + n.start()..start + n.end();
                        let fix = par.text.exact_loc(range.clone()).map(|loc| FixDraft {
                            title: format!("Замінити номер на {expected}"),
                            edits: vec![(loc, format!(" {expected}"))],
                        });
                        self.add(
                            "note-numbering",
                            par.text.loc(start + word.start()..range.end),
                            format!(
                                "примітку {expected} позначено номером {}",
                                n.as_str().trim()
                            ),
                            fix.into_iter().collect(),
                        );
                    }
                    None if !single => {
                        let end = start + word.end();
                        let fix = par.text.after(end).map(|loc| FixDraft {
                            title: format!("Пронумерувати: «Примітка {expected}»"),
                            edits: vec![(loc, format!(" {expected}"))],
                        });
                        self.add(
                            "note-numbering",
                            par.text.loc(start + word.start()..end),
                            format!("приміток кілька, тож їх нумерують: «Примітка {expected}.»"),
                            fix.into_iter().collect(),
                        );
                    }
                    _ => {}
                }
            }
        }
    }

    fn keywords(&mut self) {
        let doc = self.doc;
        for block in &doc.blocks {
            match block {
                // Шаблон сам розставляє ключові слова за абеткою
                Block::Keywords { count, loc } => self.keywords_count(*count, loc.clone()),
                // Набрані вручну: рядок великими літерами через кому в рефераті
                Block::Par(par) if par.section == Some(ABSTRACT) => {
                    let text = par.text.trimmed();
                    let letters = || text.chars().filter(|c| c.is_alphabetic());
                    if text.matches(',').count() < 2
                        || letters().any(char::is_lowercase)
                        || letters().count() < 10
                    {
                        continue;
                    }
                    let words: Vec<&str> = text
                        .trim_end_matches('.')
                        .split(',')
                        .map(str::trim)
                        .filter(|w| !w.is_empty())
                        .collect();
                    let Some(loc) = par.text.whole() else {
                        continue;
                    };
                    self.keywords_count(words.len(), loc.clone());
                    let keys: Vec<_> = words.iter().map(|w| collation_key(w)).collect();
                    if let Some(i) = out_of_order(&keys) {
                        // Рядок набрано в коді дослівно: його можна впорядкувати
                        let fix = par.text.exact_loc(par.text.trimmed_range()).map(|exact| {
                            let mut sorted = words.clone();
                            sorted.sort_by_key(|w| collation_key(w));
                            let end = if text.ends_with('.') { "." } else { "" };
                            FixDraft {
                                title: "Розташувати за абеткою".into(),
                                edits: vec![(exact, format!("{}{end}", sorted.join(", ")))],
                            }
                        });
                        self.add(
                            "keywords-order",
                            Some(loc),
                            format!(
                                "ключові слова не за абеткою: «{}» стоїть після «{}»",
                                words[i],
                                words[i - 1]
                            ),
                            fix.into_iter().collect(),
                        );
                    }
                }
                _ => {}
            }
        }
    }

    fn keywords_count(&mut self, count: usize, loc: Loc) {
        if !(5..=15).contains(&count) {
            self.add(
                "keywords-count",
                Some(loc),
                format!("ключових слів у рефераті: {count}, має бути від 5 до 15"),
                Vec::new(),
            );
        }
    }

    /// Перелік скорочень за абеткою (7.14): терміни (`/ ЕОМ: …`), перша
    /// колонка таблиці, позиції переліку чи абзаци «ЕОМ – …» поспіль.
    fn abbreviations(&mut self) {
        let doc = self.doc;
        let mut groups: Vec<Vec<(String, Loc)>> = Vec::new();
        let mut prev_par: Option<usize> = None;
        let term = |text: &Text| -> Option<(String, Loc)> {
            let s = text.trimmed();
            let m = TERM_SEPARATOR.find(s)?;
            let start = text.trimmed_range().start;
            let term = s[..m.start()].trim();
            (!term.is_empty() && term.chars().count() <= 40)
                .then(|| text.loc(start..start + term.len()))
                .flatten()
                .map(|loc| (term.to_owned(), loc))
        };
        let whole = |text: &Text| {
            let s = text.trimmed();
            (!s.is_empty())
                .then(|| text.whole())
                .flatten()
                .map(|loc| (s.to_owned(), loc))
        };
        for (i, block) in doc.blocks.iter().enumerate() {
            match block {
                Block::Terms(terms) => groups.push(terms.iter().filter_map(whole).collect()),
                Block::List(list) if list.section == Some(ABBREVIATIONS) && !list.nested => {
                    groups.push(list.items.iter().filter_map(|it| term(&it.text)).collect());
                }
                Block::Par(par) if par.section == Some(ABBREVIATIONS) => {
                    let Some(entry) = term(&par.text) else {
                        continue;
                    };
                    if prev_par != Some(i.wrapping_sub(1)) {
                        groups.push(Vec::new());
                    }
                    prev_par = Some(i);
                    groups.last_mut().unwrap().push(entry);
                }
                _ => {}
            }
        }
        for group in groups {
            let keys: Vec<_> = group.iter().map(|(t, _)| collation_key(t)).collect();
            let Some(i) = out_of_order(&keys) else {
                continue;
            };
            let (term, loc) = &group[i];
            let (prev, _) = &group[i - 1];
            let msg = if is_cyrillic(term) && !is_cyrillic(prev) {
                format!(
                    "«{term}» стоїть після «{prev}»: спершу наводять скорочення українською, потім іншими мовами"
                )
            } else {
                format!("скорочення не за абеткою: «{term}» стоїть після «{prev}»")
            };
            self.add("abbrev-order", Some(loc.clone()), msg, Vec::new());
        }
    }

    /// `bibliography`: лише джерела з посиланнями й номерні посилання (5.5.1).
    fn bibliographies(&mut self) {
        let doc = self.doc;
        for bib in &doc.bibs {
            if let Some(loc) = &bib.full {
                self.add(
                    "bibliography-full",
                    Some(loc.clone()),
                    "`full: true` друкує в переліку й джерела, на які немає посилань у тексті"
                        .into(),
                    vec![FixDraft {
                        title: "Друкувати лише джерела з посиланнями".into(),
                        edits: vec![(loc.clone(), "false".into())],
                    }],
                );
            }
            if let Some((style, loc)) = &bib.style
                && is_author_date(style)
            {
                self.add(
                    "bibliography-style",
                    Some(loc.clone()),
                    format!(
                        "стиль «{style}» посилається на джерела автором і роком чи виноскою, а за стандартом — номером у квадратних дужках"
                    ),
                    Vec::new(),
                );
            }
        }
    }

    /// Посилання на рисунки, таблиці, формули й розділи (7.11.2): типовий
    /// підпис typst («Рисунок 2.1», «Таблиця 1», «Рівняння (3)») друкується з
    /// великої літери в називному відмінку, тож посеред речення його не
    /// лишають, а слово перед посиланням не повторюють.
    fn refs(&mut self) {
        let doc = self.doc;
        // Власне show-правило `ref` чи `set ref(supplement: …)`: як друкується
        // посилання, з коду не видно
        if doc.custom_ref || doc.set_rule("ref", "supplement").is_some() {
            return;
        }
        let mut kinds: std::collections::HashMap<&str, RefKind> = Default::default();
        for block in &doc.blocks {
            let (label, kind) = match block {
                Block::Figure(f) => (
                    &f.label,
                    match f.kind {
                        FigureKind::Image => RefKind::Image,
                        FigureKind::Table => RefKind::Table,
                    },
                ),
                Block::Equation { label, .. } => (label, RefKind::Equation),
                Block::Heading(h) if h.appendix && h.level == 1 => (&h.label, RefKind::Appendix),
                Block::Heading(h) => (&h.label, RefKind::Section),
                _ => continue,
            };
            if let Some(Label::Name(name)) = label {
                kinds.insert(name, kind);
            }
        }
        // Підпис, заданий документом (`set figure(supplement: [рис.])`),
        // може бути з малої літери: тоді регістр не перевіряється
        let lowercase = |target: &str| {
            doc.set_rule(target, "supplement")
                .and_then(|s| s.value("supplement"))
                .is_some_and(|v| {
                    let v = v.trim_start_matches(['[', '"', '(']);
                    v == "none" || v.starts_with(char::is_lowercase)
                })
        };
        let figure_lower = lowercase("figure");
        let equation_lower = lowercase("math.equation");
        let heading_lower = lowercase("heading");
        for r in &doc.ref_uses {
            if r.supplement {
                continue;
            }
            let Some(&kind) = kinds.get(r.target.as_str()) else {
                continue;
            };
            let (stems, printed, short): (&[&str], &str, Option<&str>) = match kind {
                RefKind::Image => (&["рисун", "рис."], "Рисунок N", Some("рис.")),
                RefKind::Table => (&["таблиц", "табл."], "Таблиця N", Some("табл.")),
                RefKind::Equation => (&["формул", "рівнян"], "Рівняння (N)", None),
                RefKind::Appendix => (&["додат", "дод."], "Додаток N", Some("дод.")),
                RefKind::Section => (
                    &["розділ", "підрозділ", "пункт", "підпункт", "п.", "розд."],
                    "Розділ N",
                    None,
                ),
            };
            let word = last_word(&r.before);
            let lower = word.to_lowercase();
            let node = r.loc.clone();
            let shown = format!("@{}", r.target);
            // Лише `@мітка`: у `ref(<мітка>)` дописати `[]` нікуди
            let bare = doc.files[node.file].source.text()[node.range.clone()].starts_with('@');
            let at_end = Loc {
                file: node.file,
                range: node.range.end..node.range.end,
            };
            if !word.is_empty() && stems.iter().any(|s| lower.starts_with(s)) {
                // Слово вже є: друкується «рисунку Рисунок 2.1». Слово переходить у
                // підпис посилання: `@fig[рисунку]` друкує «рисунку 2.1», і
                // посиланням стає весь вираз
                let text = doc.files[node.file].source.text();
                let lead = text[..node.range.start].trim_end_matches([' ', '\t', '~', '\n']);
                let fix = (bare && lead.ends_with(word)).then(|| FixDraft {
                    title: format!("У посилання: {shown}[{word}]"),
                    edits: vec![(
                        Loc {
                            file: node.file,
                            range: lead.len() - word.len()..node.range.end,
                        },
                        format!("{shown}[{word}]"),
                    )],
                });
                self.add(
                    "ref-supplement",
                    Some(node),
                    format!(
                        "перед {shown} уже стоїть «{word}», а посилання ще й друкує «{printed}»: вийде «{word} {printed}»"
                    ),
                    fix.into_iter().collect(),
                );
                continue;
            }
            let custom_lower = match kind {
                RefKind::Image | RefKind::Table => figure_lower,
                RefKind::Equation => equation_lower,
                RefKind::Appendix | RefKind::Section => heading_lower,
            };
            if custom_lower || sentence_start(&r.before) {
                continue;
            }
            // Посеред речення: «на Рисунок 2.1», «(Таблиця 3)». Скорочення
            // (ДСТУ 3582) відмінків не має, тож підходить у будь-яке речення
            let fix = short.filter(|_| bare).map(|short| FixDraft {
                title: format!("Скорочено: {shown}[{short}]"),
                edits: vec![(at_end.clone(), format!("[{short}]"))],
            });
            let example = match kind {
                RefKind::Image => "«на @x[рисунку]», «(@x[рис.])»",
                RefKind::Table => "«у @x[таблиці]», «(@x[табл.])»",
                RefKind::Equation => "«згідно з @x[формулою]»",
                RefKind::Appendix => "«у @x[додатку]», «(@x[дод.])»",
                RefKind::Section => "«у @x[розділі]»",
            };
            self.add(
                "ref-supplement",
                Some(node),
                format!(
                    "{shown} посеред речення друкує «{printed}» з великої літери в називному відмінку; пишуть {}",
                    example.replace("@x", &shown)
                ),
                fix.into_iter().collect(),
            );
        }
    }

    /// Номерні посилання «[N]», набрані в тексті (5.5.1): джерела в переліку
    /// нумерують за першою згадкою, тож нові номери йдуть 1, 2, 3, … Джерела
    /// додатків нумерують окремо (7.15.5), тож додатки не перевіряються.
    fn citations(&mut self) {
        let doc = self.doc;
        let mut seen = 0u32;
        for block in &doc.blocks {
            let texts: Vec<&Text> = match block {
                Block::Heading(h) if h.appendix => break,
                Block::Par(p) if p.section != Some(BIBLIOGRAPHY) => vec![&p.text],
                Block::List(l) if !matches!(l.section, Some(BIBLIOGRAPHY | ABBREVIATIONS)) => {
                    l.items.iter().map(|i| &i.text).collect()
                }
                Block::Figure(f) => f.caption.iter().collect(),
                _ => continue,
            };
            for text in texts {
                for c in CITE_GROUP.captures_iter(text.as_str()) {
                    let whole = c.get(0).unwrap();
                    let start = whole.start() + whole.as_str().find('[').unwrap_or(0);
                    // Дужки з формули чи коду (`$[3, 6]$`, `` `a[2]` ``) не є
                    // посиланнями: беруться лише набрані в тексті дослівно
                    if !text.is_exact_at(start) {
                        continue;
                    }
                    if let Some((n, expected)) = cite_out_of_order(&c, &mut seen) {
                        self.add(
                            "citation-order",
                            text.loc(start..whole.end()),
                            format!(
                                "джерело [{n}] згадано раніше за [{expected}]: джерела нумерують у порядку першого посилання в тексті"
                            ),
                            Vec::new(),
                        );
                        // Далі номери вже зсунуто: одного зауваження досить
                        return;
                    }
                }
            }
        }
    }

    /// Кілька формул поспіль без тексту між ними розділяють комами (7.10.11).
    fn formulas(&mut self) {
        let doc = self.doc;
        for pair in doc.blocks.windows(2) {
            let [
                Block::Equation { loc, .. },
                Block::Equation {
                    loc: next,
                    follows: true,
                    ..
                },
            ] = pair
            else {
                continue;
            };
            let text = &doc.files[loc.file].source.text()[loc.range.clone()];
            let Some(body) = text.strip_prefix('$').and_then(|t| t.strip_suffix('$')) else {
                continue;
            };
            let trimmed = body.trim_end();
            if trimmed.ends_with(',') || trimmed.is_empty() {
                continue;
            }
            // Кінець тіла формули в коді
            let end = loc.range.start + 1 + trimmed.len();
            let last = trimmed.chars().last().unwrap_or(' ');
            let fix = if matches!(last, '.' | ';') {
                FixDraft {
                    title: format!("Замінити «{last}» на кому"),
                    edits: vec![(
                        Loc {
                            file: loc.file,
                            range: end - 1..end,
                        },
                        ",".into(),
                    )],
                }
            } else {
                FixDraft {
                    title: "Поставити кому".into(),
                    edits: vec![(
                        Loc {
                            file: loc.file,
                            range: end..end,
                        },
                        ",".into(),
                    )],
                }
            };
            // Позначка стоїть на закривальному `$`, перед яким бракує коми
            self.add(
                "formula-comma",
                Some(Loc {
                    file: loc.file,
                    range: loc.range.end - 1..loc.range.end,
                }),
                "за формулою одразу йде наступна: формули, не відокремлені текстом, розділяють комами"
                    .into(),
                vec![fix],
            );
            if next.file == loc.file {
                self.relate("formula-comma", next.clone(), "наступна формула");
                if let Some(f) = self
                    .out
                    .last_mut()
                    .filter(|f| f.rule.code == "formula-comma")
                {
                    f.context = Some(Loc {
                        file: loc.file,
                        range: loc.range.start..next.range.end,
                    });
                }
            }
        }
    }
}

/// «80 х 25 х 50 мм» -> «80 мм × 25 мм × 50 мм», «від 1 до 5 мм» -> «від 1 мм
/// до 5 мм», «10-20 МГц» -> «від 10 МГц до 20 МГц».
fn unit_rewrite(rule: &str, found: &str) -> Option<String> {
    static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(NUM).unwrap());
    let numbers: Vec<&str> = NUMBER.find_iter(found).map(|m| m.as_str()).collect();
    let last = NUMBER.find_iter(found).last()?;
    let unit = found[last.end()..].trim();
    if unit.is_empty() || numbers.len() < 2 {
        return None;
    }
    match rule {
        "unit-dimensions" => Some(
            numbers
                .iter()
                .map(|n| format!("{n} {unit}"))
                .collect::<Vec<_>>()
                .join(" × "),
        ),
        "unit-range" if numbers.len() == 2 => Some(format!(
            "від {} {unit} до {} {unit}",
            numbers[0], numbers[1]
        )),
        _ => None,
    }
}

/// Що позначає мітка, на яку посилаються.
#[derive(Debug, Clone, Copy)]
enum RefKind {
    Image,
    Table,
    Equation,
    Appendix,
    Section,
}

/// Номери групи посилань: «1, 4–6» -> 1, 4, 5, 6; «[2]–[5]» -> 2…5.
fn cite_numbers(c: &regex::Captures) -> Vec<u32> {
    let mut out = Vec::new();
    for part in c[1].split([',', ';']) {
        let bounds: Vec<u32> = part
            .split(['–', '—', '-'])
            .filter_map(|n| n.trim().parse().ok())
            .collect();
        match bounds[..] {
            [a, b] if a < b && b - a <= 50 => out.extend(a..=b),
            _ => out.extend(bounds),
        }
    }
    if let (Some(&last), Some(end)) = (out.last(), c.get(2).and_then(|m| m.as_str().parse().ok()))
        && last < end
        && end - last <= 50
    {
        out.extend(last + 1..=end);
    }
    // Роки й номери стандартів у дужках («[2020]») не є посиланнями
    out.retain(|&n| (1..=500).contains(&n));
    out
}

/// Текст закінчується знаком, після якого «;», «.» чи «:» не дописують:
/// «?», «!», три крапки.
fn ends_with_sentence_mark(text: &str) -> bool {
    text.ends_with(['?', '!', '…']) || text.ends_with("...")
}

/// Перший номер групи посилань, що порушує порядок першої згадки:
/// (номер, очікуваний). `seen` містить найбільший номер, згаданий досі.
pub(crate) fn cite_out_of_order(c: &regex::Captures, seen: &mut u32) -> Option<(u32, u32)> {
    for n in cite_numbers(c) {
        if n == *seen + 1 {
            *seen = n;
        } else if n > *seen {
            return Some((n, *seen + 1));
        }
    }
    None
}

/// Останнє слово тексту (з крапкою скорочення).
fn last_word(text: &str) -> &str {
    let t = text.trim_end();
    let start = t
        .char_indices()
        .rev()
        .find(|&(_, c)| !(c.is_alphabetic() || c == '.' || c == '\'' || c == '’'))
        .map_or(0, |(i, c)| i + c.len_utf8());
    &t[start..]
}

/// Чи текст перед посиланням закінчує речення (або його немає): тоді
/// «Рисунок 2.1» з великої літери доречний.
fn sentence_start(before: &str) -> bool {
    let t = before.trim_end();
    if t.is_empty() {
        return true;
    }
    if !t.ends_with(['.', '!', '?', '…']) {
        return false;
    }
    // «див.», «рис.», «т. д.» є скороченнями, а не кінцем речення
    let word = last_word(t).to_lowercase();
    let word = word.trim_end_matches('.');
    !matches!(
        word,
        "див"
            | "рис"
            | "табл"
            | "с"
            | "п"
            | "напр"
            | "дод"
            | "ст"
            | "розд"
            | "підрозд"
            | "зокр"
            | "д"
            | "ін"
            | "пор"
            | "вид"
            | "гл"
    )
}

/// Виправлення регістру першої літери, якщо вона в коді така сама.
fn case_fix(text: &Text, case: Case) -> Option<FixDraft> {
    let start = text.trimmed_range().start;
    let first = text.as_str()[start..].chars().next()?;
    let loc = text.exact_loc(start..start + first.len_utf8())?;
    let new: String = match case {
        Case::Upper => first.to_uppercase().collect(),
        Case::Lower => first.to_lowercase().collect(),
    };
    let title = match case {
        Case::Upper => format!("Почати з великої літери: «{new}»"),
        Case::Lower => format!("Почати з малої літери: «{new}»"),
    };
    Some(FixDraft {
        title,
        edits: vec![(loc, new)],
    })
}

/// Заміна останнього символу тексту (крапки).
fn last_char_fix(text: &Text, title: &str, with: &str) -> Option<FixDraft> {
    let end = text.trimmed_range().end;
    let last = text.as_str()[..end].chars().last()?;
    let loc = text.exact_loc(end - last.len_utf8()..end)?;
    Some(FixDraft {
        title: title.into(),
        edits: vec![(loc, with.into())],
    })
}

/// Крок рядків через `leading` (в em) для шрифту з висотою великих літер
/// `cap`: typst ставить рядки на відстані `leading` + висота великої літери.
fn spacing(leading: f64, cap: f64) -> f64 {
    (leading + cap) / 1.15
}

/// Зауваження до налаштування, поки абзаци рахуються: (правило, місце,
/// повідомлення, виправлення, абзаців із цим налаштуванням, з-поміж скількох).
type Pending = (&'static str, Loc, String, Vec<FixDraft>, usize, usize);

/// Налаштування тексту й сторінки з коду (статична частина правил кегля,
/// шрифту, інтервалу, відступу й формату; решту бачить повна перевірка).
impl Checker<'_> {
    fn styles(&mut self) {
        let doc = self.doc;
        // Документ без жодного налаштування тексту чи сторінки й без шаблону
        // найімовірніше є фрагментом (розділ, відкритий окремо): типові
        // значення typst до нього не стосуються
        let configured = doc.template.is_some()
            || doc.sets.iter().any(|s| {
                s.selector.is_none() && matches!(s.target.as_str(), "page" | "text" | "par")
            });
        let mut all = Vec::new();
        let mut body = Vec::new();
        // Абзаци з абзацним відступом: «де …» і примітки друкують без нього
        let mut indented = Vec::new();
        // Перші абзаци після заголовка, рисунка, переліку
        let mut after_block = Vec::new();
        let mut started = false;
        let mut prev_par = false;
        for block in &doc.blocks {
            match block {
                Block::Heading(_) => started = true,
                Block::Par(p) => {
                    all.push(p.style);
                    // Основний текст: після першого заголовка (не титульний аркуш)
                    let head = p.text.trimmed().to_lowercase();
                    let unindented =
                        head == "де" || head.starts_with("де ") || head.starts_with("примітк");
                    if started {
                        body.push(p.style);
                        if !unindented {
                            indented.push(p.style);
                            if !prev_par {
                                after_block.push(p.style);
                            }
                        }
                    }
                }
                Block::Keywords { .. } => continue,
                _ => {}
            }
            prev_par = matches!(block, Block::Par(_));
        }
        // Скільки абзаців основного тексту з кожними налаштуваннями
        let total = body.len();
        let mut counts: std::collections::HashMap<usize, usize> = Default::default();
        for &i in &body {
            *counts.entry(i).or_default() += 1;
        }
        let mut indent_counts: std::collections::HashMap<usize, usize> = Default::default();
        for &i in &indented {
            *indent_counts.entry(i).or_default() += 1;
        }
        let indented_total = indented.len();
        for list in [&mut all, &mut body, &mut indented, &mut after_block] {
            list.sort_unstable();
            list.dedup();
        }
        // Зауваження за місцем налаштування: (правило, місце, повідомлення,
        // виправлення, абзаців, з-поміж скількох)
        let mut pending: Vec<Pending> = Vec::new();
        let mut report = |(count, total): (usize, usize),
                          code: &'static str,
                          origin: Option<&Origin>,
                          place: (&str, &[&str]),
                          msg: String,
                          fix: Option<(&str, &str)>| {
            if origin.is_none() && !configured {
                return;
            }
            let loc = origin.map_or_else(|| doc.setting(place.0, place.1), |o| o.loc.clone());
            if let Some(p) = pending
                .iter_mut()
                .find(|p| p.0 == code && p.1 == loc && p.2.starts_with(msg.as_str()))
            {
                p.4 += count;
                return;
            }
            let msg = match origin {
                None => format!("{msg} (типове значення typst)"),
                Some(Origin {
                    template: Some(t), ..
                }) => format!("{msg} (так задає шаблон, {t})"),
                Some(_) => msg,
            };
            let fixes = match (origin.and_then(|o| o.value.clone()), fix) {
                (Some(loc), Some((title, text))) => vec![FixDraft {
                    title: title.into(),
                    edits: vec![(loc, text.into())],
                }],
                _ => Vec::new(),
            };
            pending.push((code, loc, msg, fixes, count, total));
        };

        for &i in &body {
            let style = &doc.styles[i];
            let n = (counts[&i], total);
            if let Some(size) = style.size_pt()
                && (size - 14.0).abs() > 0.05
            {
                report(
                    n,
                    "font-size",
                    style.size.origin(),
                    ("text", &["size"]),
                    format!(
                        "основний текст набрано кеглем {}, має бути 14",
                        fmt_num(size)
                    ),
                    Some(("Замінити на 14pt", "14pt")),
                );
            }
            if let Some(font) = style.font.known()
                && !font.eq_ignore_ascii_case("times new roman")
            {
                report(
                    n,
                    "font-family",
                    style.font.origin(),
                    ("text", &["font"]),
                    format!("основний текст набрано шрифтом «{font}», має бути Times New Roman"),
                    Some(("Замінити на Times New Roman", "\"Times New Roman\"")),
                );
            }
            // Інтервал: висота великої літери в шрифтах 0,6–0,75 em, тож
            // зауваження лише там, де інтервал поза межами за будь-якої з них
            if let Some(em) = style.leading_em()
                && (spacing(em, 0.75) < 1.35 || spacing(em, 0.6) > 2.2)
            {
                report(
                    n,
                    "line-spacing",
                    style.leading.origin(),
                    ("par", &["leading"]),
                    format!(
                        "основний текст набрано приблизно через {} інтервалу (leading {} em), має бути 1,5–2",
                        fmt_num(spacing(em, 0.662)),
                        fmt_num(em)
                    ),
                    None,
                );
            }
        }

        for &i in &indented {
            let style = &doc.styles[i];
            let n = (indent_counts[&i], indented_total);
            if let Some(d) = style.indent_pt() {
                let msg = if d.abs() < 0.01 {
                    Some(
                        "в абзацах немає абзацного відступу, має бути п'ять знаків (≈ 12,5 мм)"
                            .into(),
                    )
                } else if !(10.0 * MM - 0.3..=15.0 * MM + 0.3).contains(&d) {
                    Some(format!(
                        "абзацний відступ {}, має дорівнювати п'яти знакам (≈ 12,5 мм)",
                        mm(d)
                    ))
                } else {
                    None
                };
                if let Some(msg) = msg {
                    report(
                        n,
                        "par-indent",
                        style.indent.origin(),
                        ("par", &["first-line-indent"]),
                        msg,
                        None,
                    );
                }
            }
        }
        // За `all: false` typst не відступає перший абзац після заголовка,
        // рисунка, переліку
        for &i in &after_block {
            let style = &doc.styles[i];
            if style.indent_pt().is_some_and(|d| d > 0.01) && !style.indent_all {
                report(
                    (0, 0),
                    "par-indent",
                    style.indent.origin(),
                    ("par", &["first-line-indent"]),
                    "перший абзац після заголовка, рисунка чи переліку без відступу: задайте `first-line-indent: (amount: …, all: true)`".into(),
                    None,
                );
            }
        }

        for &i in &all {
            let style = &doc.styles[i];
            let n = (0, 0);
            if let Some(page) = style.page.known() {
                let near = |a: f64, b: f64| (a - b).abs() < 1.5;
                let msg = match (page.w, page.h) {
                    (Some(w), Some(h)) => {
                        let (s, l) = (w.min(h), w.max(h));
                        let a4 = near(s, 210.0 * MM) && near(l, 297.0 * MM);
                        let a3 = near(s, 297.0 * MM) && near(l, 420.0 * MM);
                        (!a4 && !a3).then(|| {
                            format!(
                                "формат {} × {}, має бути А4 (210 мм × 297 мм)",
                                mm(w),
                                mm(h)
                            )
                        })
                    }
                    _ => page
                        .name
                        .as_ref()
                        .map(|n| format!("формат «{n}», має бути А4 (210 мм × 297 мм)")),
                };
                if let Some(msg) = msg {
                    report(
                        n,
                        "page-size",
                        style.page.origin(),
                        ("page", &["paper", "width", "height", "flipped"]),
                        msg,
                        None,
                    );
                }
            }
            if let Some(m) = style.margins_pt() {
                let [(t, _), (r, _), (b, _), (l, _)] = m;
                let min = [20.0, 10.0, 20.0, 25.0];
                if let Some(bad) = (0..4).find(|&k| m[k].0 < min[k] * MM - 0.5) {
                    report(
                        n,
                        "page-margins",
                        m[bad].1,
                        ("page", &["margin"]),
                        format!(
                            "береги верхній {}, нижній {}, лівий {}, правий {}; рекомендовано не менше 20, 20, 25 і 10 мм",
                            mm(t),
                            mm(b),
                            mm(l),
                            mm(r)
                        ),
                        None,
                    );
                }
            }
        }
        for (code, loc, msg, fixes, count, total) in pending {
            // Налаштування діє не на весь основний текст (місцеве `set`)
            let msg = if count > 0 && count < total {
                format!("{msg} ({count} абзаців з {total})")
            } else {
                msg
            };
            self.add(code, Some(loc), msg, fixes);
        }
    }
}
