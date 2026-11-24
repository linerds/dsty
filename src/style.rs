//! Налаштування тексту й сторінки, що діють у точці документа: кегль,
//! шрифт, інтервал, абзацний відступ, формат і береги. Їх збирає модель із
//! `set`-правил документа й шаблонів ([`crate::eval`]), а статичні правила
//! перевіряють без компіляції. Кожне налаштування — [`Fact`]: задане
//! правилом коду, типове typst чи невідоме.

use std::rc::Rc;

use crate::defaults;
use crate::eval::{Effect, Length, MM, SetArg, SetEffect, Value};
use crate::text::Loc;
use crate::work::{At, Basis, Fact, Look, Paper, TextLook};

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
    pub size: Fact<f64>,
    /// Перший шрифт переліку, як записано в коді.
    pub font: Fact<String>,
    pub leading: Fact<Length>,
    /// Змінено `top-edge` чи `bottom-edge`: крок рядків з `leading` не вивести.
    pub edges: bool,
    /// Абзацний відступ (`first-line-indent`).
    pub indent: Fact<Length>,
    /// Відступ і в першому абзаці після заголовка, рисунка, переліку
    /// (`first-line-indent: (all: true)`); типово ні.
    pub indent_all: bool,
    pub page: Fact<PageSize>,
    /// Береги: верхній, правий, нижній, лівий; `None`: `auto`.
    pub margins: [Fact<Option<Length>>; 4],
    /// Мова тексту (`lang`): від неї типові підписи («Рисунок»).
    pub lang: Fact<String>,
    pub justify: Fact<bool>,
    /// `hyphenate`; `None`: `auto` (переноси, якщо текст вирівняно за шириною).
    pub hyphenate: Fact<Option<bool>>,
    /// Товщина шрифту (400 звичайний, 700 напівжирний).
    pub weight: Fact<i64>,
    pub italic: Fact<bool>,
    /// Чорний колір тексту.
    pub black: Fact<bool>,
    /// Текст перетворено на великі літери (`upper`).
    pub upper: Fact<bool>,
    /// Нумерація сторінок (`numbering`): шаблон, функція чи `none`.
    pub page_numbering: Fact<Value>,
    /// Розміщення номера сторінки (`number-align`), складники через `+`.
    pub number_align: Fact<String>,
    /// Власний колонтитул (`header`, `footer`): номер може друкувати він.
    pub page_header: Fact<bool>,
    /// Вміст сторінки поза текстом: `header`, `footer`, `background`,
    /// `foreground` (`None`: типовий).
    pub page_parts: Rc<Vec<(String, Fact<Value>)>>,
    /// `set` елементів і show-правила в порядку дії: з них виводиться
    /// оформлення заголовків, підписів, формул, переліків, виносок.
    pub chain: Rc<Vec<Effect>>,
}

impl Default for Style {
    /// Типові значення typst.
    fn default() -> Self {
        Style {
            size: Fact::typst(defaults::TEXT_SIZE, At::default("text", &["size"])),
            font: Fact::typst(defaults::FONT.into(), At::default("text", &["font"])),
            leading: Fact::typst(
                Length {
                    pt: 0.0,
                    em: defaults::LEADING_EM,
                },
                At::default("par", &["leading"]),
            ),
            edges: false,
            indent: Fact::typst(
                Length::default(),
                At::default("par", &["first-line-indent"]),
            ),
            indent_all: false,
            page: Fact::typst(
                PageSize::paper(defaults::PAPER),
                At::default("page", &["paper", "width", "height", "flipped"]),
            ),
            margins: std::array::from_fn(|_| Fact::typst(None, At::default("page", &["margin"]))),
            lang: Fact::typst(defaults::LANG.into(), At::default("text", &["lang"])),
            justify: Fact::typst(false, At::default("par", &["justify"])),
            hyphenate: Fact::typst(None, At::default("text", &["hyphenate"])),
            weight: Fact::typst(defaults::WEIGHT, At::default("text", &[])),
            italic: Fact::typst(false, At::default("text", &["style"])),
            black: Fact::typst(true, At::default("text", &["fill"])),
            upper: Fact::typst(false, At::default("text", &[])),
            page_numbering: Fact::typst(Value::None, At::default("page", &["numbering"])),
            number_align: Fact::typst(
                defaults::PAGE_NUMBER_ALIGN.into(),
                At::default("page", &["number-align"]),
            ),
            page_header: Fact::typst(false, At::default("page", &["header"])),
            page_parts: Rc::new(Vec::new()),
            chain: Rc::new(Vec::new()),
        }
    }
}

const TOP: usize = 0;
const RIGHT: usize = 1;
const BOTTOM: usize = 2;
const LEFT: usize = 3;

/// Властивість тексту: для вибору місця типового значення.
#[derive(Clone, Copy)]
pub enum TextProp {
    Size,
    Font,
    Weight,
    Italic,
    Black,
    Upper,
    Leading,
    Hyphenate,
}

/// Основний текст: типові значення стоять на `set text`/`set par`.
fn body_place(p: TextProp) -> At {
    match p {
        TextProp::Size => At::default("text", &["size"]),
        TextProp::Font => At::default("text", &["font"]),
        TextProp::Italic => At::default("text", &["style"]),
        TextProp::Black => At::default("text", &["fill"]),
        TextProp::Leading => At::default("par", &["leading"]),
        TextProp::Hyphenate => At::default("text", &["hyphenate"]),
        TextProp::Weight | TextProp::Upper => At::default("text", &[]),
    }
}

/// Підстава відомого значення; невідоме вважається типовим.
fn basis<T>(f: &Fact<T>) -> Basis {
    f.basis().unwrap_or(Basis::Default)
}

/// Довжина в пунктах за кеглю `size`.
pub fn pt(l: &Length, size: Option<f64>) -> Option<f64> {
    match l.em {
        0.0 => Some(l.pt),
        _ => size.map(|s| l.resolve(s)),
    }
}

impl Style {
    /// Оформлення тексту; `place` каже, де стоїть зауваження про типове
    /// значення (у заголовку — show-правило заголовка, в основному тексті —
    /// `set text`).
    pub fn text_look(&self, place: &dyn Fn(TextProp) -> At) -> TextLook {
        let hyphenate = match self.hyphenate.known() {
            None => Fact::Unknown,
            Some(Some(_)) => self
                .hyphenate
                .derive(self.hyphenate.known().copied().flatten(), || {
                    place(TextProp::Hyphenate)
                }),
            // `auto`: переноси, якщо текст вирівняно за шириною
            Some(None) => match self.justify.known() {
                None => Fact::Unknown,
                Some(j) => Fact::new(
                    *j,
                    basis(&self.justify).join(basis(&self.hyphenate)),
                    match self.justify.origin().or(self.hyphenate.origin()) {
                        Some(o) => At::Rule(o.clone()),
                        None => place(TextProp::Hyphenate),
                    },
                ),
            },
        };
        TextLook {
            size: self.size.placed(|| place(TextProp::Size)),
            font: self.font.placed(|| place(TextProp::Font)),
            weight: self.weight.placed(|| place(TextProp::Weight)),
            italic: self.italic.placed(|| place(TextProp::Italic)),
            black: self.black.placed(|| place(TextProp::Black)),
            // Регістр задає show-функція (`upper(it)`): її місце знає `place`
            upper: match self.upper.known() {
                Some(&u) => Fact::new(u, basis(&self.upper), place(TextProp::Upper)),
                None => Fact::Unknown,
            },
            leading: self
                .leading
                .derive(self.leading_em(), || place(TextProp::Leading)),
            hyphenate,
        }
    }

    /// Оформлення абзацу й сторінки.
    pub fn look(&self) -> Look {
        let margins = match self.margins_pt() {
            Some(m) => m.map(|(pt, o)| match o {
                Some(o) => Fact::set(pt, o.clone()),
                None => Fact::typst(pt, At::default("page", &["margin"])),
            }),
            None => [Fact::Unknown, Fact::Unknown, Fact::Unknown, Fact::Unknown],
        };
        Look {
            text: self.text_look(&body_place),
            indent: self.indent.derive(self.indent_pt(), || {
                At::default("par", &["first-line-indent"])
            }),
            indent_all: self.indent_all,
            paper: self.page.derive(
                self.page.known().map(|p| Paper {
                    name: p.name.clone(),
                    w: p.w,
                    h: p.h,
                }),
                || At::default("page", &["paper", "width", "height", "flipped"]),
            ),
            margins,
        }
    }

    /// Невідомий шаблон чи обгортка: діяти могло що завгодно.
    pub fn forget(&mut self) {
        self.size = Fact::Unknown;
        self.font = Fact::Unknown;
        self.leading = Fact::Unknown;
        self.indent = Fact::Unknown;
        self.page = Fact::Unknown;
        self.margins = [Fact::Unknown, Fact::Unknown, Fact::Unknown, Fact::Unknown];
        self.lang = Fact::Unknown;
        self.justify = Fact::Unknown;
        self.hyphenate = Fact::Unknown;
        self.weight = Fact::Unknown;
        self.italic = Fact::Unknown;
        self.black = Fact::Unknown;
        self.upper = Fact::Unknown;
        self.page_numbering = Fact::Unknown;
        self.number_align = Fact::Unknown;
        self.page_header = Fact::Unknown;
        self.page_parts = Rc::new(
            ["header", "footer", "background", "foreground"]
                .iter()
                .map(|n| ((*n).to_owned(), Fact::Unknown))
                .collect(),
        );
        Rc::make_mut(&mut self.chain).push(Effect::Opaque);
    }

    /// Застосовує налаштування вмісту.
    pub fn apply_effect(
        &mut self,
        effect: &Effect,
        origin: &dyn Fn(&SetEffect, &SetArg) -> Origin,
    ) {
        match effect {
            Effect::Opaque => self.forget(),
            Effect::Set(set) => {
                if !matches!(set.target.as_str(), "text" | "par" | "page") {
                    Rc::make_mut(&mut self.chain).push(effect.clone());
                }
                self.apply(set, &|a: &SetArg| origin(set, a))
            }
            Effect::Show(_) => Rc::make_mut(&mut self.chain).push(effect.clone()),
            Effect::Written(_) => {}
            Effect::Case(up) => {
                self.upper = Fact::set(
                    *up,
                    Origin {
                        loc: Loc {
                            file: 0,
                            range: 0..0,
                        },
                        value: None,
                        template: None,
                    },
                )
            }
        }
    }

    /// Застосовує `set`; `origin` каже, де задано аргумент.
    pub fn apply(&mut self, set: &SetEffect, origin: &dyn Fn(&SetArg) -> Origin) {
        match set.target.as_str() {
            "text" => {
                if set.open {
                    self.size = Fact::Unknown;
                    self.font = Fact::Unknown;
                    self.lang = Fact::Unknown;
                    self.hyphenate = Fact::Unknown;
                    self.weight = Fact::Unknown;
                    self.italic = Fact::Unknown;
                    self.black = Fact::Unknown;
                }
                if let Some(a) = set.arg("lang") {
                    self.lang = match &a.value {
                        Value::Str(l) => Fact::set(l.to_lowercase(), origin(a)),
                        _ => Fact::Unknown,
                    };
                }
                if let Some(a) = set.arg("hyphenate") {
                    self.hyphenate = match a.value {
                        Value::Bool(b) => Fact::set(Some(b), origin(a)),
                        Value::Auto => Fact::set(None, origin(a)),
                        _ => Fact::Unknown,
                    };
                }
                if let Some(a) = set.arg("weight") {
                    self.weight = match weight(&a.value) {
                        Some(w) => Fact::set(w, origin(a)),
                        None => Fact::Unknown,
                    };
                }
                if let Some(a) = set.arg("style") {
                    self.italic = match &a.value {
                        Value::Str(s) => Fact::set(s != "normal", origin(a)),
                        _ => Fact::Unknown,
                    };
                }
                if let Some(a) = set.arg("fill") {
                    self.black = match a.value {
                        Value::Color(b) => Fact::set(b, origin(a)),
                        _ => Fact::Unknown,
                    };
                }
                if let Some(a) = set.arg("size") {
                    self.size = match (&a.value, self.size.known()) {
                        (Value::Length(l), _) if l.em == 0.0 => Fact::set(l.pt, origin(a)),
                        (Value::Length(l), Some(&cur)) => Fact::set(l.resolve(cur), origin(a)),
                        _ => Fact::Unknown,
                    };
                }
                if let Some(a) = set.arg("font") {
                    self.font = match font_name(&a.value) {
                        Some(name) => Fact::set(name, origin(a)),
                        None => Fact::Unknown,
                    };
                }
                if set.arg("top-edge").is_some() || set.arg("bottom-edge").is_some() {
                    self.edges = true;
                }
            }
            "par" => {
                if set.open {
                    self.leading = Fact::Unknown;
                    self.indent = Fact::Unknown;
                    self.justify = Fact::Unknown;
                }
                if let Some(a) = set.arg("justify") {
                    self.justify = match a.value {
                        Value::Bool(b) => Fact::set(b, origin(a)),
                        _ => Fact::Unknown,
                    };
                }
                if let Some(a) = set.arg("leading") {
                    self.leading = match a.value {
                        Value::Length(l) => Fact::set(l, origin(a)),
                        _ => Fact::Unknown,
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
                        Some(l) => Fact::set(l, origin(a)),
                        None => Fact::Unknown,
                    };
                }
            }
            "page" => self.apply_page(set, origin),
            _ => {}
        }
    }

    fn apply_page(&mut self, set: &SetEffect, origin: &dyn Fn(&SetArg) -> Origin) {
        if let Some(a) = set.arg("numbering") {
            self.page_numbering = match &a.value {
                v @ (Value::None | Value::Str(_) | Value::Func(_)) => {
                    Fact::set(v.clone(), origin(a))
                }
                _ => Fact::Unknown,
            };
        }
        if let Some(a) = set.arg("number-align") {
            self.number_align = match &a.value {
                Value::Align(v) => Fact::set(v.clone(), origin(a)),
                _ => Fact::Unknown,
            };
        }
        for name in ["header", "footer"] {
            if let Some(a) = set.arg(name) {
                self.page_header = match a.value {
                    Value::None | Value::Auto => Fact::set(false, origin(a)),
                    _ => Fact::set(true, origin(a)),
                };
            }
        }
        for name in ["header", "footer", "background", "foreground"] {
            if let Some(a) = set.arg(name) {
                let parts = Rc::make_mut(&mut self.page_parts);
                parts.retain(|(n, _)| n != name);
                parts.push((name.to_owned(), Fact::set(a.value.clone(), origin(a))));
            }
        }
        if set.open {
            self.page_numbering = Fact::Unknown;
            self.number_align = Fact::Unknown;
            self.page_header = Fact::Unknown;
            self.page_parts = Rc::new(
                ["header", "footer", "background", "foreground"]
                    .iter()
                    .map(|n| ((*n).to_owned(), Fact::Unknown))
                    .collect(),
            );
            self.page = Fact::Unknown;
            self.margins = [Fact::Unknown, Fact::Unknown, Fact::Unknown, Fact::Unknown];
        }
        if let Some(a) = set.arg("paper") {
            self.page = match &a.value {
                Value::Str(name) => Fact::set(PageSize::paper(name), origin(a)),
                _ => Fact::Unknown,
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
                    Fact::set(page, origin(a))
                }
                _ => Fact::Unknown,
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
                    Some(v) => Fact::set(v, origin(a)),
                    None => Fact::Unknown,
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
                        self.margins = [Fact::Unknown, Fact::Unknown, Fact::Unknown, Fact::Unknown];
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

/// Товщина шрифту: `"bold"`, `700`.
pub fn weight(v: &Value) -> Option<i64> {
    match v {
        Value::Int(i) => Some(*i),
        Value::Str(s) => Some(match s.as_str() {
            "thin" => 100,
            "extralight" => 200,
            "light" => 300,
            "regular" => 400,
            "medium" => 500,
            "semibold" => 600,
            "bold" => 700,
            "extrabold" => 800,
            "black" => 900,
            _ => return None,
        }),
        _ => None,
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
