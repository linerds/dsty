//! Налаштування тексту й сторінки, що діють у точці документа: кегль,
//! шрифт, інтервал, абзацний відступ, формат і береги. Їх збирає модель із
//! `set`-правил документа й шаблонів ([`crate::eval`]), а статичні правила
//! перевіряють без компіляції.

use crate::eval::{Length, MM, SetArg, SetEffect, Value};
use crate::text::Loc;

/// Значення налаштування.
#[derive(Debug, Clone)]
pub enum Field<T> {
    /// Типове значення typst: ні документ, ні шаблон його не задають.
    Default(T),
    Set(T, Origin),
    /// Задано так, що з коду не видно (вираз, `context`, невідомий шаблон).
    Unknown,
}

impl<T> Field<T> {
    pub fn known(&self) -> Option<&T> {
        match self {
            Field::Default(v) | Field::Set(v, _) => Some(v),
            Field::Unknown => None,
        }
    }

    /// Де задано; `None` для типового значення.
    pub fn origin(&self) -> Option<&Origin> {
        match self {
            Field::Set(_, o) => Some(o),
            _ => None,
        }
    }
}

/// Звідки налаштування.
#[derive(Debug, Clone)]
pub struct Origin {
    /// `set`-правило в документі, а якщо воно в шаблоні поза документом, то
    /// рядок, що застосовує шаблон.
    pub loc: Loc,
    /// Значення в документі (для виправлення).
    pub value: Option<Loc>,
    /// Файл шаблону, якщо налаштування не в документі.
    pub template: Option<String>,
}

/// Розмір сторінки в пунктах (`None`: `auto`, сторінка без межі).
#[derive(Debug, Clone)]
pub struct PageSize {
    pub name: Option<String>,
    pub w: Option<f64>,
    pub h: Option<f64>,
}

impl PageSize {
    fn paper(name: &str) -> Self {
        let (w, h) = match paper_mm(name) {
            Some((w, h)) => (Some(w * MM), Some(h * MM)),
            None => (None, None),
        };
        PageSize {
            name: Some(name.to_owned()),
            w,
            h,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Style {
    /// Кегль, пт.
    pub size: Field<f64>,
    /// Перший шрифт переліку, як записано в коді.
    pub font: Field<String>,
    pub leading: Field<Length>,
    /// Змінено `top-edge` чи `bottom-edge`: крок рядків з `leading` не вивести.
    pub edges: bool,
    /// Абзацний відступ (`first-line-indent`).
    pub indent: Field<Length>,
    /// Відступ і в першому абзаці після заголовка, рисунка, переліку
    /// (`first-line-indent: (all: true)`); типово ні.
    pub indent_all: bool,
    pub page: Field<PageSize>,
    /// Береги: верхній, правий, нижній, лівий; `None`: `auto`.
    pub margins: [Field<Option<Length>>; 4],
}

impl Default for Style {
    /// Типові значення typst.
    fn default() -> Self {
        Style {
            size: Field::Default(11.0),
            font: Field::Default("Libertinus Serif".into()),
            leading: Field::Default(Length { pt: 0.0, em: 0.65 }),
            edges: false,
            indent: Field::Default(Length::default()),
            indent_all: false,
            page: Field::Default(PageSize::paper("a4")),
            margins: [
                Field::Default(None),
                Field::Default(None),
                Field::Default(None),
                Field::Default(None),
            ],
        }
    }
}

const TOP: usize = 0;
const RIGHT: usize = 1;
const BOTTOM: usize = 2;
const LEFT: usize = 3;

impl Style {
    /// Невідомий шаблон чи обгортка: діяти могло що завгодно.
    pub fn forget(&mut self) {
        self.size = Field::Unknown;
        self.font = Field::Unknown;
        self.leading = Field::Unknown;
        self.indent = Field::Unknown;
        self.page = Field::Unknown;
        self.margins = [
            Field::Unknown,
            Field::Unknown,
            Field::Unknown,
            Field::Unknown,
        ];
    }

    /// Застосовує `set`; `origin` каже, де задано аргумент.
    pub fn apply(&mut self, set: &SetEffect, origin: &dyn Fn(&SetArg) -> Origin) {
        match set.target.as_str() {
            "text" => {
                if set.open {
                    self.size = Field::Unknown;
                    self.font = Field::Unknown;
                }
                if let Some(a) = set.arg("size") {
                    self.size = match (&a.value, self.size.known()) {
                        (Value::Length(l), _) if l.em == 0.0 => Field::Set(l.pt, origin(a)),
                        (Value::Length(l), Some(&cur)) => Field::Set(l.resolve(cur), origin(a)),
                        _ => Field::Unknown,
                    };
                }
                if let Some(a) = set.arg("font") {
                    self.font = match font_name(&a.value) {
                        Some(name) => Field::Set(name, origin(a)),
                        None => Field::Unknown,
                    };
                }
                if set.arg("top-edge").is_some() || set.arg("bottom-edge").is_some() {
                    self.edges = true;
                }
            }
            "par" => {
                if set.open {
                    self.leading = Field::Unknown;
                    self.indent = Field::Unknown;
                }
                if let Some(a) = set.arg("leading") {
                    self.leading = match a.value {
                        Value::Length(l) => Field::Set(l, origin(a)),
                        _ => Field::Unknown,
                    };
                }
                if let Some(a) = set.arg("first-line-indent") {
                    // Довжина — те саме, що `(amount: …, all: false)`
                    let amount = match &a.value {
                        Value::Length(l) => Some((*l, false)),
                        v @ Value::Dict(_) => match (v.get("amount"), v.get("all")) {
                            (Some(Value::Length(l)), None) => Some((*l, false)),
                            (Some(Value::Length(l)), Some(Value::Bool(all))) => Some((*l, *all)),
                            _ => None,
                        },
                        _ => None,
                    };
                    if let Some((_, all)) = amount {
                        self.indent_all = all;
                    }
                    let amount = amount.map(|(l, _)| l);
                    self.indent = match amount {
                        Some(l) => Field::Set(l, origin(a)),
                        None => Field::Unknown,
                    };
                }
            }
            "page" => self.apply_page(set, origin),
            _ => {}
        }
    }

    fn apply_page(&mut self, set: &SetEffect, origin: &dyn Fn(&SetArg) -> Origin) {
        if set.open {
            self.page = Field::Unknown;
            self.margins = [
                Field::Unknown,
                Field::Unknown,
                Field::Unknown,
                Field::Unknown,
            ];
        }
        if let Some(a) = set.arg("paper") {
            self.page = match &a.value {
                Value::Str(name) => Field::Set(PageSize::paper(name), origin(a)),
                _ => Field::Unknown,
            };
        }
        for (name, height) in [("width", false), ("height", true)] {
            let Some(a) = set.arg(name) else { continue };
            let value = match a.value {
                Value::Length(l) if l.em == 0.0 => Some(Some(l.pt)),
                Value::Auto => Some(None),
                _ => None,
            };
            self.page = match (value, self.page.known()) {
                (Some(v), Some(page)) => {
                    let mut page = page.clone();
                    page.name = None;
                    if height {
                        page.h = v;
                    } else {
                        page.w = v;
                    }
                    Field::Set(page, origin(a))
                }
                _ => Field::Unknown,
            };
        }
        if let Some(a) = set.arg("margin") {
            let side = |v: &Value| match v {
                Value::Length(l) => Some(Some(*l)),
                Value::Auto => Some(None),
                _ => None,
            };
            let put = |this: &mut Self, i: usize, v: &Value| {
                this.margins[i] = match side(v) {
                    Some(v) => Field::Set(v, origin(a)),
                    None => Field::Unknown,
                };
            };
            match &a.value {
                Value::Dict(d) => {
                    // Спершу загальні ключі, потім окремі береги
                    for keys in [
                        &["rest"][..],
                        &["x", "y"],
                        &["top", "right", "bottom", "left", "inside", "outside"],
                    ] {
                        for (k, v) in d.iter().filter(|(k, _)| keys.contains(&k.as_str())) {
                            let sides: &[usize] = match k.as_str() {
                                "rest" => &[TOP, RIGHT, BOTTOM, LEFT],
                                "x" => &[LEFT, RIGHT],
                                "y" => &[TOP, BOTTOM],
                                "top" => &[TOP],
                                "bottom" => &[BOTTOM],
                                "left" | "inside" => &[LEFT],
                                _ => &[RIGHT],
                            };
                            for &i in sides {
                                put(self, i, v);
                            }
                        }
                    }
                    // Інші ключі з коду не видно
                    if d.iter().any(|(k, _)| {
                        !matches!(
                            k.as_str(),
                            "rest"
                                | "x"
                                | "y"
                                | "top"
                                | "right"
                                | "bottom"
                                | "left"
                                | "inside"
                                | "outside"
                        )
                    }) {
                        self.margins = [
                            Field::Unknown,
                            Field::Unknown,
                            Field::Unknown,
                            Field::Unknown,
                        ];
                    }
                }
                v => {
                    for i in 0..4 {
                        put(self, i, v);
                    }
                }
            }
        }
    }

    /// Кегль, пт.
    pub fn size_pt(&self) -> Option<f64> {
        self.size.known().copied()
    }

    /// `leading` в em, якщо з нього можна вивести крок рядків.
    pub fn leading_em(&self) -> Option<f64> {
        let l = self.leading.known()?;
        if self.edges {
            return None;
        }
        match l.pt {
            0.0 => Some(l.em),
            pt => Some(l.em + pt / self.size_pt()?),
        }
    }

    /// Абзацний відступ, пт.
    pub fn indent_pt(&self) -> Option<f64> {
        let l = self.indent.known()?;
        match l.em {
            0.0 => Some(l.pt),
            _ => Some(l.resolve(self.size_pt()?)),
        }
    }

    /// Береги в пунктах (верхній, правий, нижній, лівий) і звідки кожен.
    pub fn margins_pt(&self) -> Option<[(f64, Option<&Origin>); 4]> {
        let page = self.page.known()?;
        let auto = 2.5 / 21.0 * page.w?.min(page.h?);
        let mut out = [(0.0, None); 4];
        for (i, field) in self.margins.iter().enumerate() {
            let value = field.known()?;
            let pt = match value {
                Some(l) if l.em == 0.0 => l.pt,
                Some(l) => l.resolve(*self.size.known()?),
                None => auto,
            };
            out[i] = (
                pt,
                field.origin().or(if value.is_none() {
                    self.page.origin()
                } else {
                    None
                }),
            );
        }
        Some(out)
    }
}

/// Перший шрифт: `"Arial"`, `("Times New Roman", …)`, `(name: "…", covers: …)`.
fn font_name(v: &Value) -> Option<String> {
    match v {
        Value::Str(s) => Some(s.clone()),
        Value::Array(a) => font_name(a.first()?),
        Value::Dict(_) => match v.get("name")? {
            Value::Str(s) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Розміри форматів паперу typst, мм.
fn paper_mm(name: &str) -> Option<(f64, f64)> {
    Some(match name {
        "a0" => (841.0, 1189.0),
        "a1" => (594.0, 841.0),
        "a2" => (420.0, 594.0),
        "a3" => (297.0, 420.0),
        "a4" => (210.0, 297.0),
        "a5" => (148.0, 210.0),
        "a6" => (105.0, 148.0),
        "a7" => (74.0, 105.0),
        "iso-b4" => (250.0, 353.0),
        "iso-b5" => (176.0, 250.0),
        "iso-b6" => (125.0, 176.0),
        "us-letter" => (215.9, 279.4),
        "us-legal" => (215.9, 355.6),
        "us-executive" => (184.15, 266.7),
        "us-tabloid" => (279.4, 431.8),
        _ => return None,
    })
}
