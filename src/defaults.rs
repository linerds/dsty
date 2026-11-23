//! Типові значення typst, з якими рушій ([`crate::style`], [`crate::look`])
//! відтворює оформлення там, де документ і шаблон нічого не задають.
//!
//! Значення виміряно на typst [`VERIFIED`] (тест `defaults_match_typst` у
//! `tests/full.rs` звіряє таблицю з typst з `PATH`). Для того, що
//! перевіряють правила, у цих версіях вони однакові: різниться лише товщина
//! лінії над виносками. Якщо нова версія змінить котресь, тест це покаже, і
//! тут з'явиться вибір за версією.

/// Версії typst, на яких таблицю перевірено.
pub const VERIFIED: &[&str] = &["0.14.2", "0.15.1"];

/// Кегль тексту, пт.
pub const TEXT_SIZE: f64 = 11.0;
pub const FONT: &str = "Libertinus Serif";
/// `par.leading`, em.
pub const LEADING_EM: f64 = 0.65;
pub const LANG: &str = "en";
pub const WEIGHT: i64 = 400;
pub const PAPER: &str = "a4";
/// `page.number-align`.
pub const PAGE_NUMBER_ALIGN: &str = "center+bottom";

/// Кегль заголовка рівня `level`, em (вбудоване show-правило heading).
pub fn heading_em(level: usize) -> f64 {
    match level {
        1 => 1.4,
        2 => 1.2,
        _ => 1.0,
    }
}
/// Накреслення заголовків.
pub const HEADING_WEIGHT: i64 = 700;

/// Текст виноски: кегль і `leading`, em.
pub const FOOTNOTE_SIZE_EM: f64 = 0.85;
pub const FOOTNOTE_LEADING_EM: f64 = 0.5;
/// Довжина лінії над виносками: частка ширини тексту.
pub const FOOTNOTE_LINE: f64 = 0.3;
pub const FOOTNOTE_NUMBERING: &str = "1";

pub const ENUM_NUMBERING: &str = "1.";
/// Позначки переліку за рівнем вкладення.
pub const LIST_MARKERS: [&str; 3] = ["•", "‣", "–"];

/// `math.equation.number-align`.
pub const EQUATION_NUMBER_ALIGN: &str = "end+horizon";
/// Назва над фігурою (`figure.caption.position: top`).
pub const CAPTION_TOP: bool = false;

/// Слово підпису фігури виду `kind` (`image`, `table`, `raw`) мовою `lang`.
pub fn supplement(lang: &str, kind: &str) -> Option<&'static str> {
    Some(match (lang, kind) {
        ("uk", "image") => "Рисунок",
        ("uk", "table") => "Таблиця",
        ("uk", "raw") => "Лістинг",
        ("en", "image") => "Figure",
        ("en", "table") => "Table",
        ("en", "raw") => "Listing",
        ("ru", "image") => "Рис.",
        ("ru", "table") => "Таблица",
        ("ru", "raw") => "Листинг",
        _ => return None,
    })
}

/// Знак між номером і назвою в підписі мовою `lang`.
pub fn caption_separator(lang: &str) -> Option<&'static str> {
    Some(match lang {
        "uk" | "en" => ": ",
        "ru" => ". ",
        _ => return None,
    })
}
