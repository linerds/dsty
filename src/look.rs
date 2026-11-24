//! Як друкуються елементи (заголовки, підписи, формули, переліки, виноски):
//! висновок із `set`- і show-правил документа й шаблону без компіляції.
//!
//! Власні властивості елемента (`heading.numbering`, `figure.caption.separator`)
//! беруться, як у typst: show-set правило на елементі чи його предку бере гору
//! над звичайним `set`, пізніше над ранішим. Show-функції обчислюються над
//! символьним елементом ([`Item::Hole`] замість його тексту): з того, що вони
//! друкують, видно налаштування тексту на самому елементі, текст перед ним
//! («ДОДАТОК А»), номер, відступ і розрив сторінки. Пізніша show-функція діє
//! першою, а налаштування в ранішій лежать глибше й переважають. Чого з коду
//! не видно, те [`Fact::Unknown`]: правило над ним мовчить.

use std::rc::Rc;

use crate::defaults;
use crate::eval::{
    Args, Effect, Elem, Evaluator, Item, Length, SetArg, SetEffect, Transform, Value,
};
use crate::style::{Origin, Style, TextProp};
use crate::work::{self, Align, At, Basis, Fact};

/// Елемент, для якого шукаються правила: назва й поля для `.where(…)`.
#[derive(Clone)]
pub struct Node {
    pub elem: &'static str,
    pub fields: Vec<(&'static str, Value)>,
}

impl Node {
    pub fn new(elem: &'static str, fields: Vec<(&'static str, Value)>) -> Self {
        Node { elem, fields }
    }

    fn matches(&self, effect: &crate::eval::ShowEffect) -> Option<bool> {
        effect.matches(self.elem, &self.fields)
    }
}

/// Значення власної властивості елемента.
pub enum Prop {
    /// Типове значення typst.
    Default,
    /// Задано правилом: значення й саме правило.
    Set(Value, Rc<SetEffect>),
    Unknown,
}

impl Prop {
    pub fn value(&self) -> Option<Option<&Value>> {
        match self {
            Prop::Default => Some(None),
            Prop::Set(v, _) => Some(Some(v)),
            Prop::Unknown => None,
        }
    }

    pub fn set(&self) -> Option<&Rc<SetEffect>> {
        match self {
            Prop::Set(_, s) => Some(s),
            _ => None,
        }
    }
}

/// Властивість `arg` елемента `target` (`figure.caption`) у `node` і його
/// предках `outer` (фігура для підпису). `own`: show-set на самому `target`
/// діє на нього (для підпису — ні: його поля typst бере з фігури).
pub fn prop(
    chain: &[Effect],
    target: &str,
    node: &Node,
    outer: &[Node],
    arg: &str,
    own: bool,
) -> Prop {
    let mut plain: Option<Rc<SetEffect>> = None;
    for effect in chain.iter().rev() {
        match effect {
            Effect::Opaque => return plain.map_or(Prop::Unknown, |s| from_set(&s, arg)),
            Effect::Set(s) if s.target == target => {
                if plain.is_none() {
                    if s.arg(arg).is_some() {
                        plain = Some(s.clone());
                    } else if s.open {
                        return Prop::Unknown;
                    }
                }
            }
            Effect::Show(sh) => {
                let hit = any_match(
                    outer
                        .iter()
                        .chain(own.then_some(node))
                        .map(|n| n.matches(sh)),
                );
                match (&sh.transform, hit) {
                    (_, Some(false)) => {}
                    (Transform::Set(s), Some(true))
                        if s.target == target && s.arg(arg).is_some() =>
                    {
                        return from_set(s, arg);
                    }
                    (Transform::Set(s), _)
                        if s.target == target && (s.open || s.arg(arg).is_some()) =>
                    {
                        return Prop::Unknown;
                    }
                    (Transform::Unknown, _) => return Prop::Unknown,
                    _ => {}
                }
            }
            _ => {}
        }
    }
    plain.map_or(Prop::Default, |s| from_set(&s, arg))
}

/// Чи діє правило хоч на один з елементів: `None`, якщо для котрогось не видно.
fn any_match(matches: impl Iterator<Item = Option<bool>>) -> Option<bool> {
    let mut out = Some(false);
    for m in matches {
        match m {
            Some(true) => return Some(true),
            None => out = None,
            Some(false) => {}
        }
    }
    out
}

fn from_set(s: &Rc<SetEffect>, arg: &str) -> Prop {
    match s.arg(arg) {
        Some(a) if !matches!(a.value, Value::Unknown) => Prop::Set(a.value.clone(), s.clone()),
        _ => Prop::Unknown,
    }
}

/// Що досягнуто, коли show-функції надруковано.
#[derive(Clone, Debug)]
pub enum Reached {
    /// Елемент друкується як typst (`it` без змін): діє його вбудоване оформлення.
    Builtin,
    /// Show-функція друкує текст елемента (`it.body`) сама.
    Hole,
    /// Show-функція друкує замість елемента новий того самого виду (з
    /// іншими полями): далі той друкується як typst.
    Replaced(Rc<Elem>),
}

impl Reached {
    /// Елемент друкується вбудованим оформленням typst (сам чи замінений).
    pub fn builtin(&self) -> bool {
        !matches!(self, Reached::Hole)
    }
}

/// Елемент після show-правил.
pub struct Rendered {
    /// Налаштування на самому тексті елемента, від зовнішніх до внутрішніх.
    pub inner: Vec<Effect>,
    /// `None`: де і як друкується елемент, з коду не видно.
    pub reached: Option<Reached>,
    /// Що друкується перед текстом елемента (у порядку друку).
    pub before: Vec<Item>,
    /// Show-функція, що оформлює елемент (для зауважень).
    pub site: Option<Rc<crate::eval::ShowEffect>>,
}

/// Обчислює show-правила елемента `node` (символьний елемент `sym`) з
/// предками `outer`.
pub fn render(
    ev: &mut Evaluator,
    chain: &[Effect],
    node: &Node,
    outer: &[Node],
    sym: Rc<Elem>,
) -> Rendered {
    let mut out = Rendered {
        inner: Vec::new(),
        reached: Some(Reached::Builtin),
        before: Vec::new(),
        site: None,
    };
    // Show-set: усі, що діють, у порядку правил (пізніше переважає)
    let mut funcs = Vec::new();
    for effect in chain.iter() {
        match effect {
            Effect::Opaque => {
                out.reached = None;
                return out;
            }
            Effect::Show(sh) => {
                let own = node.matches(sh);
                let any = any_match(outer.iter().map(|n| n.matches(sh)).chain([own]));
                match (&sh.transform, any, own) {
                    (_, Some(false), _) => {}
                    (Transform::Set(s), Some(true), _) => out.inner.push(Effect::Set(s.clone())),
                    (Transform::Func(f), _, Some(true)) => funcs.push((f.clone(), sh.clone())),
                    // Show-функція предка: що вона робить з елементом, видно
                    // окремо (для підпису — функція фігури)
                    (Transform::Func(_), _, Some(false)) => {}
                    _ => {
                        out.reached = None;
                        return out;
                    }
                }
            }
            _ => {}
        }
    }
    // Пізніша функція діє першою; `it` у її виводі обробляє раніша
    for (f, sh) in funcs.into_iter().rev() {
        if out.site.is_none() {
            out.site = Some(sh.clone());
        }
        let printed = ev.render(&f, sym.clone());
        let mut path = Vec::new();
        match find(ev, &printed.items(), &sym, &mut path, &mut out.before) {
            Some(Reached::Builtin) => out.inner.extend(path),
            Some(r) => {
                out.inner.extend(path);
                out.reached = Some(r);
                return out;
            }
            None => {
                out.reached = None;
                return out;
            }
        }
    }
    out
}

/// Елементи-контейнери: їхній вміст друкується всередині.
const CONTAINERS: &[&str] = &[
    "grid",
    "grid.cell",
    "grid.header",
    "grid.footer",
    "table",
    "table.cell",
    "table.header",
    "table.footer",
    "list",
    "enum",
    "terms",
    "par",
];

/// Друкований вміст у порядку друку: кожен лист з налаштуваннями на шляху до
/// нього. `context` обчислюється, вміст контейнерів (`grid`, клітинки)
/// розгортається, межі блоків стають [`Item::Parbreak`].
pub fn flatten(
    ev: &mut Evaluator,
    items: &[Item],
    path: &mut Vec<Effect>,
    out: &mut Vec<(Item, Rc<Vec<Effect>>)>,
) {
    if out.len() > 4000 {
        out.push((Item::Unknown, Rc::new(path.clone())));
        return;
    }
    for item in items {
        match item {
            Item::Styled(effects, inner) => {
                let depth = path.len();
                path.extend(effects.iter().cloned());
                flatten(ev, inner, path, out);
                path.truncate(depth);
            }
            Item::Block(inner) => {
                out.push((Item::Parbreak, Rc::new(path.clone())));
                flatten(ev, inner, path, out);
                out.push((Item::Parbreak, Rc::new(path.clone())));
            }
            Item::Ctx(_) => {
                let resolved = ev.resolve(std::slice::from_ref(item));
                flatten(ev, &resolved, path, out);
            }
            Item::Maybe(_) => out.push((Item::Unknown, Rc::new(path.clone()))),
            // Символьний елемент (з `Hole` замість тексту) не розгортається
            Item::Elem(e)
                if CONTAINERS.contains(&e.func.as_str())
                    && !matches!(e.named("body"), Some(Value::Content(b)) if matches!(b.as_slice(), [Item::Hole])) =>
            {
                out.push((Item::Parbreak, Rc::new(path.clone())));
                for v in e.args.pos.iter().chain(e.args.named.iter().map(|(_, v)| v)) {
                    if let Value::Content(inner) = v {
                        flatten(ev, inner, path, out);
                    }
                }
                out.push((Item::Parbreak, Rc::new(path.clone())));
            }
            other => out.push((other.clone(), Rc::new(path.clone()))),
        }
    }
}

/// Шукає елемент `sym` чи його текст у виводі show-функції: налаштування на
/// шляху до нього йдуть у `path`, а все, що друкується раніше, у `before`.
fn find(
    ev: &mut Evaluator,
    items: &[Item],
    sym: &Rc<Elem>,
    path: &mut Vec<Effect>,
    before: &mut Vec<Item>,
) -> Option<Reached> {
    let mut flat = Vec::new();
    flatten(ev, items, &mut Vec::new(), &mut flat);
    for (item, at) in flat {
        let reached = match &item {
            Item::Hole => Some(Reached::Hole),
            Item::Elem(e) if Rc::ptr_eq(e, sym) => Some(Reached::Builtin),
            // Новий елемент того самого виду замість `it` (`math.equation(it.body, numbering: none)`)
            Item::Elem(e) if e.func == sym.func => Some(Reached::Replaced(e.clone())),
            _ => None,
        };
        if let Some(r) = reached {
            path.extend(at.iter().cloned());
            return Some(r);
        }
        before.push(item);
    }
    None
}

/// Чи друкується посилання (`@мітка`) під налаштуваннями `style` так, як
/// типово в typst: підпис цілі й номер («Таблиця 1.2»). Show-правило `ref`
/// (у документі чи шаблоні), `set ref(…)` чи перетворення, якого не
/// простежити (невідомий шаблон): невідомо.
pub fn ref_form(style: &Style) -> Fact<bool> {
    for effect in style.chain.iter() {
        match effect {
            Effect::Opaque => return Fact::Unknown,
            Effect::Show(s) if s.elem == "ref" => return Fact::Unknown,
            Effect::Set(s) if s.target == "ref" => return Fact::Unknown,
            _ => {}
        }
    }
    Fact::typst(true, At::default("ref", &["supplement"]))
}

/// Символьний елемент для show-функцій: поля й текст ([`Item::Hole`]).
pub fn symbol(func: &str, fields: Vec<(&str, Value)>) -> Rc<Elem> {
    let mut named: Vec<(String, Value)> =
        fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
    if !named.iter().any(|(k, _)| k == "body") {
        named.push(("body".into(), Value::Content(Rc::new(vec![Item::Hole]))));
    }
    Rc::new(Elem {
        func: func.to_owned(),
        args: Args {
            named,
            ..Default::default()
        },
    })
}

/// Налаштування тексту на самому елементі: `base` (там, де стоїть елемент),
/// вбудоване оформлення елемента `builtin`, якщо він друкується як typst,
/// потім налаштування show-правил.
pub fn text_style(
    base: &Style,
    rendered: &Rendered,
    builtin: &[Effect],
    origin: &dyn Fn(&SetEffect, &SetArg) -> Origin,
) -> Option<Style> {
    let reached = rendered.reached.clone()?;
    let mut style = base.clone();
    if reached.builtin() && !builtin.is_empty() {
        for e in builtin {
            style.apply_effect(e, origin);
        }
        // Вбудоване оформлення typst — типові значення, а не правило коду
        for e in builtin {
            if let Effect::Set(s) = e {
                for a in &s.args {
                    match a.name.as_str() {
                        "size" => style.size = style.size.as_typst(At::default("text", &["size"])),
                        "weight" => style.weight = style.weight.as_typst(At::default("text", &[])),
                        "leading" => {
                            style.leading = style.leading.as_typst(At::default("par", &["leading"]))
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    for e in &rendered.inner {
        style.apply_effect(e, origin);
    }
    Some(style)
}

/// Місце show-правила.
pub fn show_origin(
    sh: &crate::eval::ShowEffect,
    origin: &dyn Fn(&SetEffect, &SetArg) -> Origin,
) -> Origin {
    let set = SetEffect {
        target: sh.elem.clone(),
        args: Vec::new(),
        open: false,
        path: sh.path.clone(),
        span: sh.span,
    };
    let arg = SetArg {
        name: String::new(),
        value: Value::None,
        span: typst_syntax::Span::detached(),
    };
    origin(&set, &arg)
}

/// Налаштування тексту в елементі `elem` (`outline.entry`) без вбудованого
/// оформлення.
pub fn inside(
    ev: &mut Evaluator,
    style: &Style,
    elem: &'static str,
    origin: &dyn Fn(&SetEffect, &SetArg) -> Origin,
) -> Option<Style> {
    let chain = style.chain.clone();
    let node = Node::new(elem, Vec::new());
    let r = render(ev, &chain, &node, &[], symbol(elem, Vec::new()));
    text_style(style, &r, &[], origin)
}

/// Номери сторінок з налаштувань `body` (основний текст) і `title`
/// (титульний аркуш).
pub fn pages(ev: &mut Evaluator, body: &Style, title: Option<&Style>) -> work::PageNumbers {
    // Що друкують колонтитули: номер (`counter(page).display()`) і де
    let part = |ev: &mut Evaluator,
                name: &str|
     -> Option<(Option<Option<String>>, Option<Origin>)> {
        let field = body
            .page_parts
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, f)| f);
        let (v, o) = match field {
            None => return Some((None, None)),
            Some(Fact::Unknown) => return None,
            Some(f @ Fact::Known(v, _)) => match f.origin() {
                Some(o) if !matches!(v, Value::Auto) => (v, o),
                _ => return Some((None, None)),
            },
        };
        let mut flat = Vec::new();
        flatten(ev, &v.items(), &mut Vec::new(), &mut flat);
        if flat
            .iter()
            .any(|(i, _)| matches!(i, Item::Unknown | Item::Source(_)))
        {
            return None;
        }
        let number = flat.iter().find(|(i, _)| matches!(i, Item::Number));
        Some((
            Some(
                number.map(|(_, path)| alignment(path).flatten().unwrap_or_else(|| "start".into())),
            ),
            Some(o.clone()),
        ))
    };
    let header = part(ev, "header");
    let footer = part(ev, "footer");
    let numbering = &body.page_numbering;
    // Номер, як його друкує нумерація (власний колонтитул без неї — «1»)
    let numbered = |ev: &mut Evaluator, numbering: &Fact<Value>| {
        let mut first = 2;
        let mut printed = page_number(ev, numbering, 2);
        for page in 3..=30 {
            if !matches!(printed.known(), Some(None)) {
                break;
            }
            first = page;
            printed = page_number(ev, numbering, page);
        }
        (printed, first)
    };
    let (mut printed, mut first) = numbered(ev, numbering);
    let side = |a: &str| if a.contains("top") { "top" } else { "bottom" };
    let align = match (&header, &footer) {
        // Номер друкує власний колонтитул (другого колонтитула може й не бути видно)
        (Some((Some(Some(a)), o)), _) | (_, Some((Some(Some(a)), o))) => {
            let edge = if matches!(header, Some((Some(Some(_)), _))) {
                "top"
            } else {
                "bottom"
            };
            if matches!(numbering.known(), Some(Value::None)) {
                let one = Fact::typst(Value::Str("1".into()), At::default("page", &["numbering"]));
                (printed, first) = numbered(ev, &one);
            }
            Fact::set(format!("{a}+{edge}"), o.clone().unwrap())
        }
        // Колонтитула не видно: номер може друкувати він
        (None, _) | (_, None) => {
            if matches!(printed.known(), Some(None)) {
                printed = Fact::Unknown;
            }
            Fact::Unknown
        }
        (Some((h, _)), Some((f, _))) => {
            // Типовий номер: на краю `number-align`, якщо там немає власного колонтитула
            match body.number_align.clone() {
                Fact::Unknown => Fact::Unknown,
                align => {
                    let at = align.known().map(|a| side(a)).unwrap_or("bottom");
                    let custom = if at == "top" {
                        h.is_some()
                    } else {
                        f.is_some()
                    };
                    if custom {
                        printed = Fact::typst(None, At::default("page", &["numbering"]));
                    }
                    align
                }
            }
        }
    };
    let title = title.map(|t| page_number(ev, &t.page_numbering, 1));
    // Лічильник виносок скидається на кожній сторінці (`background: counter(footnote).update(0)`)
    let mut footnote_reset = Some(false);
    for name in ["background", "foreground", "header", "footer"] {
        match body
            .page_parts
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, f)| f)
        {
            Some(f @ Fact::Known(v, _)) if f.origin().is_some() => {
                let mut flat = Vec::new();
                flatten(ev, &v.items(), &mut Vec::new(), &mut flat);
                if flat
                    .iter()
                    .any(|(i, _)| matches!(i, Item::Count(k, Value::Int(0)) if k == "footnote"))
                {
                    footnote_reset = Some(true);
                    break;
                }
                if flat.iter().any(|(i, _)| matches!(i, Item::Unknown)) {
                    footnote_reset = None;
                }
            }
            Some(Fact::Unknown) => footnote_reset = None,
            _ => {}
        }
    }
    let numbering = || At::default("page", &["numbering"]);
    work::PageNumbers {
        body: printed.placed(numbering),
        first_page: first,
        title: title.map(|t| t.placed(numbering)),
        align: align.placed(|| At::default("page", &["number-align"])),
        footnote_reset,
    }
}

/// Номер сторінки `page` за нумерацією `numbering`.
pub fn page_number(ev: &mut Evaluator, numbering: &Fact<Value>, page: i64) -> Fact<Option<String>> {
    let printed = |ev: &mut Evaluator, v: &Value| match v {
        Value::None => Some(None),
        // Функція отримує й кількість сторінок, шаблон з одним символом лічби — лише номер
        v => match ev.number(
            v,
            &[page, page][..if matches!(v, Value::Func(_)) { 2 } else { 1 }],
        ) {
            Value::None => Some(None),
            n => text_of(&n).map(|t| Some(t.trim().to_owned()).filter(|t| !t.is_empty())),
        },
    };
    match numbering.known().and_then(|v| printed(ev, v)) {
        Some(p) => numbering.derive(Some(p), || At::default("page", &["numbering"])),
        None => Fact::Unknown,
    }
}

/// Налаштування `set ціль(арг: значення)` без місця в коді: вбудоване
/// оформлення елемента.
pub fn builtin(target: &str, args: &[(&str, Value)]) -> Effect {
    Effect::Set(Rc::new(SetEffect {
        target: target.into(),
        args: args
            .iter()
            .map(|(k, v)| SetArg {
                name: (*k).into(),
                value: v.clone(),
                span: typst_syntax::Span::detached(),
            })
            .collect(),
        open: false,
        path: Default::default(),
        span: typst_syntax::Span::detached(),
    }))
}

/// Текст перед елементом: `Number` друкується як `number`; `None`, якщо там
/// є щось невідоме.
pub fn prefix(before: &[Item], number: Option<&str>) -> Option<String> {
    let mut out = String::new();
    for item in before {
        match item {
            Item::Text(t) => out.push_str(t),
            Item::Space | Item::H(_) => out.push(' '),
            Item::Parbreak | Item::Pagebreak => out.push('\n'),
            Item::Number => out.push_str(number?),
            Item::Label(_) | Item::Elem(_) => {}
            _ => return None,
        }
    }
    Some(out)
}

/// Чи є розрив сторінки перед елементом: `None`, якщо там є щось невідоме.
pub fn pagebreak(before: &[Item]) -> Option<bool> {
    if before.iter().any(|i| matches!(i, Item::Pagebreak)) {
        return Some(true);
    }
    (!before
        .iter()
        .any(|i| matches!(i, Item::Unknown | Item::Source(_) | Item::Body)))
    .then_some(false)
}

/// Горизонтальний проміжок на початку рядка перед елементом (`h(…)`,
/// `pad(left: …)`): довжина; 0, якщо його немає.
pub fn offset(before: &[Item]) -> Option<Length> {
    let mut at = Length::default();
    for item in before {
        match item {
            Item::Parbreak | Item::Pagebreak => at = Length::default(),
            Item::H(Value::Length(l)) => {
                at = Length {
                    pt: at.pt + l.pt,
                    em: at.em + l.em,
                }
            }
            Item::H(_) => return None,
            _ => {}
        }
    }
    Some(at)
}

/// Вирівнювання (`set align(…)`, `align(…)[…]`) серед налаштувань `effects`.
pub fn alignment(effects: &[Effect]) -> Option<Option<String>> {
    let mut out = None;
    for e in effects {
        match e {
            Effect::Set(s) if s.target == "align" => match s.arg("alignment").map(|a| &a.value) {
                Some(Value::Align(a)) => out = Some(a.clone()),
                Some(_) => return None,
                None => {}
            },
            Effect::Opaque => return None,
            _ => {}
        }
    }
    Some(out)
}

/// Текст значення-вмісту (`[Рисунок]`, `[ -- ]`); `None` для невідомого.
pub fn text_of(v: &Value) -> Option<String> {
    match v {
        Value::Str(s) => Some(s.clone()),
        Value::Content(items) => {
            let mut out = String::new();
            for item in items.iter() {
                match item {
                    Item::Text(t) => out.push_str(t),
                    Item::Space | Item::H(_) => out.push(' '),
                    Item::Styled(_, inner) | Item::Block(inner) => {
                        out.push_str(&text_of(&Value::Content(inner.clone()))?)
                    }
                    Item::Label(_) => {}
                    _ => return None,
                }
            }
            Some(out)
        }
        Value::None => Some(String::new()),
        _ => None,
    }
}

/// Властивість як [`Fact`]: типове значення typst (зауваження про нього
/// стоїть на `at`) чи задане правилом.
pub fn field<T>(
    prop: &Prop,
    default: T,
    at: At,
    map: impl Fn(&Value) -> Option<T>,
    origin: impl Fn(&Rc<SetEffect>) -> Origin,
) -> Fact<T> {
    match prop {
        Prop::Default => Fact::typst(default, at),
        Prop::Set(v, s) => match map(v) {
            Some(t) => Fact::set(t, origin(s)),
            None => Fact::Unknown,
        },
        Prop::Unknown => Fact::Unknown,
    }
}

/// Заголовок, як його друкують правила.
pub struct Heading {
    pub look: work::HeadingLook,
    /// Show-правило ставить розрив сторінки перед заголовком.
    pub pagebreak: Option<bool>,
    /// Лічильники, які show-правило змінює перед заголовком
    /// (`counter(figure.where(kind: image)).update(0)`).
    pub counts: Vec<(String, Value)>,
}

fn align_of(a: &Option<String>) -> Align {
    match a.as_deref() {
        Some(a) if a.contains("center") => Align::Center,
        Some(a) if a.contains("right") || a.contains("end") => Align::Right,
        _ => Align::Left,
    }
}

fn rule_or(origin: Option<&Origin>, fallback: impl FnOnce() -> At) -> At {
    origin.map_or_else(fallback, |o| At::Rule(o.clone()))
}

/// Оформлення заголовка рівня `level` з налаштуваннями `style`; `numbering`:
/// значення нумерації (`None`, якщо з коду не видно), `counters`: лічильник
/// заголовків на ньому.
pub fn heading(
    ev: &mut Evaluator,
    style: &Style,
    level: usize,
    numbering: Option<&Value>,
    counters: Option<&[i64]>,
    origin: &dyn Fn(&SetEffect, &SetArg) -> Origin,
) -> Heading {
    let chain = style.chain.clone();
    let node = Node::new(
        "heading",
        vec![
            ("level", Value::Int(level as i64)),
            ("depth", Value::Int(level as i64)),
            ("outlined", Value::Bool(true)),
        ],
    );
    let numbering_origin = prop(&chain, "heading", &node, &[], "numbering", true)
        .set()
        .and_then(|s| Some(origin(s, s.arg("numbering")?)));
    let number = match numbering {
        None => None,
        Some(Value::None) => Some(None),
        Some(v) => counters.and_then(|c| text_of(&ev.number(v, c)).map(Some)),
    };
    let supplement = match prop(&chain, "heading", &node, &[], "supplement", true) {
        Prop::Set(v, _) => v,
        Prop::Default => Value::Auto,
        Prop::Unknown => Value::Unknown,
    };
    let sym = symbol(
        "heading",
        vec![
            ("level", Value::Int(level as i64)),
            ("depth", Value::Int(level as i64)),
            ("numbering", numbering.cloned().unwrap_or(Value::Unknown)),
            ("supplement", supplement),
            ("outlined", Value::Bool(true)),
        ],
    );
    let rendered = render(ev, &chain, &node, &[], sym);
    let em = defaults::heading_em(level);
    let builtin = [builtin(
        "text",
        &[
            ("size", Value::Length(Length { pt: 0.0, em })),
            ("weight", Value::Int(defaults::HEADING_WEIGHT)),
        ],
    )];
    let text = text_style(style, &rendered, &builtin, origin);
    let printed = number.as_ref().and_then(|n| n.as_deref());
    let printed = printed.map(str::to_owned);
    let printed = printed.as_deref();
    let prefix = match rendered.reached {
        None => None,
        Some(Reached::Hole) => prefix(&rendered.before, printed),
        Some(Reached::Builtin | Reached::Replaced(_)) => {
            match (prefix(&rendered.before, printed), &number) {
                (Some(p), Some(Some(n))) => Some(format!("{p}{n} ")),
                (Some(p), Some(None)) => Some(p),
                _ => None,
            }
        }
    };
    let size = text.as_ref().and_then(|t| t.size_pt());
    let offset = rendered
        .reached
        .as_ref()
        .and_then(|_| offset(&rendered.before))
        .and_then(|l| match l.em {
            0.0 => Some(l.pt),
            _ => Some(l.resolve(size?)),
        });
    let align = rendered
        .reached
        .as_ref()
        .and_then(|_| alignment(&rendered.inner));
    // Типове оформлення стоїть на show-правилі заголовка, якщо воно є
    let site = rendered.site.as_ref().map(|sh| show_origin(sh, origin));
    let site = site.as_ref();
    let at_site = |fallback: At| rule_or(site, || fallback);
    let place = |p: TextProp| match p {
        TextProp::Size => at_site(At::default_in("heading", "text", &["size"])),
        TextProp::Weight => at_site(At::default_in("heading", "text", &["weight"])),
        TextProp::Font => At::default_in("heading", "text", &["font"]),
        TextProp::Hyphenate => At::default("text", &["hyphenate"]),
        _ => at_site(At::default_in("heading", "text", &[])),
    };
    let block = || at_site(At::default_in("heading", "block", &[]));
    let look = work::HeadingLook {
        text: text.as_ref().map(|s| s.text_look(&place)),
        number: match number {
            Some(n) => Fact::new(
                n,
                Basis::Computed,
                rule_or(numbering_origin.as_ref(), || {
                    At::default("heading", &["numbering"])
                }),
            ),
            None => Fact::Unknown,
        },
        prefix,
        align: match &align {
            Some(a) => Fact::new(align_of(a), Basis::Computed, block()),
            None => Fact::Unknown,
        },
        offset: match offset {
            Some(o) => Fact::new(o, Basis::Computed, block()),
            None => Fact::Unknown,
        },
    };
    Heading {
        look,
        pagebreak: rendered
            .reached
            .as_ref()
            .and_then(|_| pagebreak(&rendered.before)),
        counts: rendered
            .before
            .iter()
            .filter_map(|i| match i {
                Item::Count(k, v) => Some((k.clone(), v.clone())),
                _ => None,
            })
            .collect(),
    }
}

/// Оформлення підпису до того, як стане фактом моделі.
struct CaptionLook {
    supplement: Fact<String>,
    separator: Fact<String>,
    top: Fact<bool>,
    /// Нумерація фігур: шаблон, функція, `none`.
    numbering: Fact<Value>,
}

/// Нумерація елементів: `None` усередині, якщо вимкнено, інакше номер
/// першого елемента, як його надруковано.
fn numbered(ev: &mut Evaluator, f: &Fact<Value>, fallback: fn() -> At) -> Fact<Option<String>> {
    match f.known() {
        None => Fact::Unknown,
        Some(Value::None) => f.derive(Some(None), fallback),
        Some(n) => match text_of(&ev.number(n, &[1])) {
            Some(first) => Fact::new(Some(first), Basis::Computed, rule_or(f.origin(), fallback)),
            None => Fact::Unknown,
        },
    }
}

/// Як show-функції фігури друкують її частини.
struct FigureParts {
    /// Фігура друкується як typst.
    builtin: bool,
    /// Назва над тілом (для фігури, що друкує частини сама).
    top: Option<bool>,
    /// Show-функція, що друкує частини.
    site: Option<Rc<crate::eval::ShowEffect>>,
}

/// Show-функції фігури: де вони друкують назву (`cap`) відносно тіла.
fn figure_parts(
    ev: &mut Evaluator,
    chain: &[Effect],
    node: &Node,
    sym: &Rc<Elem>,
    cap: &Rc<Elem>,
) -> Option<FigureParts> {
    let mut funcs = Vec::new();
    for effect in chain {
        match effect {
            Effect::Opaque => return None,
            Effect::Show(sh) => match (node.matches(sh), &sh.transform) {
                (Some(false), _) | (Some(true), Transform::Set(_)) => {}
                (Some(true), Transform::Func(f)) => funcs.push((f.clone(), sh.clone())),
                _ => return None,
            },
            _ => {}
        }
    }
    for (f, sh) in funcs.iter().rev() {
        let printed = ev.render(f, sym.clone());
        let mut flat = Vec::new();
        flatten(ev, &printed.items(), &mut Vec::new(), &mut flat);
        let at = |pred: &dyn Fn(&Item) -> bool| flat.iter().position(|(i, _)| pred(i));
        if at(&|i| matches!(i, Item::Elem(e) if Rc::ptr_eq(e, sym))).is_some() {
            continue;
        }
        let c = at(&|i| matches!(i, Item::Elem(e) if Rc::ptr_eq(e, cap)))?;
        let b = at(&|i| matches!(i, Item::Hole));
        return Some(FigureParts {
            builtin: false,
            top: b.map(|b| c < b),
            site: Some(sh.clone()),
        });
    }
    Some(FigureParts {
        builtin: true,
        top: None,
        site: None,
    })
}

/// Оформлення підпису фігури виду `kind` (`image`, `table`) і нумерація
/// таких фігур; `explicit`: нумерація, задана в самій фігурі.
pub fn caption(
    ev: &mut Evaluator,
    style: &Style,
    kind: &str,
    explicit: Option<(Value, Origin)>,
    origin: &dyn Fn(&Rc<SetEffect>, &str) -> Origin,
) -> (work::CaptionLook, Fact<Value>) {
    let mut look = caption_parts(ev, style, kind, origin);
    if let Some((n, o)) = explicit {
        look.numbering = Fact::set(n, o);
    }
    let fact = work::CaptionLook {
        supplement: look.supplement,
        separator: look.separator,
        top: look.top,
        numbering: numbered(ev, &look.numbering, || {
            At::default("figure", &["numbering"])
        }),
    };
    (fact, look.numbering)
}

fn caption_parts(
    ev: &mut Evaluator,
    style: &Style,
    kind: &str,
    origin: &dyn Fn(&Rc<SetEffect>, &str) -> Origin,
) -> CaptionLook {
    let chain = style.chain.clone();
    let kind_value = Value::Func(Rc::new(crate::eval::Func::Builtin(kind.to_owned())));
    let fig = Node::new("figure", vec![("kind", kind_value.clone())]);
    let cap = Node::new("figure.caption", vec![("kind", kind_value.clone())]);
    let lang = style.lang.known().cloned();
    let default_word = || match lang.as_deref().and_then(|l| defaults::supplement(l, kind)) {
        Some(s) => Fact::typst(s.to_owned(), At::default("figure", &["supplement"])),
        None => Fact::Unknown,
    };
    let supplement = match prop(&chain, "figure", &fig, &[], "supplement", true) {
        Prop::Default | Prop::Set(Value::Auto, _) => default_word(),
        Prop::Set(v, s) => match text_of(&v) {
            Some(t) => Fact::set(t, origin(&s, "supplement")),
            None => Fact::Unknown,
        },
        Prop::Unknown => Fact::Unknown,
    };
    let separator = match prop(
        &chain,
        "figure.caption",
        &cap,
        std::slice::from_ref(&fig),
        "separator",
        false,
    ) {
        Prop::Default | Prop::Set(Value::Auto, _) => {
            match lang.as_deref().and_then(defaults::caption_separator) {
                Some(s) => Fact::typst(s.to_owned(), At::default("figure.caption", &["separator"])),
                None => Fact::Unknown,
            }
        }
        Prop::Set(v, s) => match text_of(&v) {
            Some(t) => Fact::set(t, origin(&s, "separator")),
            None => Fact::Unknown,
        },
        Prop::Unknown => Fact::Unknown,
    };
    let top = field(
        &prop(
            &chain,
            "figure.caption",
            &cap,
            std::slice::from_ref(&fig),
            "position",
            false,
        ),
        defaults::CAPTION_TOP,
        At::default("figure.caption", &["position"]),
        |v| match v {
            Value::Align(a) if a == "top" => Some(true),
            Value::Align(a) if a == "bottom" => Some(false),
            _ => None,
        },
        |s| origin(s, "position"),
    );
    let numbering = field(
        &prop(&chain, "figure", &fig, &[], "numbering", true),
        Value::Str("1".into()),
        At::default("figure", &["numbering"]),
        |v| Some(v.clone()),
        |s| origin(s, "numbering"),
    );
    let mut look = CaptionLook {
        supplement,
        separator,
        top,
        numbering,
    };
    let numbering_value = look.numbering.known().cloned().unwrap_or(Value::Unknown);
    let counter = Value::Counter(format!("figure:{kind}"));
    let known_str = |f: &Fact<String>| f.known().map_or(Value::Unknown, |s| Value::Str(s.clone()));
    let (supp_v, sep_v) = (known_str(&look.supplement), known_str(&look.separator));
    let position = Value::Align(
        if look.top.known() == Some(&true) {
            "top"
        } else {
            "bottom"
        }
        .into(),
    );
    let cap_fields = |body: Value| {
        vec![
            ("kind", kind_value.clone()),
            ("supplement", supp_v.clone()),
            ("separator", sep_v.clone()),
            ("numbering", numbering_value.clone()),
            ("counter", counter.clone()),
            ("position", position.clone()),
            ("body", body),
        ]
    };
    // Назва в фігурі: позначка замість тексту, щоб не сплутати з тілом
    let in_fig = symbol("figure.caption", cap_fields(Value::Str("\u{2}".into())));
    let fig_sym = symbol(
        "figure",
        vec![
            ("kind", kind_value.clone()),
            ("numbering", numbering_value.clone()),
            (
                "supplement",
                look.supplement
                    .known()
                    .map_or(Value::Unknown, |s| Value::Str(s.clone())),
            ),
            ("counter", counter.clone()),
            ("placement", Value::None),
            (
                "caption",
                Value::Content(Rc::new(vec![Item::Elem(in_fig.clone())])),
            ),
        ],
    );
    let Some(parts) = figure_parts(ev, &chain, &fig, &fig_sym, &in_fig) else {
        look.top = Fact::Unknown;
        look.supplement = Fact::Unknown;
        look.separator = Fact::Unknown;
        return look;
    };
    if !parts.builtin {
        // Фігуру друкує show-функція: де назва, видно з її виводу
        let site = parts.site.as_ref().map(|sh| {
            origin(
                &Rc::new(SetEffect {
                    target: "figure".into(),
                    args: Vec::new(),
                    open: false,
                    path: sh.path.clone(),
                    span: sh.span,
                }),
                "",
            )
        });
        look.top = match (parts.top, site) {
            (Some(t), Some(o)) => Fact::set(t, o),
            _ => Fact::Unknown,
        };
    }
    // Сам підпис: його show-функції
    let sym = symbol(
        "figure.caption",
        cap_fields(Value::Content(Rc::new(vec![Item::Hole]))),
    );
    let r = render(ev, &chain, &cap, std::slice::from_ref(&fig), sym);
    match &r.reached {
        Some(Reached::Builtin | Reached::Replaced(_)) => {
            if r.before.iter().any(|i| {
                !matches!(
                    i,
                    Item::Space | Item::Parbreak | Item::Label(_) | Item::H(_)
                )
            }) {
                look.supplement = Fact::Unknown;
                look.separator = Fact::Unknown;
            }
        }
        Some(Reached::Hole) => {
            // Підпис друкує show-функція: слово перед номером і знак після нього
            let text = prefix(&r.before, Some("\u{1}"));
            match text.as_deref().and_then(|t| t.split_once('\u{1}')) {
                Some((word, sep)) => {
                    let site = r.site.as_ref().map(|sh| {
                        origin(
                            &Rc::new(SetEffect {
                                target: "figure.caption".into(),
                                args: Vec::new(),
                                open: false,
                                path: sh.path.clone(),
                                span: sh.span,
                            }),
                            "",
                        )
                    });
                    let mk = |t: &str| match &site {
                        Some(o) => Fact::set(t.to_owned(), o.clone()),
                        None => Fact::Unknown,
                    };
                    look.supplement = mk(word.trim());
                    look.separator = mk(sep);
                }
                None => {
                    look.supplement = Fact::Unknown;
                    look.separator = Fact::Unknown;
                }
            }
        }
        None => {
            look.supplement = Fact::Unknown;
            look.separator = Fact::Unknown;
        }
    }
    look
}

/// Оформлення формули з міткою `label` (якщо є) і її нумерація, як її
/// друкують show-правила; `explicit`: нумерація, задана в самому виклику
/// `math.equation(numbering: …)`.
pub fn equation(
    ev: &mut Evaluator,
    style: &Style,
    label: Option<&str>,
    explicit: Option<(Value, Origin)>,
    origin: &dyn Fn(&Rc<SetEffect>, &str) -> Origin,
) -> (work::EquationLook, Fact<Value>) {
    let chain = style.chain.clone();
    let node = Node::new("math.equation", vec![("block", Value::Bool(true))]);
    let mut numbering = match explicit {
        Some((Value::Unknown, _)) => Fact::Unknown,
        Some((v, o)) => Fact::set(v, o),
        None => field(
            &prop(&chain, "math.equation", &node, &[], "numbering", true),
            Value::None,
            At::default("math.equation", &["numbering"]),
            |v| Some(v.clone()),
            |s| origin(s, "numbering"),
        ),
    };
    let number_align = field(
        &prop(&chain, "math.equation", &node, &[], "number-align", true),
        defaults::EQUATION_NUMBER_ALIGN.to_owned(),
        At::default("math.equation", &["number-align"]),
        |v| match v {
            Value::Align(a) => Some(a.clone()),
            _ => None,
        },
        |s| origin(s, "number-align"),
    );
    let mut fields = vec![
        ("block", Value::Bool(true)),
        (
            "numbering",
            numbering.known().cloned().unwrap_or(Value::Unknown),
        ),
    ];
    if let Some(l) = label {
        fields.push(("label", Value::Label(l.to_owned())));
    }
    let r = render(ev, &chain, &node, &[], symbol("math.equation", fields));
    // Show-правило друкує замість формули нову, без номера
    if let Some(Reached::Replaced(e)) = &r.reached
        && let Some(n) = e.named("numbering")
    {
        numbering = match &r.site {
            Some(sh) => Fact::set(
                n.clone(),
                origin(
                    &Rc::new(SetEffect {
                        target: "math.equation".into(),
                        args: Vec::new(),
                        open: false,
                        path: sh.path.clone(),
                        span: sh.span,
                    }),
                    "",
                ),
            ),
            None => Fact::Unknown,
        };
    }
    let align = match (&r.reached, alignment(&r.inner)) {
        (Some(_), Some(Some(a))) => Some(a),
        (Some(Reached::Builtin | Reached::Replaced(_)), Some(None)) => Some("center".into()),
        _ => None,
    };
    let look = work::EquationLook {
        numbering: numbered(ev, &numbering, || {
            At::default("math.equation", &["numbering"])
        }),
        number_align: number_align.placed(|| At::default("math.equation", &["number-align"])),
        align,
    };
    (look, numbering)
}

/// Оформлення переліку (`enumerated`: нумерованого); `nested`: вкладеного в
/// інший перелік. Позначка першої позиції, як її надруковано («–», «а)»), і
/// відступ позицій від берега, пт.
pub fn list(
    ev: &mut Evaluator,
    style: &Style,
    enumerated: bool,
    nested: bool,
    origin: &dyn Fn(&Rc<SetEffect>, &str) -> Origin,
) -> (Fact<String>, Fact<f64>) {
    let elem = if enumerated { "enum" } else { "list" };
    let unknown = (Fact::Unknown, Fact::Unknown);
    let mut chain = style.chain.to_vec();
    // Вкладений перелік: `set` у show-функції зовнішнього переліку діє на нього
    if nested {
        for parent in ["list", "enum"] {
            let r = render(
                ev,
                &chain,
                &Node::new(parent, Vec::new()),
                &[],
                symbol(parent, Vec::new()),
            );
            let Some(Reached::Builtin) = r.reached else {
                return unknown;
            };
            for e in r.inner {
                if matches!(&e, Effect::Set(s) if s.target == elem) {
                    chain.push(e);
                }
            }
        }
    }
    let node = Node::new(elem, Vec::new());
    let r = render(ev, &chain, &node, &[], symbol(elem, Vec::new()));
    if !matches!(r.reached, Some(Reached::Builtin)) {
        return unknown;
    }
    let marker_args: &'static [&'static str] = if enumerated {
        &["numbering"]
    } else {
        &["marker"]
    };
    let (arg, default) = if enumerated {
        ("numbering", defaults::ENUM_NUMBERING)
    } else if nested {
        ("marker", defaults::LIST_MARKERS[1])
    } else {
        ("marker", defaults::LIST_MARKERS[0])
    };
    let marker = match prop(&chain, elem, &node, &[], arg, true) {
        Prop::Default => Fact::typst(default.to_owned(), At::default(elem, marker_args)),
        Prop::Set(v, s) => {
            let shown = match (&v, enumerated) {
                (Value::Str(p), true) => crate::eval::apply_numbering(p, &[1]),
                (Value::Func(_), _) => text_of(&ev.number(&v, &[1])),
                (Value::Array(a), false) => {
                    a.get(usize::from(nested)).or(a.last()).and_then(text_of)
                }
                (v, _) => text_of(v),
            };
            match shown {
                Some(t) => Fact::set(t.trim().to_owned(), origin(&s, arg)),
                None => Fact::Unknown,
            }
        }
        Prop::Unknown => Fact::Unknown,
    };
    let indent = field(
        &prop(&chain, elem, &node, &[], "indent", true),
        Length::default(),
        At::default(elem, &["indent"]),
        |v| match v {
            Value::Length(l) => Some(*l),
            _ => None,
        },
        |s| origin(s, "indent"),
    );
    let size = style.size_pt();
    let indent_pt = indent.known().and_then(|i| crate::style::pt(i, size));
    let indent = indent.derive(indent_pt, || At::default(elem, &["indent"]));
    (marker, indent)
}

/// Оформлення виносок.
pub fn footnote(
    ev: &mut Evaluator,
    style: &Style,
    origin: &dyn Fn(&Rc<SetEffect>, &str) -> Origin,
    set_origin: &dyn Fn(&SetEffect, &SetArg) -> Origin,
) -> work::FootnoteLook {
    let chain = style.chain.clone();
    let node = Node::new("footnote", Vec::new());
    let entry = Node::new("footnote.entry", Vec::new());
    let marker = match prop(&chain, "footnote", &node, &[], "numbering", true) {
        Prop::Default => Fact::typst(
            defaults::FOOTNOTE_NUMBERING.to_owned(),
            At::default("footnote", &["numbering"]),
        ),
        Prop::Set(v, s) => match text_of(&ev.number(&v, &[1])) {
            Some(t) => Fact::set(t.trim().to_owned(), origin(&s, "numbering")),
            None => Fact::Unknown,
        },
        Prop::Unknown => Fact::Unknown,
    };
    let separator = field(
        &prop(
            &chain,
            "footnote.entry",
            &entry,
            std::slice::from_ref(&node),
            "separator",
            true,
        ),
        Some((Length::default(), defaults::FOOTNOTE_LINE)),
        At::default("footnote.entry", &["separator"]),
        |v| match v {
            Value::None => Some(None),
            Value::Content(items) => match items.as_slice() {
                [Item::Elem(e)] if e.func == "line" => match e.named("length") {
                    Some(Value::Length(l)) => Some(Some((*l, 0.0))),
                    Some(Value::Ratio(r)) => Some(Some((Length::default(), *r))),
                    None => Some(Some((Length::default(), 1.0))),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        },
        |s| origin(s, "separator"),
    );
    let sym = symbol("footnote.entry", Vec::new());
    let r = render(ev, &chain, &entry, &[node], sym);
    let builtin = [
        builtin(
            "text",
            &[(
                "size",
                Value::Length(Length {
                    pt: 0.0,
                    em: defaults::FOOTNOTE_SIZE_EM,
                }),
            )],
        ),
        builtin(
            "par",
            &[(
                "leading",
                Value::Length(Length {
                    pt: 0.0,
                    em: defaults::FOOTNOTE_LEADING_EM,
                }),
            )],
        ),
    ];
    let text = text_style(style, &r, &builtin, set_origin);
    // Довжина лінії: частка ширини тексту на сторінці (без берегів)
    let width = text.as_ref().and_then(|s| {
        let page = s.page.known()?;
        let m = s.margins_pt()?;
        Some(page.w? - m[1].0 - m[3].0)
    });
    let len = separator.known().and_then(|sep| match sep {
        None => Some(0.0),
        Some((l, r)) if l.em == 0.0 => width.map(|w| l.pt + r * w).or((*r == 0.0).then_some(l.pt)),
        Some(_) => None,
    });
    let place = |p: TextProp| match p {
        TextProp::Size => At::default_in("footnote.entry", "text", &["size"]),
        TextProp::Leading => At::default_in("footnote.entry", "par", &["leading"]),
        _ => At::default_in("footnote.entry", "text", &[]),
    };
    work::FootnoteLook {
        marker: marker.placed(|| At::default("footnote", &["numbering"])),
        separator: separator.derive(len, || At::default("footnote.entry", &["separator"])),
        text: text.as_ref().map(|s| s.text_look(&place)),
    }
}
