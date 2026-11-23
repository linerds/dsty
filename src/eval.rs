//! Невеликий інтерпретатор Typst над синтаксичним деревом. Обчислює вміст,
//! який дають шаблони й обгортки: які елементи вони створюють (`heading`,
//! `figure`, `table`, `bibliography`, `outline`), з якими налаштуваннями
//! (`set`) і де стоїть вміст документа. Шаблон розпізнається лише за тим, що
//! він робить, а не за назвою.
//!
//! Вміст документа (аргументи-вміст його викликів, решта документа після
//! `#show: шаблон`) не обчислюється, а лишається посиланням на свій вузол коду
//! ([`Item::Source`], [`Item::Body`]): модель обходить його сама, з місцями в
//! коді.
//!
//! Обчислюється лише те, що не залежить від розкладки: літерали, довжини й
//! арифметика з ними, `let`, `import` (локальні файли й пакунки з тек typst),
//! функції та `.with(…)`, `toml(…)`, вбудовані елементи. Решта (`context`,
//! лічильники, стан, більшість вбудованих функцій) дає [`Value::Unknown`]:
//! правило над таким значенням мовчить, а не вважає його порушенням.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use typst_syntax::ast::{self, AstNode, Expr};
use typst_syntax::{Source, Span, SyntaxNode};

use crate::model::normalize;

/// Довжина: абсолютна частина в пунктах і відносна до кегля (`em`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Length {
    pub pt: f64,
    pub em: f64,
}

impl Length {
    pub fn pt(pt: f64) -> Self {
        Length { pt, em: 0.0 }
    }

    /// Довжина в пунктах за кегля `size`.
    pub fn resolve(self, size: f64) -> f64 {
        self.pt + self.em * size
    }

    fn scale(self, k: f64) -> Self {
        Length {
            pt: self.pt * k,
            em: self.em * k,
        }
    }
}

pub const MM: f64 = 72.0 / 25.4;

#[derive(Debug, Clone)]
pub enum Value {
    /// Значення з коду не дізнатися.
    Unknown,
    /// Обчислення не завершується (`panic`): такої гілки не буває.
    Never,
    None,
    Auto,
    Bool(bool),
    Int(i64),
    Float(f64),
    Length(Length),
    Ratio(f64),
    Str(String),
    Label(String),
    Content(Rc<Vec<Item>>),
    Array(Rc<Vec<Value>>),
    Dict(Rc<Vec<(String, Value)>>),
    Func(Rc<Func>),
    /// Модуль: номер файлу в [`Evaluator`].
    Module(usize),
    /// Аргументи, зібрані приймачем `..args`.
    Args(Rc<Args>),
    /// Тип (`type(x)`): його назва.
    Type(&'static str),
    /// Значення вбудованого типу, якого не обчислюємо (`path(…)`):
    /// відомо лише, що воно є.
    Thing,
    /// Вирівнювання: складники через `+` (`right+top`), як у коді.
    Align(String),
    /// Колір: чи він чорний.
    Color(bool),
    /// `counter(ціль)`: ціль (`heading`, `page`, `footnote`, `figure:image`, …).
    Counter(String),
    /// `елемент.where(…)`: елемент і умови.
    Selector(Rc<(String, Vec<(String, Value)>)>),
}

/// Частина вмісту.
#[derive(Debug, Clone)]
pub enum Item {
    /// Текст із коду шаблону.
    Text(String),
    Space,
    /// Межа абзацу (`parbreak`, розрив сторінки, вертикальний проміжок).
    Parbreak,
    /// Вміст документа: вузол його коду, який модель обходить сама.
    Source(SyntaxNode),
    /// Решта документа після `#show: шаблон`.
    Body,
    Label(String),
    Elem(Rc<Elem>),
    /// Блок (`align`, `block`, `pad`): окремо від абзаців навколо.
    Block(Rc<Vec<Item>>),
    /// Вміст під налаштуваннями (`set` перед ним).
    Styled(Rc<Vec<Effect>>, Rc<Vec<Item>>),
    /// Вміст гілки, умови якої з коду не видно: лише щоб знати, які
    /// елементи можуть бути (перелік джерел).
    Maybe(Rc<Vec<Item>>),
    /// Вміст, якого не обчислити.
    Unknown,
    /// Розрив сторінки (`pagebreak`).
    Pagebreak,
    /// Горизонтальний проміжок (`h(…)`, лівий відступ `pad`).
    H(Value),
    /// Власний текст елемента в show-функції (`it.body`).
    Hole,
    /// Номер елемента, як його друкує лічильник (`counter(…).display(…)`).
    Number,
    /// `counter(ціль).update(значення)`; `Unknown` для `step` і функцій.
    Count(String, Value),
    /// `context …`: обчислюється там, де стоїть ([`Evaluator::resolve`]).
    Ctx(Rc<Deferred>),
}

/// Відкладений вираз `context`: вузол і значення, які він захопив.
#[derive(Debug)]
pub struct Deferred {
    file: usize,
    node: SyntaxNode,
    captured: Rc<Vec<(String, Value)>>,
}

/// Вбудований елемент і його аргументи.
#[derive(Debug)]
pub struct Elem {
    /// `heading`, `figure`, `table`, `table.header`, `bibliography`, …
    pub func: String,
    pub args: Args,
}

impl Elem {
    pub fn named(&self, name: &str) -> Option<&Value> {
        self.args
            .named
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v)
    }

    /// Вміст елемента: `body:` чи останній позиційний аргумент-вміст.
    pub fn body(&self) -> Option<&Value> {
        self.named("body").or_else(|| {
            self.args
                .pos
                .iter()
                .rev()
                .find(|v| matches!(v, Value::Content(_) | Value::Str(_)))
        })
    }
}

impl Value {
    /// Текст рядка чи вмісту з самого тексту.
    pub fn text(&self) -> Option<String> {
        match self {
            Value::Str(s) => Some(s.clone()),
            Value::Content(items) => {
                let mut out = String::new();
                for item in items.iter() {
                    match item {
                        Item::Text(t) => out.push_str(t),
                        Item::Space | Item::H(_) => out.push(' '),
                        Item::Styled(_, inner) | Item::Block(inner) => {
                            out.push_str(&Value::Content(inner.clone()).text()?)
                        }
                        _ => return None,
                    }
                }
                Some(out.trim().to_owned())
            }
            _ => None,
        }
    }

    /// Значення ключа словника.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Dict(d) => d.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Вміст, яким значення друкується.
    pub fn items(&self) -> Vec<Item> {
        match self {
            Value::Content(items) => items.to_vec(),
            Value::Str(s) => vec![Item::Text(s.clone())],
            Value::Int(i) => vec![Item::Text(i.to_string())],
            Value::Float(f) => vec![Item::Text(f.to_string())],
            Value::Label(l) => vec![Item::Label(l.clone())],
            Value::None | Value::Auto | Value::Never => Vec::new(),
            _ => vec![Item::Unknown],
        }
    }
}

fn content(items: Vec<Item>) -> Value {
    Value::Content(Rc::new(items))
}

fn styled(effects: Vec<Effect>, rest: Value) -> Value {
    if matches!(rest, Value::Never) {
        return rest;
    }
    content(vec![Item::Styled(Rc::new(effects), Rc::new(rest.items()))])
}

/// Чи є у вмісті `context`.
fn needs_resolve(item: &Item) -> bool {
    match item {
        Item::Ctx(_) => true,
        Item::Styled(_, inner) | Item::Block(inner) | Item::Maybe(inner) => {
            inner.iter().any(needs_resolve)
        }
        _ => false,
    }
}

/// Чи є у вмісті вміст документа.
fn has_source(items: &[Item]) -> bool {
    items.iter().any(|item| match item {
        Item::Source(_) | Item::Body => true,
        Item::Styled(e, _) if matches!(e.first(), Some(Effect::Written(_))) => true,
        Item::Block(inner) | Item::Styled(_, inner) | Item::Maybe(inner) => has_source(inner),
        Item::Elem(e) => e
            .args
            .pos
            .iter()
            .chain(e.args.named.iter().map(|(_, v)| v))
            .any(|v| matches!(v, Value::Content(c) if has_source(c))),
        _ => false,
    })
}

#[derive(Debug)]
pub enum Func {
    /// Функція з коду: вузол `Closure` і значення, які вона захопила.
    Closure {
        file: usize,
        node: SyntaxNode,
        captured: Rc<Vec<(String, Value)>>,
    },
    /// `f.with(…)`.
    With(Rc<Func>, Args),
    /// Вбудована функція чи елемент (`text`, `figure.caption`).
    Builtin(String),
}

#[derive(Debug, Clone, Default)]
pub struct Args {
    pub pos: Vec<Value>,
    pub named: Vec<(String, Value)>,
    /// Є розгортання (`..x`), значення якого невідоме: будь-який аргумент
    /// міг прийти з нього.
    pub open: bool,
}

impl Args {
    fn then(&self, more: &Args) -> Args {
        let mut out = self.clone();
        out.pos.extend(more.pos.iter().cloned());
        out.named.extend(more.named.iter().cloned());
        out.open |= more.open;
        out
    }
}

/// Налаштування вмісту.
#[derive(Debug, Clone)]
pub enum Effect {
    Set(Rc<SetEffect>),
    /// Щось, чого не простежити (невідоме `show: …`): діяти могло що завгодно.
    Opaque,
    /// `show селектор: …` для елемента.
    Show(Rc<ShowEffect>),
    /// `upper(…)` (`true`) чи `lower(…)`.
    Case(bool),
    /// Вміст набрано в коді проєкту ([`Written`]): обхід документа бере його
    /// звідти, а не з обчисленого вмісту.
    Written(Rc<Written>),
}

/// Блок вмісту з коду проєкту (`#let назва = […]` верхнього рівня): де він
/// і вузол `[…]`. Документ, що його друкує, друкує цей код.
#[derive(Debug)]
pub struct Written {
    pub path: PathBuf,
    pub node: SyntaxNode,
}

/// `show елемент.where(…): перетворення`.
#[derive(Debug)]
pub struct ShowEffect {
    /// `heading`, `figure`, `figure.caption`, `math.equation`, `footnote.entry`, …
    pub elem: String,
    /// Умови `.where(…)`.
    pub filter: Vec<(String, Value)>,
    pub transform: Transform,
    /// Файл і вузол правила.
    pub path: PathBuf,
    pub span: Span,
}

#[derive(Debug)]
pub enum Transform {
    /// `show …: set ціль(…)`: налаштування всередині елемента.
    Set(Rc<SetEffect>),
    /// `show …: it => …` чи інша функція.
    Func(Value),
    /// Перетворення, якого не обчислити.
    Unknown,
}

impl ShowEffect {
    /// Чи діє правило на елемент `elem` з полями `fields`; `None`, якщо
    /// умови з коду не видно.
    pub fn matches(&self, elem: &str, fields: &[(&str, Value)]) -> Option<bool> {
        if self.elem != elem {
            return Some(false);
        }
        let mut known = true;
        for (k, want) in &self.filter {
            match fields.iter().find(|(f, _)| f == k) {
                Some((_, have)) => match same(have, want) {
                    Some(true) => {}
                    Some(false) => return Some(false),
                    None => known = false,
                },
                None => known = false,
            }
        }
        known.then_some(true)
    }
}

/// `set ціль(…)`, застосоване до вмісту.
#[derive(Debug)]
pub struct SetEffect {
    /// `text`, `par`, `page`, `heading`, `footnote.entry`.
    pub target: String,
    pub args: Vec<SetArg>,
    /// Позиційні аргументи чи розгортання: які властивості вони задають, не видно.
    pub open: bool,
    /// Файл і вузол правила.
    pub path: PathBuf,
    pub span: Span,
}

#[derive(Debug)]
pub struct SetArg {
    pub name: String,
    pub value: Value,
    /// Вузол значення (для виправлення).
    pub span: Span,
}

impl SetEffect {
    pub fn arg(&self, name: &str) -> Option<&SetArg> {
        self.args.iter().rev().find(|a| a.name == name)
    }
}

/// Що викликає документ.
pub enum Callee {
    /// Вбудована функція чи елемент (без `std.`).
    Builtin(String),
    /// Функція з коду (шаблону, пакунка, документа).
    User,
    /// Функцію не знайти.
    Unknown,
}

#[derive(Clone)]
struct Env {
    file: usize,
    locals: Vec<(String, Value)>,
}

#[derive(PartialEq)]
enum State {
    Fresh,
    Loading,
    Done,
}

struct File {
    path: PathBuf,
    source: Source,
    /// Корінь для шляхів `/…`: тека проєкту чи пакунка.
    root: PathBuf,
    state: State,
    bindings: HashMap<String, Value>,
}

/// Розібрані файли пакунків: між перевірками (у LSP на кожну зміну) вони не
/// змінюються, а розбір великого пакунка (lilaq, cetz) коштує мілісекунди.
/// Текст звіряється, тож відредагований `@local`-пакунок розбирається наново.
static PACKAGES: std::sync::LazyLock<std::sync::Mutex<HashMap<PathBuf, Source>>> =
    std::sync::LazyLock::new(Default::default);

fn parsed_package(path: &Path, text: String) -> Source {
    let mut cache = PACKAGES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(source) = cache.get(path)
        && source.text() == text
    {
        return source.clone();
    }
    let source = crate::model::parse(text);
    cache.insert(path.to_path_buf(), source.clone());
    source
}

/// Межа роботи на одну побудову моделі: інтерпретатор не має зависати на
/// циклах і глибокій рекурсії.
const FUEL: usize = 400_000;
const MAX_DEPTH: usize = 48;
const MAX_LOOP: usize = 1000;

pub struct Evaluator<'a> {
    root: PathBuf,
    load: &'a dyn Fn(&Path) -> Option<String>,
    files: Vec<File>,
    by_path: HashMap<PathBuf, usize>,
    fuel: usize,
    depth: usize,
    /// Значення `return`, яке піднімається до виклику функції.
    ret: Option<Value>,
    /// Лічильник заголовків там, де зараз модель (`counter(heading).get()`).
    heading: Option<Vec<i64>>,
    /// Скільки посилань на кожну мітку (`query(ref.where(target: …))`);
    /// `None`, поки документ не обійдено.
    refs: Option<HashMap<String, usize>>,
    /// Глибина обчислення `context`.
    resolving: usize,
    /// Змінні циклу, у якому зараз обхід документа: (файл, назва, значення).
    doc_locals: Vec<(usize, String, Value)>,
    /// Обчислюється значення верхнього рівня файлу проєкту: (файл, глибина
    /// викликів). Блоки вмісту саме цього файлу й рівня позначаються [`Written`].
    written: Option<(usize, usize)>,
}

impl<'a> Evaluator<'a> {
    pub fn new(root: &Path, load: &'a dyn Fn(&Path) -> Option<String>) -> Self {
        Evaluator {
            root: root.to_path_buf(),
            load,
            files: Vec::new(),
            by_path: HashMap::new(),
            fuel: FUEL,
            depth: 0,
            ret: None,
            heading: None,
            refs: None,
            resolving: 0,
            doc_locals: Vec::new(),
            written: None,
        }
    }

    /// Обхід документа заходить у тіло циклу у файлі `path`: змінна `name`
    /// має значення `value` для всіх виразів цього файлу до [`Self::unbind_local`].
    pub fn bind_local(&mut self, path: &Path, name: &str, value: Value) {
        let root = self.root.clone();
        if let Some(file) = self.file_id(path, &root) {
            self.doc_locals.push((file, name.to_owned(), value));
        }
    }

    /// Знімає останню змінну, задану [`Self::bind_local`].
    pub fn unbind_local(&mut self) {
        self.doc_locals.pop();
    }

    /// Файл, уже розібраний моделлю: щоб не розбирати вдруге.
    pub fn adopt(&mut self, path: &Path, source: Source) {
        if !self.by_path.contains_key(path) {
            let root = self.root.clone();
            self.push(path.to_path_buf(), source, root);
        }
    }

    fn push(&mut self, path: PathBuf, source: Source, root: PathBuf) -> usize {
        self.files.push(File {
            path: path.clone(),
            source,
            root,
            state: State::Fresh,
            bindings: HashMap::new(),
        });
        let id = self.files.len() - 1;
        self.by_path.insert(path, id);
        id
    }

    fn file_id(&mut self, path: &Path, root: &Path) -> Option<usize> {
        let path = normalize(path);
        if let Some(&id) = self.by_path.get(&path) {
            return Some(id);
        }
        let text = (self.load)(&path)?;
        let source = if root == self.root {
            crate::model::parse(text)
        } else {
            parsed_package(&path, text)
        };
        Some(self.push(path, source, root.to_path_buf()))
    }

    fn tick(&mut self) -> bool {
        if self.fuel == 0 {
            return false;
        }
        self.fuel -= 1;
        true
    }

    fn env_of(&mut self, path: &Path) -> Option<Env> {
        let root = self.root.clone();
        let file = self.file_id(path, &root)?;
        self.ensure_module(file);
        let locals = self
            .doc_locals
            .iter()
            .filter(|(f, _, _)| *f == file)
            .map(|(_, n, v)| (n.clone(), v.clone()))
            .collect();
        Some(Env { file, locals })
    }

    // --- Відкриті методи для моделі ----------------------------------------------

    /// Значення виразу з файлу `path` (у його області видимості верхнього рівня).
    pub fn eval_in(&mut self, path: &Path, expr: Expr) -> Value {
        match self.env_of(path) {
            Some(mut env) => self.eval(expr, &mut env),
            None => Value::Unknown,
        }
    }

    /// Що викликає `call` у файлі `path`.
    pub fn callee_in(&mut self, path: &Path, call: ast::FuncCall) -> Callee {
        let Some(mut env) = self.env_of(path) else {
            return Callee::Unknown;
        };
        let value = match self.callee(call.callee(), call.args(), &mut env, false) {
            Ok(v) => v,
            Err(v) => v,
        };
        match &value {
            Value::Func(f) => match &**f {
                Func::Builtin(name) => Callee::Builtin(name.clone()),
                _ => Callee::User,
            },
            _ => Callee::Unknown,
        }
    }

    /// Вираз документа з файлу `path`: виклики обчислюються, а вміст документа
    /// в аргументах лишається посиланням на код ([`Item::Source`]).
    pub fn eval_doc_in(&mut self, path: &Path, expr: Expr) -> Value {
        match self.env_of(path) {
            Some(mut env) => self.eval_doc(expr, &mut env),
            None => Value::Unknown,
        }
    }

    /// `#show: transform` у файлі `path`: вміст, у якому решта документа
    /// стоїть як [`Item::Body`].
    pub fn show_in(&mut self, path: &Path, transform: Expr) -> Value {
        let Some(mut env) = self.env_of(path) else {
            return Value::Unknown;
        };
        let func = self.eval_doc(transform, &mut env);
        self.show_with(&func, content(vec![Item::Body]), env.file)
    }

    /// Лічильник заголовків там, де зараз обхід документа.
    pub fn set_heading(&mut self, counter: Option<Vec<i64>>) {
        self.heading = counter;
    }

    /// Посилання документа: скільки разів на кожну мітку.
    pub fn set_refs(&mut self, refs: HashMap<String, usize>) {
        self.refs = Some(refs);
    }

    /// Обчислює `context` у вмісті там, де він стоїть.
    pub fn resolve(&mut self, items: &[Item]) -> Vec<Item> {
        let mut out = Vec::new();
        for item in items {
            match item {
                Item::Ctx(d) => {
                    if self.resolving >= 8 || !self.tick() {
                        out.push(Item::Unknown);
                        continue;
                    }
                    self.ensure_module(d.file);
                    let mut env = Env {
                        file: d.file,
                        locals: d.captured.to_vec(),
                    };
                    self.resolving += 1;
                    let saved = self.ret.take();
                    let v = match d.node.cast::<Expr>() {
                        Some(e) => self.eval(e, &mut env),
                        None => Value::Unknown,
                    };
                    let v = self.ret.take().unwrap_or(v);
                    self.ret = saved;
                    let items = self.resolve(&v.items());
                    self.resolving -= 1;
                    out.extend(items);
                }
                Item::Styled(e, inner) => {
                    out.push(Item::Styled(e.clone(), Rc::new(self.resolve(inner))));
                }
                Item::Block(inner) => out.push(Item::Block(Rc::new(self.resolve(inner)))),
                Item::Maybe(inner) => out.push(Item::Maybe(Rc::new(self.resolve(inner)))),
                other => out.push(other.clone()),
            }
        }
        out
    }

    /// Значення з обчисленим `context`.
    pub fn resolved(&mut self, v: Value) -> Value {
        match &v {
            Value::Content(items) if items.iter().any(needs_resolve) => {
                content(self.resolve(items))
            }
            _ => v,
        }
    }

    /// `show селектор: …` у файлі `path` як налаштування елемента.
    pub fn show_effect_in(&mut self, path: &Path, show: ast::ShowRule) -> Option<Effect> {
        let mut env = self.env_of(path)?;
        self.show_effect(show, &mut env)
    }

    /// Що друкує show-функція `func` для елемента `elem`.
    pub fn render(&mut self, func: &Value, elem: Rc<Elem>) -> Value {
        let it = content(vec![Item::Elem(elem)]);
        let out = match func {
            Value::Func(_) => {
                let args = Args {
                    pos: vec![it],
                    ..Default::default()
                };
                self.call_value(func, args, 0, Span::detached())
            }
            v => v.clone(),
        };
        self.resolved(out)
    }

    /// Значення функції нумерації чи шаблону для номерів `nums`.
    pub fn number(&mut self, numbering: &Value, nums: &[i64]) -> Value {
        match numbering {
            Value::Str(p) => apply_numbering(p, nums).map_or(Value::Unknown, Value::Str),
            Value::Func(_) => {
                let args = Args {
                    pos: nums.iter().map(|&n| Value::Int(n)).collect(),
                    ..Default::default()
                };
                let v = self.call_value(numbering, args, 0, Span::detached());
                self.resolved(v)
            }
            Value::None => Value::None,
            _ => Value::Unknown,
        }
    }

    /// `set`-правило у файлі `path` як налаштування.
    pub fn set_in(&mut self, path: &Path, set: ast::SetRule) -> Option<Rc<SetEffect>> {
        let mut env = self.env_of(path)?;
        match self.set_effect(set, &mut env)? {
            Effect::Set(s) => Some(s),
            _ => None,
        }
    }

    /// Розібраний код файлу `path`, якщо його вже прочитано.
    pub fn source_of(&self, path: &Path) -> Option<Source> {
        let id = *self.by_path.get(&normalize(path))?;
        Some(self.files[id].source.clone())
    }

    /// Вбудований `text(…)[…]` чи `par(…)[…]` у файлі `path` як налаштування
    /// для свого вмісту.
    pub fn call_set_in(&mut self, path: &Path, call: ast::FuncCall) -> Option<Rc<SetEffect>> {
        let mut env = self.env_of(path)?;
        let target = path_of(call.callee())?;
        let mut args = Vec::new();
        let mut open = false;
        for arg in call.args().items() {
            match arg {
                ast::Arg::Named(n) => {
                    let value = self.eval(n.expr(), &mut env);
                    args.push(SetArg {
                        name: n.name().as_str().to_owned(),
                        value,
                        span: n.expr().span(),
                    });
                }
                ast::Arg::Spread(_) => open = true,
                ast::Arg::Pos(_) => {}
            }
        }
        Some(Rc::new(SetEffect {
            target: target.trim_start_matches("std.").to_owned(),
            args,
            open,
            path: self.files[env.file].path.clone(),
            span: call.span(),
        }))
    }

    // --- Модулі --------------------------------------------------------------------

    fn ensure_module(&mut self, file: usize) {
        if self.files[file].state != State::Fresh {
            return;
        }
        self.files[file].state = State::Loading;
        let source = self.files[file].source.clone();
        let mut env = Env {
            file,
            locals: Vec::new(),
        };
        // Вміст, набраний у значеннях верхнього рівня файлу проєкту (`#let
        // назва = […]`), пам'ятає свій код ([`Written`])
        let project = self.files[file].root == self.root;
        if let Some(markup) = source.root().cast::<ast::Markup>() {
            for expr in markup.exprs() {
                let bound = match expr {
                    Expr::LetBinding(l) => {
                        let saved = self.written.take();
                        if project {
                            self.written = Some((file, self.depth));
                        }
                        let bound = self.let_binding(l, &mut env);
                        self.written = saved;
                        bound
                    }
                    Expr::ModuleImport(i) => self.import(i, &env),
                    _ => continue,
                };
                for (name, value) in bound {
                    self.files[file].bindings.insert(name, value);
                }
            }
        }
        self.files[file].state = State::Done;
    }

    fn let_binding(&mut self, l: ast::LetBinding, env: &mut Env) -> Vec<(String, Value)> {
        let bound = match l.kind() {
            ast::LetBindingKind::Closure(name) => {
                let Some(init) = l.init() else {
                    return Vec::new();
                };
                let func = Func::Closure {
                    file: env.file,
                    node: init.to_untyped().clone(),
                    captured: Rc::new(env.locals.clone()),
                };
                vec![(name.as_str().to_owned(), Value::Func(Rc::new(func)))]
            }
            ast::LetBindingKind::Normal(ast::Pattern::Normal(Expr::Ident(name))) => {
                let value = match l.init() {
                    Some(init) => self.eval(init, env),
                    None => Value::None,
                };
                vec![(name.as_str().to_owned(), value)]
            }
            // `let (a, b) = …`, `let (x: a, ..rest) = …`
            ast::LetBindingKind::Normal(pattern) => {
                let value = match l.init() {
                    Some(init) => self.eval(init, env),
                    None => Value::None,
                };
                destructure(pattern, &value)
            }
        };
        env.locals.extend(bound.iter().cloned());
        bound
    }

    /// Модуль, який імпортує `imp`.
    fn import_module(&mut self, imp: ast::ModuleImport, env: &Env) -> Option<usize> {
        let module = match imp.source() {
            Expr::Str(s) => {
                let spec = s.get();
                if spec.starts_with('@') {
                    self.package(&spec)
                } else {
                    let base = &self.files[env.file];
                    let path = match spec.strip_prefix('/') {
                        Some(rel) => base.root.join(rel),
                        None => base.path.parent().unwrap_or(&base.root).join(spec.as_str()),
                    };
                    let root = base.root.clone();
                    self.file_id(&path, &root)
                }
            }
            e => match self.eval(e, &mut env.clone()) {
                Value::Module(m) => Some(m),
                _ => None,
            },
        };
        if let Some(m) = module {
            self.ensure_module(m);
        }
        module
    }

    /// Імпорт: прив'язки, які він додає.
    fn import(&mut self, imp: ast::ModuleImport, env: &Env) -> Vec<(String, Value)> {
        let module = self.import_module(imp, env);
        let get = |this: &Self, m: Option<usize>, name: &str| -> Value {
            m.and_then(|m| this.files[m].bindings.get(name).cloned())
                .unwrap_or(Value::Unknown)
        };
        let mut out = Vec::new();
        let whole = module.map_or(Value::Unknown, Value::Module);
        if let Some(name) = imp.new_name() {
            out.push((name.as_str().to_owned(), whole.clone()));
        }
        match imp.imports() {
            None => {
                if imp.new_name().is_none()
                    && let Ok(name) = imp.bare_name()
                {
                    out.push((name.to_string(), whole));
                }
            }
            Some(ast::Imports::Wildcard) => {
                if let Some(m) = module {
                    let mut all: Vec<(String, Value)> = self.files[m]
                        .bindings
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    all.sort_by(|a, b| a.0.cmp(&b.0));
                    out.extend(all);
                }
            }
            Some(ast::Imports::Items(items)) => {
                for item in items.iter() {
                    let mut current = module;
                    let mut value = Value::Unknown;
                    for (i, part) in item.path().iter().enumerate() {
                        value = get(self, current, part.as_str());
                        current = match &value {
                            Value::Module(m) => {
                                self.ensure_module(*m);
                                Some(*m)
                            }
                            _ if i + 1 < item.path().iter().count() => None,
                            _ => current,
                        };
                    }
                    out.push((item.bound_name().as_str().to_owned(), value));
                }
            }
        }
        out
    }

    /// `@простір/назва:версія`: точка входу пакунка з тек typst.
    fn package(&mut self, spec: &str) -> Option<usize> {
        let (path, version) = spec.strip_prefix('@')?.split_once(':')?;
        let (namespace, name) = path.split_once('/')?;
        for base in package_dirs(namespace) {
            let dir = base.join(namespace).join(name).join(version);
            let Some(manifest) = (self.load)(&dir.join("typst.toml")) else {
                continue;
            };
            let entry = toml::from_str::<toml::Table>(&manifest)
                .ok()
                .and_then(|t| {
                    t.get("package")?
                        .get("entrypoint")?
                        .as_str()
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| "lib.typ".into());
            return self.file_id(&dir.join(entry.trim_start_matches('/')), &dir);
        }
        None
    }

    // --- Вирази --------------------------------------------------------------------

    fn lookup(&mut self, env: &Env, name: &str) -> Value {
        if let Some((_, v)) = env.locals.iter().rev().find(|(n, _)| n == name) {
            return v.clone();
        }
        if let Some(v) = self.files[env.file].bindings.get(name) {
            return v.clone();
        }
        if is_builtin(name) {
            return Value::Func(Rc::new(Func::Builtin(name.to_owned())));
        }
        if ALIGNMENTS.contains(&name) {
            return Value::Align(name.to_owned());
        }
        if name == "black" {
            return Value::Color(true);
        }
        if COLORS.contains(&name) {
            return Value::Color(false);
        }
        Value::Unknown
    }

    fn eval(&mut self, expr: Expr, env: &mut Env) -> Value {
        if !self.tick() {
            return Value::Unknown;
        }
        match expr {
            Expr::None(_) => Value::None,
            Expr::Auto(_) => Value::Auto,
            Expr::Bool(b) => Value::Bool(b.get()),
            Expr::Int(i) => Value::Int(i.get()),
            Expr::Float(f) => Value::Float(f.get()),
            Expr::Numeric(n) => {
                let (v, unit) = n.get();
                match unit {
                    ast::Unit::Pt => Value::Length(Length::pt(v)),
                    ast::Unit::Mm => Value::Length(Length::pt(v * MM)),
                    ast::Unit::Cm => Value::Length(Length::pt(v * 10.0 * MM)),
                    ast::Unit::In => Value::Length(Length::pt(v * 72.0)),
                    ast::Unit::Em => Value::Length(Length { pt: 0.0, em: v }),
                    ast::Unit::Percent => Value::Ratio(v / 100.0),
                    _ => Value::Thing,
                }
            }
            Expr::Str(s) => Value::Str(s.get().to_string()),
            Expr::Label(l) => Value::Label(l.get().to_owned()),
            Expr::ContentBlock(b) => {
                let exprs: Vec<Expr> = b.body().exprs().collect();
                let written = self.written.take();
                let value = self.seq(&exprs, &mut env.clone(), true);
                self.written = written;
                match written {
                    Some((file, depth)) if file == env.file && depth == self.depth => styled(
                        vec![Effect::Written(Rc::new(Written {
                            path: self.files[file].path.clone(),
                            node: expr.to_untyped().clone(),
                        }))],
                        value,
                    ),
                    _ => value,
                }
            }
            Expr::CodeBlock(b) => {
                let exprs: Vec<Expr> = b.body().exprs().collect();
                self.seq(&exprs, &mut env.clone(), false)
            }
            Expr::Ident(id) => self.lookup(env, id.as_str()),
            Expr::Parenthesized(p) => self.eval(p.expr(), env),
            Expr::Array(a) => {
                let mut out = Vec::new();
                for item in a.items() {
                    match item {
                        ast::ArrayItem::Pos(e) => out.push(self.eval(e, env)),
                        ast::ArrayItem::Spread(s) => match self.eval(s.expr(), env) {
                            Value::Array(more) => out.extend(more.iter().cloned()),
                            Value::None => {}
                            _ => return Value::Unknown,
                        },
                    }
                }
                Value::Array(Rc::new(out))
            }
            Expr::Dict(d) => {
                let mut out = Vec::new();
                for item in d.items() {
                    match item {
                        ast::DictItem::Named(n) => {
                            let v = self.eval(n.expr(), env);
                            out.push((n.name().as_str().to_owned(), v));
                        }
                        ast::DictItem::Keyed(k) => {
                            let Value::Str(key) = self.eval(k.key(), env) else {
                                return Value::Unknown;
                            };
                            let v = self.eval(k.expr(), env);
                            out.push((key, v));
                        }
                        ast::DictItem::Spread(s) => match self.eval(s.expr(), env) {
                            Value::Dict(more) => out.extend(more.iter().cloned()),
                            Value::None => {}
                            _ => return Value::Unknown,
                        },
                    }
                }
                Value::Dict(Rc::new(out))
            }
            Expr::FieldAccess(f) => {
                let target = self.eval(f.target(), env);
                self.field(target, f.field().as_str())
            }
            Expr::FuncCall(call) => self.call(call, env, false),
            Expr::Closure(c) => Value::Func(Rc::new(Func::Closure {
                file: env.file,
                node: c.to_untyped().clone(),
                captured: Rc::new(env.locals.clone()),
            })),
            Expr::Unary(u) => {
                let v = self.eval(u.expr(), env);
                unary(u.op(), v)
            }
            Expr::Binary(b) => {
                let op = b.op();
                if matches!(op, ast::BinOp::And | ast::BinOp::Or) {
                    let lhs = self.eval(b.lhs(), env);
                    return match (op, lhs) {
                        (ast::BinOp::And, Value::Bool(false)) => Value::Bool(false),
                        (ast::BinOp::Or, Value::Bool(true)) => Value::Bool(true),
                        (_, Value::Bool(_)) => match self.eval(b.rhs(), env) {
                            Value::Bool(r) => Value::Bool(r),
                            _ => Value::Unknown,
                        },
                        _ => Value::Unknown,
                    };
                }
                let lhs = self.eval(b.lhs(), env);
                let rhs = self.eval(b.rhs(), env);
                binary(op, lhs, rhs)
            }
            Expr::Conditional(c) => match self.eval(c.condition(), env) {
                Value::Bool(true) => self.eval(c.if_body(), env),
                Value::Bool(false) => match c.else_body() {
                    Some(e) => self.eval(e, env),
                    None => Value::None,
                },
                _ => {
                    let a = self.eval(c.if_body(), env);
                    let ra = self.ret.take();
                    let b = match c.else_body() {
                        Some(e) => self.eval(e, env),
                        None => Value::None,
                    };
                    let rb = self.ret.take();
                    // `return` в гілці, якої не видно: результат функції невідомий
                    if ra.is_some() || rb.is_some() {
                        self.ret = Some(Value::Unknown);
                        return Value::Never;
                    }
                    uncertain(a, b)
                }
            },
            Expr::FuncReturn(r) => {
                let v = match r.body() {
                    Some(e) => self.eval(e, env),
                    None => Value::None,
                };
                self.ret = Some(v);
                Value::Never
            }
            Expr::ForLoop(f) => {
                let iterable = self.eval(f.iterable(), env);
                let names: Vec<String> = f
                    .pattern()
                    .bindings()
                    .iter()
                    .map(|n| n.as_str().to_owned())
                    .collect();
                let Some(values) = iterate(&iterable) else {
                    // Скільки разів і з чим, не видно: вміст тіла лише можливий
                    let mut inner = env.clone();
                    for n in &names {
                        inner.locals.push((n.clone(), Value::Unknown));
                    }
                    let once = self.eval(f.body(), &mut inner);
                    return uncertain(once, Value::None);
                };
                let mut acc = Value::None;
                for v in values.iter().take(MAX_LOOP) {
                    let mut inner = env.clone();
                    inner.locals.extend(destructure(f.pattern(), v));
                    let body = self.eval(f.body(), &mut inner);
                    if self.ret.is_some() {
                        return Value::Never;
                    }
                    acc = join(acc, body);
                }
                acc
            }
            Expr::WhileLoop(_) => Value::Unknown,
            Expr::Contextual(c) => content(vec![Item::Ctx(Rc::new(Deferred {
                file: env.file,
                node: c.body().to_untyped().clone(),
                captured: Rc::new(env.locals.clone()),
            }))]),
            Expr::SetRule(_) | Expr::ShowRule(_) | Expr::LetBinding(_) | Expr::ModuleImport(_) => {
                let exprs = [expr];
                self.seq(&exprs, env, false)
            }
            Expr::Text(_)
            | Expr::Space(_)
            | Expr::Linebreak(_)
            | Expr::Parbreak(_)
            | Expr::Escape(_)
            | Expr::Shorthand(_)
            | Expr::SmartQuote(_)
            | Expr::Strong(_)
            | Expr::Emph(_)
            | Expr::Raw(_)
            | Expr::Heading(_)
            | Expr::Equation(_)
            | Expr::Link(_)
            | Expr::Ref(_)
            | Expr::ListItem(_)
            | Expr::EnumItem(_)
            | Expr::TermItem(_) => self.markup_value(expr, env),
            _ => Value::Unknown,
        }
    }

    /// Вираз документа: вміст лишається посиланням на свій код, виклики
    /// обчислюються з такими аргументами.
    fn eval_doc(&mut self, expr: Expr, env: &mut Env) -> Value {
        if !self.tick() {
            return Value::Unknown;
        }
        match expr {
            Expr::ContentBlock(_)
            | Expr::Strong(_)
            | Expr::Emph(_)
            | Expr::Raw(_)
            | Expr::Equation(_)
            | Expr::Ref(_)
            | Expr::Link(_)
            | Expr::ModuleInclude(_) => content(vec![Item::Source(expr.to_untyped().clone())]),
            Expr::Parenthesized(p) => self.eval_doc(p.expr(), env),
            Expr::Array(a) => {
                let mut out = Vec::new();
                for item in a.items() {
                    match item {
                        ast::ArrayItem::Pos(e) => out.push(self.eval_doc(e, env)),
                        ast::ArrayItem::Spread(s) => match self.eval(s.expr(), env) {
                            Value::Array(more) => out.extend(more.iter().cloned()),
                            _ => return Value::Unknown,
                        },
                    }
                }
                Value::Array(Rc::new(out))
            }
            Expr::Dict(d) => {
                let mut out = Vec::new();
                for item in d.items() {
                    match item {
                        ast::DictItem::Named(n) => {
                            let v = self.eval_doc(n.expr(), env);
                            out.push((n.name().as_str().to_owned(), v));
                        }
                        ast::DictItem::Keyed(k) => {
                            let Value::Str(key) = self.eval(k.key(), env) else {
                                return Value::Unknown;
                            };
                            let v = self.eval_doc(k.expr(), env);
                            out.push((key, v));
                        }
                        ast::DictItem::Spread(s) => match self.eval(s.expr(), env) {
                            Value::Dict(more) => out.extend(more.iter().cloned()),
                            _ => return Value::Unknown,
                        },
                    }
                }
                Value::Dict(Rc::new(out))
            }
            Expr::FuncCall(call) => self.call(call, env, true),
            _ => self.eval(expr, env),
        }
    }

    /// Послідовність виразів блоку коду чи розмітки: `set` і `show` діють на
    /// все після себе.
    fn seq(&mut self, exprs: &[Expr], env: &mut Env, markup: bool) -> Value {
        let mut acc = if markup {
            content(Vec::new())
        } else {
            Value::None
        };
        let mut i = 0;
        while i < exprs.len() {
            let e = exprs[i];
            i += 1;
            match e {
                Expr::LetBinding(l) => {
                    self.let_binding(l, env);
                }
                Expr::ModuleImport(imp) => {
                    let bound = self.import(imp, env);
                    env.locals.extend(bound);
                }
                Expr::SetRule(set) => {
                    let Some(effect) = self.set_effect(set, env) else {
                        continue;
                    };
                    let rest = self.seq(&exprs[i..], env, markup);
                    return join(acc, styled(vec![effect], rest));
                }
                Expr::ShowRule(show) => match show.selector() {
                    None => {
                        let rest = self.seq(&exprs[i..], env, markup);
                        let shown = match show.transform() {
                            Expr::SetRule(set) => match self.set_effect(set, env) {
                                Some(effect) => styled(vec![effect], rest),
                                None => rest,
                            },
                            t => {
                                let func = self.eval(t, env);
                                self.show_with(&func, rest, env.file)
                            }
                        };
                        return join(acc, shown);
                    }
                    // `show par: set text(…)` діє на абзаци основного тексту, а
                    // `show par: it => …` змінює їх невідомо як
                    Some(sel) if is_body_selector(sel) => {
                        let effect = match show.transform() {
                            Expr::SetRule(set) => self.set_effect(set, env),
                            _ => Some(Effect::Opaque),
                        };
                        if let Some(effect) = effect {
                            let rest = self.seq(&exprs[i..], env, markup);
                            return join(acc, styled(vec![effect], rest));
                        }
                    }
                    Some(_) => {
                        let effects: Vec<Effect> = self
                            .heading_show(show, env)
                            .into_iter()
                            .chain(self.show_effect(show, env))
                            .collect();
                        if !effects.is_empty() {
                            let rest = self.seq(&exprs[i..], env, markup);
                            return join(acc, styled(effects, rest));
                        }
                    }
                },
                e if markup => {
                    let v = self.markup_value(e, env);
                    acc = join(acc, content(v.items()));
                    if matches!(v, Value::Never) {
                        return Value::Never;
                    }
                }
                e => {
                    let v = self.eval(e, env);
                    if self.ret.is_some() {
                        return Value::Never;
                    }
                    acc = join(acc, v);
                }
            }
            if self.ret.is_some() {
                return Value::Never;
            }
        }
        acc
    }

    /// `show: func` над вмістом `rest`.
    fn show_with(&mut self, func: &Value, rest: Value, file: usize) -> Value {
        match func {
            Value::Func(_) => {
                let args = Args {
                    pos: vec![rest],
                    ..Default::default()
                };
                self.call_value(func, args, file, Span::detached())
            }
            // Невідома функція: вміст лишається, а що вона з ним робить, не видно
            _ => styled(vec![Effect::Opaque], rest),
        }
    }

    /// Вираз у розмітці шаблону.
    fn markup_value(&mut self, expr: Expr, env: &mut Env) -> Value {
        let text = |s: String| content(vec![Item::Text(s)]);
        match expr {
            Expr::Text(t) => text(t.get().to_string()),
            Expr::Space(_) | Expr::Linebreak(_) => content(vec![Item::Space]),
            Expr::Parbreak(_) => content(vec![Item::Parbreak]),
            Expr::Escape(e) => text(e.get().to_string()),
            Expr::Shorthand(s) => text(s.get().to_string()),
            Expr::SmartQuote(q) => text(if q.double() { "\"" } else { "'" }.into()),
            Expr::Strong(s) => {
                let exprs: Vec<Expr> = s.body().exprs().collect();
                self.seq(&exprs, &mut env.clone(), true)
            }
            Expr::Emph(s) => {
                let exprs: Vec<Expr> = s.body().exprs().collect();
                self.seq(&exprs, &mut env.clone(), true)
            }
            Expr::Heading(h) => {
                let exprs: Vec<Expr> = h.body().exprs().collect();
                let body = self.seq(&exprs, &mut env.clone(), true);
                elem(
                    "heading",
                    Args {
                        named: vec![
                            ("level".into(), Value::Int(h.depth().get() as i64)),
                            ("body".into(), body),
                        ],
                        ..Default::default()
                    },
                )
            }
            Expr::Label(l) => content(vec![Item::Label(l.get().to_owned())]),
            Expr::Raw(r) if !r.block() => {
                let lines: Vec<&str> = r.lines().map(|l| l.get().as_str()).collect();
                text(lines.join(" "))
            }
            Expr::Raw(_) => elem("raw", Args::default()),
            Expr::Equation(_)
            | Expr::Ref(_)
            | Expr::Link(_)
            | Expr::ListItem(_)
            | Expr::EnumItem(_)
            | Expr::TermItem(_) => content(vec![Item::Unknown]),
            e => self.eval(e, env),
        }
    }

    fn field(&mut self, target: Value, name: &str) -> Value {
        match target {
            Value::Module(m) => {
                self.ensure_module(m);
                self.files[m]
                    .bindings
                    .get(name)
                    .cloned()
                    .unwrap_or(Value::Unknown)
            }
            Value::Func(f) => match &*f {
                Func::Builtin(b) if b == "std" => {
                    Value::Func(Rc::new(Func::Builtin(name.to_owned())))
                }
                Func::Builtin(b) => Value::Func(Rc::new(Func::Builtin(format!("{b}.{name}")))),
                _ => Value::Unknown,
            },
            Value::Dict(_) => target.get(name).cloned().unwrap_or(Value::Unknown),
            // Поле елемента в show-функції (`it.body`, `it.level`)
            Value::Content(items) => match items.as_slice() {
                [Item::Elem(e)] => e.named(name).cloned().unwrap_or(Value::Unknown),
                _ => Value::Unknown,
            },
            _ => Value::Unknown,
        }
    }

    fn args(&mut self, args: ast::Args, env: &mut Env, doc: bool) -> Args {
        let mut out = Args::default();
        for arg in args.items() {
            match arg {
                ast::Arg::Pos(e) => {
                    let v = if doc {
                        self.eval_doc(e, env)
                    } else {
                        self.eval(e, env)
                    };
                    out.pos.push(v);
                }
                ast::Arg::Named(n) => {
                    let v = if doc {
                        self.eval_doc(n.expr(), env)
                    } else {
                        self.eval(n.expr(), env)
                    };
                    out.named.push((n.name().as_str().to_owned(), v));
                }
                ast::Arg::Spread(s) => {
                    let v = self.eval(s.expr(), env);
                    spread(&mut out, v);
                }
            }
        }
        out
    }

    /// Функція, яку викликає `callee`; методи значень (`d.at(…)`) обчислюються
    /// тут же, тож `Err`: уже готове значення.
    fn callee(
        &mut self,
        callee: Expr,
        args: ast::Args,
        env: &mut Env,
        doc: bool,
    ) -> Result<Value, Value> {
        let Expr::FieldAccess(f) = callee else {
            return Ok(self.eval(callee, env));
        };
        let target = self.eval(f.target(), env);
        let name = f.field().as_str();
        match (&target, name) {
            (Value::Counter(c), "display") if c != "state" => Err(content(vec![Item::Number])),
            // Зміна лічильника нічого не друкує, але номери далі інші
            (Value::Counter(c), "update" | "step") if c != "state" => {
                let a = self.args(args, env, false);
                let value = match name {
                    "update" => a.pos.first().cloned().unwrap_or(Value::Unknown),
                    _ => Value::Unknown,
                };
                Err(content(vec![Item::Count(c.clone(), value)]))
            }
            (Value::Counter(_), "update") => Err(content(Vec::new())),
            (Value::Func(func), "with") => {
                let bound = self.args(args, env, doc);
                Err(Value::Func(Rc::new(Func::With(func.clone(), bound))))
            }
            (Value::Func(func), "where") => {
                let Func::Builtin(name) = &**func else {
                    return Err(Value::Unknown);
                };
                let a = self.args(args, env, false);
                Err(Value::Selector(Rc::new((
                    name.trim_start_matches("std.").to_owned(),
                    a.named,
                ))))
            }
            (Value::Func(_), "before" | "after" | "or" | "and") => Err(Value::Unknown),
            // Лічильник заголовків там, де стоїть `context`
            (Value::Counter(c), "get" | "at") if c == "heading" => Err(match &self.heading {
                Some(h) => Value::Array(Rc::new(h.iter().map(|&n| Value::Int(n)).collect())),
                None => Value::Unknown,
            }),
            // Поля й методи елемента в show-функції
            (Value::Content(items), "has" | "func")
                if matches!(items.as_slice(), [Item::Elem(_)]) =>
            {
                let [Item::Elem(e)] = items.as_slice() else {
                    unreachable!()
                };
                if name == "func" {
                    return Err(Value::Func(Rc::new(Func::Builtin(e.func.clone()))));
                }
                let a = self.args(args, env, false);
                Err(match a.pos.first() {
                    Some(Value::Str(k)) => {
                        Value::Bool(e.named(k).is_some_and(|v| !matches!(v, Value::None)))
                    }
                    _ => Value::Unknown,
                })
            }
            (Value::Module(_), _) | (Value::Func(_), _) => Ok(self.field(target, name)),
            (Value::Array(items), "map") => {
                let a = self.args(args, env, false);
                let Some(func) = a.pos.first() else {
                    return Err(Value::Unknown);
                };
                let mut out = Vec::new();
                for item in items.iter().take(MAX_LOOP) {
                    let one = Args {
                        pos: vec![item.clone()],
                        ..Default::default()
                    };
                    out.push(self.call_value(func, one, env.file, Span::detached()));
                }
                Err(Value::Array(Rc::new(out)))
            }
            (Value::Dict(_) | Value::Array(_), "remove" | "insert" | "push" | "pop") => {
                let a = self.args(args, env, false);
                let (result, changed) = mutate(&target, name, &a);
                // Метод змінює змінну, з якої його викликано
                if let Expr::Ident(id) = f.target()
                    && let Some(slot) = env.locals.iter_mut().rev().find(|(n, _)| n == id.as_str())
                {
                    slot.1 = changed;
                }
                Err(result)
            }
            (Value::Dict(_) | Value::Array(_) | Value::Str(_) | Value::Args(_), _) => {
                let a = self.args(args, env, false);
                Err(method(&target, name, &a))
            }
            _ => Err(Value::Unknown),
        }
    }

    /// Виклик; `doc`: виклик із коду документа, його вміст лишається посиланням.
    fn call(&mut self, call: ast::FuncCall, env: &mut Env, doc: bool) -> Value {
        let func = match self.callee(call.callee(), call.args(), env, doc) {
            Ok(func) => func,
            Err(value) => return value,
        };
        if doc && !matches!(func, Value::Func(_)) {
            return content(vec![Item::Source(call.to_untyped().clone())]);
        }
        let args = self.args(call.args(), env, doc);
        self.call_value(&func, args, env.file, call.span())
    }

    fn call_value(&mut self, func: &Value, args: Args, file: usize, span: Span) -> Value {
        let Value::Func(f) = func else {
            return Value::Unknown;
        };
        match &**f {
            Func::With(inner, bound) => {
                let inner = Value::Func(inner.clone());
                self.call_value(&inner, bound.then(&args), file, span)
            }
            Func::Builtin(name) => self.builtin(name, args, file, span),
            Func::Closure { .. } => {
                if self.depth >= MAX_DEPTH {
                    return Value::Unknown;
                }
                let f = f.clone();
                let Func::Closure {
                    file,
                    node,
                    captured,
                } = &*f
                else {
                    unreachable!()
                };
                let Some(closure) = node.cast::<ast::Closure>() else {
                    return Value::Unknown;
                };
                let mut env = self.bind(*file, closure, captured, args);
                self.depth += 1;
                let saved = self.ret.take();
                let v = self.eval(closure.body(), &mut env);
                let v = self.ret.take().unwrap_or(v);
                self.ret = saved;
                self.depth -= 1;
                v
            }
        }
    }

    fn bind(
        &mut self,
        file: usize,
        closure: ast::Closure,
        captured: &[(String, Value)],
        args: Args,
    ) -> Env {
        let mut env = Env {
            file,
            locals: captured.to_vec(),
        };
        self.ensure_module(file);
        let mut pos = args.pos.into_iter();
        let mut named = args.named;
        let mut sink = None;
        for param in closure.params().children() {
            match param {
                ast::Param::Pos(ast::Pattern::Normal(Expr::Ident(id))) => {
                    let v = pos.next().unwrap_or(Value::Unknown);
                    env.locals.push((id.as_str().to_owned(), v));
                }
                ast::Param::Pos(pattern) => {
                    pos.next();
                    for id in pattern.bindings() {
                        env.locals.push((id.as_str().to_owned(), Value::Unknown));
                    }
                }
                ast::Param::Named(n) => {
                    let name = n.name().as_str();
                    let given = named.iter().rposition(|(k, _)| k == name);
                    let v = match given {
                        Some(i) => {
                            let v = named[i].1.clone();
                            named.retain(|(k, _)| k != name);
                            v
                        }
                        None if args.open => Value::Unknown,
                        None => self.eval(n.expr(), &mut env),
                    };
                    env.locals.push((name.to_owned(), v));
                }
                ast::Param::Spread(s) => sink = s.sink_ident().map(|id| id.as_str().to_owned()),
            }
        }
        if let Some(name) = sink {
            let rest = Args {
                pos: pos.collect(),
                named,
                open: args.open,
            };
            env.locals.push((name, Value::Args(Rc::new(rest))));
        }
        env
    }

    fn builtin(&mut self, name: &str, args: Args, file: usize, span: Span) -> Value {
        let name = name.trim_start_matches("std.");
        let first = args.pos.first();
        // Позиційні аргументи-вміст (`align(center, [..])` -> `[..]`)
        let body = |args: &Args| -> Vec<Item> {
            args.pos
                .iter()
                .filter(|v| matches!(v, Value::Content(_) | Value::Str(_)))
                .flat_map(|v| v.items())
                .collect()
        };
        match name {
            "heading" | "figure" | "figure.caption" | "table" | "table.header" | "table.footer"
            | "table.cell" | "grid" | "grid.cell" | "grid.header" | "bibliography" | "outline"
            | "image" | "raw" | "list" | "enum" | "terms" | "footnote" | "math.equation"
            | "cite" | "ref" => elem(name, args),
            "table.hline" | "table.vline" | "grid.hline" | "grid.vline" | "metadata" => {
                content(Vec::new())
            }
            "align" => {
                let block = content(vec![Item::Block(Rc::new(body(&args)))]);
                match args.pos.iter().find(|v| matches!(v, Value::Align(_))) {
                    Some(a) => {
                        let set = self.builtin_set("align", "alignment", a.clone(), file, span);
                        styled(vec![set], block)
                    }
                    None => block,
                }
            }
            // Лівий відступ `pad` стоїть на початку блоку
            "pad" => {
                let named = |k: &str| args.named.iter().rev().find(|(n, _)| n == k);
                let left = named("left")
                    .or_else(|| named("x"))
                    .or_else(|| named("rest"))
                    .map(|(_, v)| v.clone())
                    .or_else(|| {
                        args.pos
                            .iter()
                            .find(|v| matches!(v, Value::Length(_)))
                            .cloned()
                    });
                let mut items = Vec::new();
                if let Some(left) = left {
                    items.push(Item::H(left));
                }
                items.extend(body(&args));
                content(vec![Item::Block(Rc::new(items))])
            }
            "block" | "place" | "stack" | "columns" | "rect" | "square" | "circle" | "ellipse"
            | "scale" | "move" | "rotate" | "quote" => {
                content(vec![Item::Block(Rc::new(body(&args)))])
            }
            "strong" => {
                let set = self.builtin_set("text", "weight", Value::Str("bold".into()), file, span);
                styled(vec![set], content(body(&args)))
            }
            "emph" => {
                let set =
                    self.builtin_set("text", "style", Value::Str("italic".into()), file, span);
                styled(vec![set], content(body(&args)))
            }
            "box" | "underline" | "overline" | "strike" | "highlight" | "smallcaps" | "sub"
            | "super" | "hide" | "link" | "repeat" => content(body(&args)),
            "upper" | "lower" => {
                let up = name == "upper";
                let case = |s: &str| {
                    if up {
                        s.to_uppercase()
                    } else {
                        s.to_lowercase()
                    }
                };
                match first {
                    Some(Value::Str(s)) => Value::Str(case(s)),
                    Some(Value::Content(items)) => {
                        let mapped = items
                            .iter()
                            .map(|i| match i {
                                Item::Text(t) => Item::Text(case(t)),
                                other => other.clone(),
                            })
                            .collect();
                        content(vec![Item::Styled(
                            Rc::new(vec![Effect::Case(up)]),
                            Rc::new(mapped),
                        )])
                    }
                    _ => Value::Unknown,
                }
            }
            "text" | "par" | "page" => {
                let set = SetEffect {
                    target: name.to_owned(),
                    args: args
                        .named
                        .iter()
                        .map(|(n, v)| SetArg {
                            name: n.clone(),
                            value: v.clone(),
                            span: Span::detached(),
                        })
                        .collect(),
                    open: args.open,
                    path: self.files[file].path.clone(),
                    span,
                };
                styled(vec![Effect::Set(Rc::new(set))], content(body(&args)))
            }
            "pagebreak" => content(vec![Item::Pagebreak]),
            "colbreak" | "v" | "parbreak" => content(vec![Item::Parbreak]),
            "h" => content(vec![Item::H(first.cloned().unwrap_or(Value::Unknown))]),
            "linebreak" => content(vec![Item::Space]),
            "line" => elem(name, args),
            "state" => Value::Counter("state".into()),
            "counter" => match first {
                Some(Value::Func(f)) => match &**f {
                    Func::Builtin(b) => Value::Counter(b.trim_start_matches("std.").to_owned()),
                    _ => Value::Unknown,
                },
                Some(Value::Str(s)) => Value::Counter(s.clone()),
                // `counter(figure.where(kind: image))`
                Some(Value::Selector(sel)) => {
                    let kind = sel.1.iter().find(|(k, _)| k == "kind").map(|(_, v)| v);
                    match kind {
                        Some(Value::Func(f)) => match &**f {
                            Func::Builtin(k) => Value::Counter(format!("{}:{k}", sel.0)),
                            _ => Value::Unknown,
                        },
                        None => Value::Counter(sel.0.clone()),
                        _ => Value::Unknown,
                    }
                }
                _ => Value::Unknown,
            },
            // Посилання на мітку: скільки їх у документі
            "query" => match (first, &self.refs) {
                (Some(Value::Selector(sel)), Some(refs)) if sel.0 == "ref" => {
                    match sel.1.iter().find(|(k, _)| k == "target").map(|(_, v)| v) {
                        Some(Value::Label(l)) => {
                            Value::Array(Rc::new(vec![
                                Value::Thing;
                                refs.get(l).copied().unwrap_or(0)
                            ]))
                        }
                        _ => Value::Unknown,
                    }
                }
                _ => Value::Unknown,
            },
            "here" | "locate" => Value::Thing,
            "numbering" => {
                let nums: Option<Vec<i64>> = args
                    .pos
                    .iter()
                    .skip(1)
                    .map(|v| match v {
                        Value::Int(i) => Some(*i),
                        _ => None,
                    })
                    .collect();
                match (first, nums) {
                    (_, None) => Value::Unknown,
                    (Some(Value::Str(p)), Some(nums)) => {
                        apply_numbering(p, &nums).map_or(Value::Unknown, Value::Str)
                    }
                    (Some(f @ Value::Func(_)), Some(nums)) => {
                        let f = f.clone();
                        let call = Args {
                            pos: nums.into_iter().map(Value::Int).collect(),
                            ..Default::default()
                        };
                        self.call_value(&f, call, file, span)
                    }
                    _ => Value::Unknown,
                }
            }
            // Мітка з виразу, якого не обчислити: є, але яка, не видно
            "label" => match first {
                Some(Value::Str(s)) => Value::Label(s.clone()),
                _ => Value::Label(String::new()),
            },
            "panic" => Value::Never,
            "assert" => Value::None,
            "type" => match first.and_then(type_of) {
                Some(t) => Value::Type(t),
                None => Value::Unknown,
            },
            "rgb" | "luma" => match black(name, &args) {
                Some(b) => Value::Color(b),
                None => Value::Thing,
            },
            "path" | "read" | "bytes" | "datetime" | "regex" | "cmyk" | "color" => Value::Thing,
            "toml" => match first {
                Some(Value::Str(path)) => {
                    let base = &self.files[file];
                    let path = match path.strip_prefix('/') {
                        Some(rel) => base.root.join(rel),
                        None => base.path.parent().unwrap_or(&base.root).join(path),
                    };
                    (self.load)(&normalize(&path))
                        .and_then(|text| toml::from_str::<toml::Table>(&text).ok())
                        .map_or(Value::Unknown, |t| from_toml(toml::Value::Table(t)))
                }
                _ => Value::Unknown,
            },
            "str" => match first {
                Some(Value::Str(s)) => Value::Str(s.clone()),
                Some(Value::Int(i)) => Value::Str(i.to_string()),
                _ => Value::Unknown,
            },
            "int" => match first {
                Some(Value::Int(i)) => Value::Int(*i),
                _ => Value::Unknown,
            },
            "float" => match first.and_then(number) {
                Some(f) => Value::Float(f),
                None => Value::Unknown,
            },
            "lorem" => content(vec![Item::Unknown]),
            "range" => range(&args),
            _ if name.starts_with("calc.") => calc(&name[5..], &args),
            _ => Value::Unknown,
        }
    }

    fn set_effect(&mut self, set: ast::SetRule, env: &mut Env) -> Option<Effect> {
        let target = path_of(set.target())?;
        let cond = set.condition().map(|c| self.eval(c, env));
        if matches!(cond, Some(Value::Bool(false))) {
            return None;
        }
        // `set … if умова` з невідомою умовою: могло й не застосуватися
        let uncertain = cond.is_some() && !matches!(cond, Some(Value::Bool(true)));
        let mut args = Vec::new();
        let mut open = false;
        for arg in set.args().items() {
            match arg {
                ast::Arg::Named(n) => {
                    let value = if uncertain {
                        Value::Unknown
                    } else {
                        self.eval(n.expr(), env)
                    };
                    args.push(SetArg {
                        name: n.name().as_str().to_owned(),
                        value,
                        span: n.expr().span(),
                    });
                }
                // `set align(center)`: вирівнювання — позиційний аргумент
                ast::Arg::Pos(e) if path_of(set.target()).as_deref() == Some("align") => {
                    let value = if uncertain {
                        Value::Unknown
                    } else {
                        self.eval(e, env)
                    };
                    args.push(SetArg {
                        name: "alignment".into(),
                        value,
                        span: e.span(),
                    });
                }
                ast::Arg::Pos(_) | ast::Arg::Spread(_) => open = true,
            }
        }
        // Нумерація заголовків функцією: який номер вона дає першому
        let target = target.trim_start_matches("std.").to_owned();
        if target == "heading"
            && let Some(f) = args
                .iter()
                .rev()
                .find(|a| a.name == "numbering")
                .map(|a| a.value.clone())
            && matches!(f, Value::Func(_))
        {
            let one = Args {
                pos: vec![Value::Int(1)],
                ..Default::default()
            };
            let first = self.call_value(&f, one, env.file, Span::detached());
            if let Some(text) = first.text() {
                args.push(SetArg {
                    name: "numbering-first".into(),
                    value: Value::Str(text),
                    span: Span::detached(),
                });
            }
        }
        Some(Effect::Set(Rc::new(SetEffect {
            target,
            args,
            open,
            path: self.files[env.file].path.clone(),
            span: set.span(),
        })))
    }

    /// `show елемент.where(…): …` як налаштування елемента; `None` для
    /// селекторів не з елементів (рядок, мітка, `selector(…)`).
    fn show_effect(&mut self, show: ast::ShowRule, env: &mut Env) -> Option<Effect> {
        let (elem, filter) = match show.selector()? {
            Expr::FuncCall(call) => {
                let Expr::FieldAccess(f) = call.callee() else {
                    return None;
                };
                if f.field().as_str() != "where" {
                    return None;
                }
                let elem = path_of(f.target())?;
                let mut filter = Vec::new();
                for arg in call.args().items() {
                    match arg {
                        ast::Arg::Named(n) => {
                            let v = self.eval(n.expr(), env);
                            filter.push((n.name().as_str().to_owned(), v));
                        }
                        _ => filter.push((String::new(), Value::Unknown)),
                    }
                }
                (elem, filter)
            }
            e => (path_of(e)?, Vec::new()),
        };
        let elem = elem.trim_start_matches("std.").to_owned();
        if !is_builtin(elem.split('.').next().unwrap_or("")) {
            return None;
        }
        let transform = match show.transform() {
            Expr::SetRule(set) => match self.set_effect(set, env)? {
                Effect::Set(s) => Transform::Set(s),
                _ => Transform::Unknown,
            },
            t => match self.eval(t, env) {
                v @ (Value::Func(_) | Value::Content(_) | Value::Str(_) | Value::None) => {
                    Transform::Func(v)
                }
                _ => Transform::Unknown,
            },
        };
        Some(Effect::Show(Rc::new(ShowEffect {
            elem,
            filter,
            transform,
            path: self.files[env.file].path.clone(),
            span: show.span(),
        })))
    }

    /// Налаштування, яке дає вбудована функція (`strong` -> `text(weight: "bold")`).
    fn builtin_set(
        &self,
        target: &str,
        arg: &str,
        value: Value,
        file: usize,
        span: Span,
    ) -> Effect {
        Effect::Set(Rc::new(SetEffect {
            target: target.into(),
            args: vec![SetArg {
                name: arg.into(),
                value,
                span: Span::detached(),
            }],
            open: false,
            path: self.files[file].path.clone(),
            span,
        }))
    }

    /// `show heading: it => …`, що друкує «Додаток» перед заголовком: такі
    /// заголовки є додатками, хоч `supplement` не задано.
    fn heading_show(&mut self, show: ast::ShowRule, env: &Env) -> Option<Effect> {
        let selector = path_of(match show.selector()? {
            Expr::FuncCall(call) => call.callee(),
            e => e,
        })?;
        let selector = selector.trim_end_matches(".where");
        if selector != "heading" || matches!(show.transform(), Expr::SetRule(_)) {
            return None;
        }
        let text = show.transform().to_untyped().full_text();
        if !text.to_lowercase().contains("додаток") {
            return None;
        }
        Some(Effect::Set(Rc::new(SetEffect {
            target: "heading".into(),
            args: vec![SetArg {
                name: "supplement".into(),
                value: Value::Str("Додаток".into()),
                span: Span::detached(),
            }],
            open: false,
            path: self.files[env.file].path.clone(),
            span: show.span(),
        })))
    }
}

/// Селектор, що охоплює абзаци основного тексту: `par`, `text`, `par.where(…)`.
pub fn is_body_selector(selector: Expr) -> bool {
    let target = match selector {
        Expr::FuncCall(call) => call.callee(),
        e => e,
    };
    path_of(target).is_some_and(|p| {
        matches!(
            p.trim_end_matches(".where").trim_start_matches("std."),
            "par" | "text"
        )
    })
}

fn elem(func: &str, args: Args) -> Value {
    content(vec![Item::Elem(Rc::new(Elem {
        func: func.to_owned(),
        args,
    }))])
}

/// Шлях елемента: `text`, `footnote.entry`, `math.equation`.
pub fn path_of(expr: Expr) -> Option<String> {
    match expr {
        Expr::Ident(i) => Some(i.as_str().to_owned()),
        Expr::FieldAccess(f) => Some(format!("{}.{}", path_of(f.target())?, f.field().as_str())),
        _ => None,
    }
}

fn spread(out: &mut Args, v: Value) {
    match v {
        Value::Array(a) => out.pos.extend(a.iter().cloned()),
        Value::Dict(d) => out.named.extend(d.iter().cloned()),
        Value::Args(a) => {
            out.pos.extend(a.pos.iter().cloned());
            out.named.extend(a.named.iter().cloned());
            out.open |= a.open;
        }
        Value::None => {}
        _ => out.open = true,
    }
}

/// Значення, що друкуються.
fn displayable(v: &Value) -> bool {
    matches!(
        v,
        Value::Content(_)
            | Value::Str(_)
            | Value::Label(_)
            | Value::Int(_)
            | Value::Float(_)
            | Value::Unknown
    )
}

/// З'єднання значень блоку (`{ a; b }`), як у typst.
fn join(a: Value, b: Value) -> Value {
    use Value as V;
    match (a, b) {
        (V::Never, _) | (_, V::Never) => V::Never,
        (V::None, b) => b,
        (a, V::None) => a,
        (V::Str(x), V::Str(y)) => V::Str(x + &y),
        (V::Array(x), V::Array(y)) => {
            V::Array(Rc::new(x.iter().chain(y.iter()).cloned().collect()))
        }
        (V::Dict(x), V::Dict(y)) => V::Dict(Rc::new(x.iter().chain(y.iter()).cloned().collect())),
        (a, b) if displayable(&a) && displayable(&b) => {
            let mut items = a.items();
            items.extend(b.items());
            content(items)
        }
        _ => V::Unknown,
    }
}

/// Значення `if` з умовою, якої не видно.
fn uncertain(a: Value, b: Value) -> Value {
    match (a, b) {
        (Value::Never, x) | (x, Value::Never) => x,
        (a, b) if matches!(a, Value::Content(_)) || matches!(b, Value::Content(_)) => {
            let (a, b) = (a.items(), b.items());
            // Вміст документа йде однією з гілок: обходимо її, але налаштування
            // там невідомі; друга гілка лише можлива
            let (doc, other) = match (has_source(&a), has_source(&b)) {
                (true, _) => (a, b),
                (false, true) => (b, a),
                (false, false) => {
                    let mut all = a;
                    all.extend(b);
                    return content(vec![Item::Maybe(Rc::new(all))]);
                }
            };
            content(vec![
                Item::Styled(Rc::new(vec![Effect::Opaque]), Rc::new(doc)),
                Item::Maybe(Rc::new(other)),
            ])
        }
        (a, b) => match same(&a, &b) {
            Some(true) => a,
            _ => Value::Unknown,
        },
    }
}

fn type_of(v: &Value) -> Option<&'static str> {
    Some(match v {
        Value::None => "none",
        Value::Auto => "auto",
        Value::Bool(_) => "bool",
        Value::Int(_) => "int",
        Value::Float(_) => "float",
        Value::Length(_) => "length",
        Value::Ratio(_) => "ratio",
        Value::Str(_) => "str",
        Value::Label(_) => "label",
        Value::Content(_) => "content",
        Value::Array(_) => "array",
        Value::Dict(_) => "dictionary",
        Value::Func(_) => "function",
        Value::Module(_) => "module",
        Value::Args(_) => "arguments",
        Value::Type(_) => "type",
        Value::Align(_) => "alignment",
        Value::Color(_) => "color",
        Value::Counter(_) => "counter",
        Value::Selector(_) => "selector",
        Value::Unknown | Value::Never | Value::Thing => return None,
    })
}

fn unary(op: ast::UnOp, v: Value) -> Value {
    match (op, v) {
        (ast::UnOp::Pos, v @ (Value::Int(_) | Value::Float(_) | Value::Length(_))) => v,
        (ast::UnOp::Neg, Value::Int(i)) => Value::Int(-i),
        (ast::UnOp::Neg, Value::Float(f)) => Value::Float(-f),
        (ast::UnOp::Neg, Value::Length(l)) => Value::Length(l.scale(-1.0)),
        (ast::UnOp::Not, Value::Bool(b)) => Value::Bool(!b),
        _ => Value::Unknown,
    }
}

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Int(i) => Some(*i as f64),
        Value::Float(f) => Some(*f),
        _ => None,
    }
}

fn binary(op: ast::BinOp, lhs: Value, rhs: Value) -> Value {
    use Value as V;
    use ast::BinOp::*;
    match (op, &lhs, &rhs) {
        (Add, V::Int(a), V::Int(b)) => V::Int(a + b),
        (Sub, V::Int(a), V::Int(b)) => V::Int(a - b),
        (Mul, V::Int(a), V::Int(b)) => V::Int(a * b),
        (Add, V::Length(a), V::Length(b)) => V::Length(Length {
            pt: a.pt + b.pt,
            em: a.em + b.em,
        }),
        (Sub, V::Length(a), V::Length(b)) => V::Length(Length {
            pt: a.pt - b.pt,
            em: a.em - b.em,
        }),
        (Mul, V::Length(l), n) | (Mul, n, V::Length(l)) if number(n).is_some() => {
            V::Length(l.scale(number(n).unwrap()))
        }
        (Div, V::Length(l), n) if number(n).is_some_and(|n| n != 0.0) => {
            V::Length(l.scale(1.0 / number(n).unwrap()))
        }
        (Add | Sub | Mul | Div, a, b) if number(a).is_some() && number(b).is_some() => {
            let (a, b) = (number(a).unwrap(), number(b).unwrap());
            match op {
                Add => V::Float(a + b),
                Sub => V::Float(a - b),
                Mul => V::Float(a * b),
                _ if b != 0.0 => V::Float(a / b),
                _ => V::Unknown,
            }
        }
        (Add, V::Str(a), V::Str(b)) => V::Str(format!("{a}{b}")),
        (Add, V::Align(a), V::Align(b)) => V::Align(format!("{a}+{b}")),
        (Add, V::Array(a), V::Array(b)) => {
            V::Array(Rc::new(a.iter().chain(b.iter()).cloned().collect()))
        }
        (Add, V::Dict(a), V::Dict(b)) => {
            V::Dict(Rc::new(a.iter().chain(b.iter()).cloned().collect()))
        }
        (Add, a, b)
            if matches!(a, V::Content(_) | V::Str(_)) && matches!(b, V::Content(_) | V::Str(_)) =>
        {
            let mut items = a.items();
            items.extend(b.items());
            content(items)
        }
        (Eq | Neq, a, b) => match same(a, b) {
            Some(eq) => V::Bool(eq == (op == Eq)),
            None => V::Unknown,
        },
        (Lt | Leq | Gt | Geq, a, b) if number(a).is_some() && number(b).is_some() => {
            let (a, b) = (number(a).unwrap(), number(b).unwrap());
            V::Bool(match op {
                Lt => a < b,
                Leq => a <= b,
                Gt => a > b,
                _ => a >= b,
            })
        }
        (In | NotIn, V::Str(a), V::Str(b)) => V::Bool(b.contains(a.as_str()) == (op == In)),
        (In | NotIn, V::Str(a), V::Dict(d)) => V::Bool(d.iter().any(|(k, _)| k == a) == (op == In)),
        (In | NotIn, a, V::Array(items)) => {
            let mut found = Some(false);
            for item in items.iter() {
                match same(a, item) {
                    Some(true) => {
                        found = Some(true);
                        break;
                    }
                    Some(false) => {}
                    None => found = None,
                }
            }
            match found {
                Some(f) => V::Bool(f == (op == In)),
                None => V::Unknown,
            }
        }
        _ => V::Unknown,
    }
}

/// Рівність значень; `None`, якщо її не видно.
fn same(a: &Value, b: &Value) -> Option<bool> {
    use Value as V;
    let known = |v: &Value| !matches!(v, V::Unknown | V::Never);
    Some(match (a, b) {
        (V::None, V::None) | (V::Auto, V::Auto) => true,
        (V::Bool(a), V::Bool(b)) => a == b,
        (V::Str(a), V::Str(b)) => a == b,
        (V::Label(a), V::Label(b)) => a == b,
        (V::Length(a), V::Length(b)) => a == b,
        (V::Type(a), V::Type(b)) => a == b,
        (V::Type(t), V::Func(f)) | (V::Func(f), V::Type(t)) => {
            matches!(&**f, Func::Builtin(b) if b == t)
        }
        (V::Func(a), V::Func(b)) => match (&**a, &**b) {
            (Func::Builtin(a), Func::Builtin(b)) => {
                a.trim_start_matches("std.") == b.trim_start_matches("std.")
            }
            _ => return None,
        },
        (V::Align(a), V::Align(b)) => {
            let parts = |s: &str| {
                let mut p: Vec<String> = s.split('+').map(str::to_owned).collect();
                p.sort_unstable();
                p
            };
            parts(a) == parts(b)
        }
        (V::Color(a), V::Color(b)) if *a && *b => true,
        (a, b) if number(a).is_some() && number(b).is_some() => number(a) == number(b),
        (V::Array(a), V::Array(b)) => {
            if a.len() != b.len() {
                return Some(false);
            }
            for (x, y) in a.iter().zip(b.iter()) {
                if !same(x, y)? {
                    return Some(false);
                }
            }
            true
        }
        // `none` і `auto` відрізняються від будь-якого відомого значення
        (V::None | V::Auto, v) | (v, V::None | V::Auto) if known(v) => false,
        (
            V::Bool(_) | V::Str(_) | V::Int(_) | V::Float(_) | V::Label(_),
            V::Bool(_) | V::Str(_) | V::Int(_) | V::Float(_) | V::Label(_),
        ) => false,
        _ => return None,
    })
}

fn method(target: &Value, name: &str, args: &Args) -> Value {
    let default = || {
        args.named
            .iter()
            .find(|(k, _)| k == "default")
            .map(|(_, v)| v.clone())
    };
    match (target, name, args.pos.first()) {
        (Value::Dict(_), "at", Some(Value::Str(key))) => target
            .get(key)
            .cloned()
            .or_else(default)
            .unwrap_or(Value::Unknown),
        (Value::Array(a), "at", Some(Value::Int(i))) => {
            let i = if *i < 0 { a.len() as i64 + i } else { *i };
            usize::try_from(i)
                .ok()
                .and_then(|i| a.get(i).cloned())
                .or_else(default)
                .unwrap_or(Value::Unknown)
        }
        (Value::Array(a), "enumerate", None) => {
            let start = match args.named.iter().find(|(k, _)| k == "start") {
                None => 0,
                Some((_, Value::Int(s))) => *s,
                Some(_) => return Value::Unknown,
            };
            Value::Array(Rc::new(
                a.iter()
                    .enumerate()
                    .map(|(i, v)| {
                        Value::Array(Rc::new(vec![Value::Int(start + i as i64), v.clone()]))
                    })
                    .collect(),
            ))
        }
        (Value::Array(a), "rev", None) => Value::Array(Rc::new(a.iter().rev().cloned().collect())),
        (Value::Dict(_), "pairs", None) => {
            iterate(target).map_or(Value::Unknown, |p| Value::Array(Rc::new(p)))
        }
        (Value::Str(s), "slice", Some(Value::Int(from))) => {
            let chars: Vec<char> = s.chars().collect();
            let at = |i: i64| -> Option<usize> {
                let i = if i < 0 { chars.len() as i64 + i } else { i };
                usize::try_from(i).ok().filter(|&i| i <= chars.len())
            };
            let end = match args.pos.get(1) {
                None => Some(chars.len()),
                Some(Value::Int(e)) => at(*e),
                Some(_) => None,
            };
            match (at(*from), end) {
                (Some(a), Some(b)) if a <= b => Value::Str(chars[a..b].iter().collect()),
                _ => Value::Unknown,
            }
        }
        (Value::Array(a), "first", None) => a.first().cloned().unwrap_or(Value::Unknown),
        (Value::Array(a), "last", None) => a.last().cloned().unwrap_or(Value::Unknown),
        (Value::Array(a), "len", None) => Value::Int(a.len() as i64),
        (Value::Array(a), "join", sep) => {
            let mut acc = Value::None;
            for (i, item) in a.iter().enumerate() {
                if i > 0
                    && let Some(sep) = sep
                {
                    acc = join(acc, sep.clone());
                }
                acc = join(acc, item.clone());
            }
            acc
        }
        (Value::Array(_), "contains", Some(v)) => binary(ast::BinOp::In, v.clone(), target.clone()),
        (Value::Dict(d), "len", None) => Value::Int(d.len() as i64),
        (Value::Dict(d), "keys", None) => Value::Array(Rc::new(
            d.iter().map(|(k, _)| Value::Str(k.clone())).collect(),
        )),
        (Value::Dict(d), "values", None) => {
            Value::Array(Rc::new(d.iter().map(|(_, v)| v.clone()).collect()))
        }
        (Value::Str(s), "len", None) => Value::Int(s.len() as i64),
        (Value::Str(s), "clusters" | "codepoints", None) => Value::Array(Rc::new(
            s.chars().map(|c| Value::Str(c.to_string())).collect(),
        )),
        (Value::Str(s), "trim", None) => Value::Str(s.trim().to_owned()),
        (Value::Str(s), "split", None) => Value::Array(Rc::new(
            s.split_whitespace()
                .map(|p| Value::Str(p.to_owned()))
                .collect(),
        )),
        (Value::Str(s), "split", Some(Value::Str(sep))) => Value::Array(Rc::new(
            s.split(sep.as_str())
                .map(|p| Value::Str(p.to_owned()))
                .collect(),
        )),
        (Value::Str(s), "replace", Some(Value::Str(from))) => match args.pos.get(1) {
            Some(Value::Str(to)) => Value::Str(s.replace(from.as_str(), to)),
            _ => Value::Unknown,
        },
        (Value::Str(s), "first", None) => s
            .chars()
            .next()
            .map_or(Value::Unknown, |c| Value::Str(c.to_string())),
        (Value::Str(s), "last", None) => s
            .chars()
            .last()
            .map_or(Value::Unknown, |c| Value::Str(c.to_string())),
        (Value::Str(s), "contains", Some(Value::Str(p))) => Value::Bool(s.contains(p.as_str())),
        (Value::Str(s), "starts-with", Some(Value::Str(p))) => {
            Value::Bool(s.starts_with(p.as_str()))
        }
        (Value::Str(s), "ends-with", Some(Value::Str(p))) => Value::Bool(s.ends_with(p.as_str())),
        (Value::Args(a), "pos", None) => Value::Array(Rc::new(a.pos.clone())),
        (Value::Args(a), "named", None) if !a.open => Value::Dict(Rc::new(a.named.clone())),
        (Value::Args(a), "at", Some(Value::Int(i))) => usize::try_from(*i)
            .ok()
            .and_then(|i| a.pos.get(i).cloned())
            .or_else(default)
            .unwrap_or(Value::Unknown),
        _ => Value::Unknown,
    }
}

/// Метод, що змінює словник чи масив: (результат, нове значення).
fn mutate(target: &Value, name: &str, args: &Args) -> (Value, Value) {
    let default = || {
        args.named
            .iter()
            .find(|(k, _)| k == "default")
            .map(|(_, v)| v.clone())
    };
    match (target, name, args.pos.first()) {
        (Value::Dict(d), "remove", Some(Value::Str(key))) => {
            let value = target.get(key).cloned().or_else(default);
            let rest: Vec<_> = d.iter().filter(|(k, _)| k != key).cloned().collect();
            (value.unwrap_or(Value::Unknown), Value::Dict(Rc::new(rest)))
        }
        (Value::Dict(d), "insert", Some(Value::Str(key))) => {
            let mut all: Vec<_> = d.iter().filter(|(k, _)| k != key).cloned().collect();
            all.push((
                key.clone(),
                args.pos.get(1).cloned().unwrap_or(Value::Unknown),
            ));
            (Value::None, Value::Dict(Rc::new(all)))
        }
        (Value::Array(a), "push", Some(v)) => {
            let mut all = a.to_vec();
            all.push(v.clone());
            (Value::None, Value::Array(Rc::new(all)))
        }
        (Value::Array(a), "pop", None) => {
            let mut all = a.to_vec();
            let last = all.pop().unwrap_or(Value::Unknown);
            (last, Value::Array(Rc::new(all)))
        }
        _ => (Value::Unknown, Value::Unknown),
    }
}

fn from_toml(v: toml::Value) -> Value {
    match v {
        toml::Value::String(s) => Value::Str(s),
        toml::Value::Integer(i) => Value::Int(i),
        toml::Value::Float(f) => Value::Float(f),
        toml::Value::Boolean(b) => Value::Bool(b),
        toml::Value::Array(a) => Value::Array(Rc::new(a.into_iter().map(from_toml).collect())),
        toml::Value::Table(t) => Value::Dict(Rc::new(
            t.into_iter().map(|(k, v)| (k, from_toml(v))).collect(),
        )),
        toml::Value::Datetime(_) => Value::Unknown,
    }
}

/// Теки пакунків typst у порядку пошуку: локальні (`@local`, власні простори
/// й переважно `@preview`), потім кеш завантажених `@preview`.
fn package_dirs(namespace: &str) -> Vec<PathBuf> {
    let env = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let home = env("HOME");
    let (data, cache) = if cfg!(target_os = "macos") {
        let lib = home.map(|h| h.join("Library"));
        (
            lib.as_ref().map(|l| l.join("Application Support")),
            lib.map(|l| l.join("Caches")),
        )
    } else if cfg!(windows) {
        (env("APPDATA"), env("LOCALAPPDATA"))
    } else {
        (
            env("XDG_DATA_HOME").or_else(|| home.as_ref().map(|h| h.join(".local/share"))),
            env("XDG_CACHE_HOME").or_else(|| home.map(|h| h.join(".cache"))),
        )
    };
    let mut out = Vec::new();
    if let Some(p) = env("TYPST_PACKAGE_PATH").or_else(|| data.map(|d| d.join("typst/packages"))) {
        out.push(p);
    }
    if namespace == "preview"
        && let Some(p) =
            env("TYPST_PACKAGE_CACHE_PATH").or_else(|| cache.map(|c| c.join("typst/packages")))
    {
        out.push(p);
    }
    out
}

/// Номер за шаблоном нумерації typst (`"1.1"`, `"а)"` не шаблон, `"A.1"`):
/// як `numbering(шаблон, ..nums)`. `None`: символ лічби, якого не
/// обчислюємо, чи шаблон без жодного.
pub fn apply_numbering(pattern: &str, nums: &[i64]) -> Option<String> {
    // (префікс, символ лічби) і суфікс, як у typst
    let mut pieces: Vec<(String, char)> = Vec::new();
    let mut prefix = String::new();
    for c in pattern.chars() {
        match c {
            '1' | 'a' | 'A' | 'i' | 'I' | '*' => pieces.push((std::mem::take(&mut prefix), c)),
            'α' | 'Α' | '一' | '壹' | 'あ' | 'い' | 'ア' | 'イ' | 'א' | '가' | 'ㄱ' | '①' | '⓵' =>
            {
                return None;
            }
            c => prefix.push(c),
        }
    }
    if pieces.is_empty() {
        return None;
    }
    let suffix = prefix;
    let mut out = String::new();
    let mut nums = nums.iter();
    for ((prefix, kind), &n) in pieces.iter().zip(&mut nums) {
        out.push_str(prefix);
        out.push_str(&count(*kind, n)?);
    }
    let (prefix, kind) = pieces.last()?;
    for &n in nums {
        out.push_str(if prefix.is_empty() { &suffix } else { prefix });
        out.push_str(&count(*kind, n)?);
    }
    out.push_str(&suffix);
    Some(out)
}

/// Число `n` символом лічби `kind`.
fn count(kind: char, n: i64) -> Option<String> {
    let letters = |base: u8| {
        let mut n = n;
        let mut out = Vec::new();
        while n > 0 {
            n -= 1;
            out.push((base + (n % 26) as u8) as char);
            n /= 26;
        }
        out.iter().rev().collect::<String>()
    };
    Some(match kind {
        '1' => n.to_string(),
        _ if n <= 0 => return None,
        'a' => letters(b'a'),
        'A' => letters(b'A'),
        'i' | 'I' => {
            let mut n = n;
            let mut out = String::new();
            for (v, s) in [
                (1000, "M"),
                (900, "CM"),
                (500, "D"),
                (400, "CD"),
                (100, "C"),
                (90, "XC"),
                (50, "L"),
                (40, "XL"),
                (10, "X"),
                (9, "IX"),
                (5, "V"),
                (4, "IV"),
                (1, "I"),
            ] {
                while n >= v {
                    out.push_str(s);
                    n -= v;
                }
            }
            if kind == 'i' { out.to_lowercase() } else { out }
        }
        _ => {
            let symbols = ['*', '†', '‡', '§', '¶', '‖'];
            let k = ((n - 1) / 6 + 1) as usize;
            symbols[((n - 1) % 6) as usize].to_string().repeat(k)
        }
    })
}

/// Чи чорний колір `rgb(…)` чи `luma(…)`; `None`, якщо з аргументів не видно.
fn black(name: &str, args: &Args) -> Option<bool> {
    let ints: Option<Vec<f64>> = args
        .pos
        .iter()
        .map(|v| match v {
            Value::Ratio(r) => Some(*r),
            v => number(v),
        })
        .collect();
    match (name, args.pos.first()) {
        ("rgb", Some(Value::Str(hex))) => {
            let hex = hex.trim_start_matches('#');
            let digits = if hex.len() == 4 || hex.len() == 8 {
                &hex[..hex.len() * 3 / 4]
            } else {
                hex
            };
            if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            Some(digits.chars().all(|c| c == '0'))
        }
        ("rgb", _) => {
            let v = ints?;
            (v.len() >= 3).then(|| v[..3].iter().all(|&c| c == 0.0))
        }
        _ => Some(ints?.first()? == &0.0),
    }
}

const ALIGNMENTS: &[&str] = &[
    "left", "right", "center", "top", "bottom", "horizon", "start", "end",
];

const COLORS: &[&str] = &[
    "gray", "silver", "white", "navy", "blue", "aqua", "teal", "eastern", "purple", "fuchsia",
    "maroon", "red", "orange", "yellow", "olive", "green", "lime",
];

/// Імена вбудованих функцій і типів: невідомий ідентифікатор інакше дає
/// [`Value::Unknown`].
fn is_builtin(name: &str) -> bool {
    BUILTINS.contains(&name)
}

#[rustfmt::skip]
const BUILTINS: &[&str] = &[
    "std", "text", "par", "page", "heading", "figure", "table", "grid", "block", "box",
    "align", "pad", "place", "stack", "columns", "colbreak", "pagebreak", "parbreak", "v",
    "h", "line", "rect", "square", "circle", "ellipse", "polygon", "image", "raw", "link",
    "strong", "emph", "underline", "overline", "strike", "highlight", "smallcaps", "sub",
    "super", "list", "enum", "terms", "footnote", "outline", "bibliography", "cite", "ref",
    "quote", "math", "lorem", "linebreak", "hide", "scale", "rotate", "move", "repeat",
    "metadata", "upper", "lower", "numbering", "counter", "state", "str", "int", "float",
    "bool", "array", "dictionary", "content", "function", "length", "arguments", "module",
    "type", "toml", "json", "yaml", "csv", "read", "path", "bytes", "datetime", "regex",
    "rgb", "luma", "cmyk", "color", "repr", "query", "here", "locate", "measure", "layout",
    "calc", "label", "selector", "document", "smartquote", "panic", "assert", "range",
];

/// Найбільше значень у `range(…)`: довший масив з коду не стежимо.
const MAX_RANGE: i64 = 10_000;

/// `range(кінець)`, `range(початок, кінець, step: крок)`.
fn range(args: &Args) -> Value {
    let int = |v: &Value| match v {
        Value::Int(i) => Some(*i),
        _ => None,
    };
    let step = match args.named.iter().find(|(k, _)| k == "step") {
        None => 1,
        Some((_, Value::Int(s))) if *s != 0 => *s,
        Some(_) => return Value::Unknown,
    };
    let bounds: Option<Vec<i64>> = args.pos.iter().map(int).collect();
    let (start, end) = match bounds.as_deref() {
        Some([end]) => (0, *end),
        Some([start, end]) => (*start, *end),
        _ => return Value::Unknown,
    };
    if (end - start) / step > MAX_RANGE {
        return Value::Unknown;
    }
    let mut out = Vec::new();
    let mut i = start;
    while (step > 0 && i < end) || (step < 0 && i > end) {
        out.push(Value::Int(i));
        i += step;
    }
    Value::Array(Rc::new(out))
}

/// Функції `calc`, які трапляються в нумерації й підписах.
fn calc(name: &str, args: &Args) -> Value {
    let pos = &args.pos;
    let ints: Option<Vec<i64>> = pos
        .iter()
        .map(|v| match v {
            Value::Int(i) => Some(*i),
            _ => None,
        })
        .collect();
    let floats: Option<Vec<f64>> = pos.iter().map(number).collect();
    match (name, ints.as_deref(), floats.as_deref()) {
        ("min", Some(i), _) if !i.is_empty() => Value::Int(*i.iter().min().unwrap()),
        ("max", Some(i), _) if !i.is_empty() => Value::Int(*i.iter().max().unwrap()),
        ("min", _, Some(f)) if !f.is_empty() => {
            Value::Float(f.iter().copied().fold(f64::INFINITY, f64::min))
        }
        ("max", _, Some(f)) if !f.is_empty() => {
            Value::Float(f.iter().copied().fold(f64::NEG_INFINITY, f64::max))
        }
        ("abs", Some([i]), _) => Value::Int(i.abs()),
        ("abs", _, Some([f])) => Value::Float(f.abs()),
        ("floor", _, Some([f])) => Value::Int(f.floor() as i64),
        ("ceil", _, Some([f])) => Value::Int(f.ceil() as i64),
        ("round", _, Some([f])) if args.named.is_empty() => Value::Int(f.round() as i64),
        ("rem", Some([a, b]), _) if *b != 0 => Value::Int(a % b),
        ("quo", Some([a, b]), _) if *b != 0 => Value::Int(a.div_euclid(*b)),
        ("pow", Some([a, b]), _) if (0..=62).contains(b) => {
            a.checked_pow(*b as u32).map_or(Value::Unknown, Value::Int)
        }
        ("even", Some([i]), _) => Value::Bool(i % 2 == 0),
        ("odd", Some([i]), _) => Value::Bool(i % 2 != 0),
        _ => Value::Unknown,
    }
}

/// Значення, за якими йде `for`: елементи масиву, пари `(ключ, значення)`
/// словника, символи рядка.
pub fn iterate(value: &Value) -> Option<Vec<Value>> {
    match value {
        Value::Array(a) => Some(a.to_vec()),
        Value::Dict(d) => Some(
            d.iter()
                .map(|(k, v)| Value::Array(Rc::new(vec![Value::Str(k.clone()), v.clone()])))
                .collect(),
        ),
        Value::Str(s) => Some(s.chars().map(|c| Value::Str(c.to_string())).collect()),
        _ => None,
    }
}

/// Прив'язки зразка `pattern` до `value` (`let (a, ..b) = …`, `for (k, v) in …`).
/// Чого не зіставити, те невідоме.
pub fn destructure(pattern: ast::Pattern, value: &Value) -> Vec<(String, Value)> {
    let unknown = |p: ast::Pattern| -> Vec<(String, Value)> {
        p.bindings()
            .into_iter()
            .map(|n| (n.as_str().to_owned(), Value::Unknown))
            .collect()
    };
    match pattern {
        ast::Pattern::Normal(Expr::Ident(id)) => vec![(id.as_str().to_owned(), value.clone())],
        ast::Pattern::Normal(_) | ast::Pattern::Placeholder(_) => Vec::new(),
        ast::Pattern::Parenthesized(p) => destructure(p.pattern(), value),
        ast::Pattern::Destructuring(d) => {
            let items: Vec<ast::DestructuringItem> = d.items().collect();
            match value {
                Value::Array(a) => {
                    let spread = items
                        .iter()
                        .position(|i| matches!(i, ast::DestructuringItem::Spread(_)));
                    let fixed = items.len() - usize::from(spread.is_some());
                    let fits = match spread {
                        Some(_) => a.len() >= fixed,
                        None => a.len() == fixed,
                    };
                    if !fits
                        || items
                            .iter()
                            .any(|i| matches!(i, ast::DestructuringItem::Named(_)))
                    {
                        return unknown(pattern);
                    }
                    let mut out = Vec::new();
                    let after = spread.map_or(0, |s| items.len() - s - 1);
                    for (k, item) in items.iter().enumerate() {
                        match item {
                            ast::DestructuringItem::Pattern(p) => {
                                let at = match spread {
                                    Some(s) if k > s => a.len() - (items.len() - k),
                                    _ => k,
                                };
                                out.extend(destructure(*p, &a[at]));
                            }
                            ast::DestructuringItem::Spread(sp) => {
                                if let Some(id) = sp.sink_ident() {
                                    let rest = a[k..a.len() - after].to_vec();
                                    out.push((id.as_str().to_owned(), Value::Array(Rc::new(rest))));
                                }
                            }
                            ast::DestructuringItem::Named(_) => {}
                        }
                    }
                    out
                }
                Value::Dict(d) => {
                    let mut out = Vec::new();
                    let mut used = Vec::new();
                    for item in &items {
                        match item {
                            ast::DestructuringItem::Pattern(ast::Pattern::Normal(Expr::Ident(
                                id,
                            ))) => {
                                let key = id.as_str();
                                used.push(key.to_owned());
                                out.push((
                                    key.to_owned(),
                                    value.get(key).cloned().unwrap_or(Value::Unknown),
                                ));
                            }
                            ast::DestructuringItem::Named(n) => {
                                let key = n.name().as_str();
                                used.push(key.to_owned());
                                match value.get(key) {
                                    Some(v) => out.extend(destructure(n.pattern(), v)),
                                    None => out.extend(unknown(n.pattern())),
                                }
                            }
                            ast::DestructuringItem::Spread(sp) => {
                                if let Some(id) = sp.sink_ident() {
                                    let rest: Vec<_> = d
                                        .iter()
                                        .filter(|(k, _)| !used.contains(k))
                                        .cloned()
                                        .collect();
                                    out.push((id.as_str().to_owned(), Value::Dict(Rc::new(rest))));
                                }
                            }
                            ast::DestructuringItem::Pattern(p) => out.extend(unknown(*p)),
                        }
                    }
                    out
                }
                _ => unknown(pattern),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(list: &[(&str, &str)]) -> impl Fn(&Path) -> Option<String> {
        let map: HashMap<PathBuf, String> = list
            .iter()
            .map(|(p, t)| (PathBuf::from(p), (*t).to_owned()))
            .collect();
        move |p: &Path| map.get(p).cloned()
    }

    /// Стислий вигляд вмісту для порівняння.
    fn shape(items: &[Item]) -> String {
        let mut out = Vec::new();
        for item in items {
            out.push(match item {
                Item::Text(t) => format!("{t:?}"),
                Item::Space => continue,
                Item::Parbreak => "¶".into(),
                Item::Source(n) => format!("src({})", n.full_text()),
                Item::Body => "BODY".into(),
                Item::Label(l) => format!("<{l}>"),
                Item::Elem(e) => {
                    let mut args: Vec<String> = e
                        .args
                        .pos
                        .iter()
                        .map(|v| match v {
                            Value::Content(c) => shape(c),
                            v => format!("{v:?}"),
                        })
                        .collect();
                    for (k, v) in &e.args.named {
                        args.push(match v {
                            Value::Content(c) => format!("{k}: {}", shape(c)),
                            Value::Label(l) => format!("{k}: <{l}>"),
                            Value::Int(i) => format!("{k}: {i}"),
                            Value::None => format!("{k}: none"),
                            _ => format!("{k}: …"),
                        });
                    }
                    format!("{}({})", e.func, args.join(", "))
                }
                Item::Block(inner) => format!("block[{}]", shape(inner)),
                Item::Styled(effects, inner) => {
                    let sets: Vec<String> = effects
                        .iter()
                        .map(|e| match e {
                            Effect::Set(s) => {
                                let args: Vec<&str> =
                                    s.args.iter().map(|a| a.name.as_str()).collect();
                                format!("{}({})", s.target, args.join(","))
                            }
                            Effect::Opaque => "?".into(),
                            Effect::Show(s) => format!("show {}", s.elem),
                            Effect::Case(true) => "upper".into(),
                            Effect::Case(false) => "lower".into(),
                            Effect::Written(_) => "written".into(),
                        })
                        .collect();
                    format!("{}[{}]", sets.join("+"), shape(inner))
                }
                Item::Maybe(inner) => format!("maybe[{}]", shape(inner)),
                Item::Unknown => "…".into(),
                Item::Pagebreak => "⤓".into(),
                Item::H(_) => "h".into(),
                Item::Hole => "HOLE".into(),
                Item::Number => "N".into(),
                Item::Count(c, _) => format!("count({c})"),
                Item::Ctx(_) => "ctx".into(),
            });
        }
        out.join(" ")
    }

    /// Що дає `#show: <вираз>` у main.typ.
    fn show(list: &[(&str, &str)], transform: &str) -> String {
        let load = files(list);
        let mut ev = Evaluator::new(Path::new("/p"), &load);
        let src = crate::model::parse(format!("#({transform})"));
        let expr: Expr = src
            .root()
            .cast::<ast::Markup>()
            .unwrap()
            .exprs()
            .next()
            .unwrap();
        match ev.show_in(Path::new("/p/main.typ"), expr) {
            Value::Content(items) => shape(&items),
            v => format!("{v:?}"),
        }
    }

    /// Що дає виклик `call` у main.typ, з вмістом документа як посиланнями.
    fn doc_call(list: &[(&str, &str)], call: &str) -> String {
        let load = files(list);
        let mut ev = Evaluator::new(Path::new("/p"), &load);
        let src = crate::model::parse(format!("#{call}"));
        let expr: Expr = src
            .root()
            .cast::<ast::Markup>()
            .unwrap()
            .exprs()
            .next()
            .unwrap();
        match ev.eval_doc_in(Path::new("/p/main.typ"), expr) {
            Value::Content(items) => shape(&items),
            v => format!("{v:?}"),
        }
    }

    #[test]
    fn template_settings_and_body() {
        let lib = r#"
#let base = 14pt
#let tpl(doc, size: base) = {
  set page(paper: "a4")
  set text(size: size)
  show heading: set text(size: 20pt)
  align(center)[ЗВІТ]
  doc
  set text(size: 8pt)
  [Кінець]
}
"#;
        let list = [
            ("/p/main.typ", "#import \"/lib.typ\": *\n"),
            ("/p/lib.typ", lib),
        ];
        assert_eq!(
            show(&list, "tpl.with(size: 12pt)"),
            "page(paper)[text(size)[show heading[align(alignment)[block[\"ЗВІТ\"]] BODY text(size)[\"Кінець\"]]]]"
        );
    }

    #[test]
    fn nested_show_and_unknowns() {
        let lib = r#"
#let inner(it) = [#set text(size: 13pt); #it]
#let outer(body, big: false) = {
  show: inner
  set par(leading: 2em) if big
  if big [Великий] else [Звичайний]
  mystery(body)
}
"#;
        let list = [
            ("/p/main.typ", "#import \"lib.typ\": outer\n"),
            ("/p/lib.typ", lib),
        ];
        // Невідома функція: вміст лишається, налаштування невідомі
        assert_eq!(show(&list, "outer"), "text(size)[\"Звичайний\" …]");
        assert_eq!(
            show(&list, "outer.with(big: true)"),
            "text(size)[par(leading)[\"Великий\" …]]"
        );
        let list = [(
            "/p/main.typ",
            "#let w(x, flag: none) = if flag == none [#x] else [A #x]\n",
        )];
        assert_eq!(
            show(&list, "w.with(flag: unknown)"),
            "?[BODY] maybe[\"A\" BODY]"
        );
        assert_eq!(show(&list, "nothing"), "?[BODY]");
    }

    #[test]
    fn wrappers_produce_elements() {
        let lib = r#"
#let _required(name, value) = {
  if value == none { panic(name + " is required") }
  value
}
#let img-caption(cap, source) = if source == none { cap + " (самостійно)" } else { cap }
#let fig(file, caption, ..args) = {
  if type(file) == str { panic("path") }
  let source = args.pos().at(0, default: none)
  let named = args.named()
  let lbl = named.remove("label", default: none)
  [#figure(image(file, ..named), caption: img-caption(caption, source)) #lbl]
}
#let tab(caption: none, columns: none, header: none, label: none, ..args) = {
  let caption = _required("caption", caption)
  if type(header) != array { panic("header") }
  [#figure(table(columns: columns, table.header(..header), ..args.pos()), caption: caption) #label]
}
#let nheading(title) = heading(depth: 1, numbering: none, title)
"#;
        let list = [
            ("/p/main.typ", "#import \"lib.typ\": *\n"),
            ("/p/lib.typ", lib),
        ];
        assert_eq!(
            doc_call(&list, "fig(path(\"a.png\"), [Схема], label: <s>)"),
            "figure(image(Thing), caption: src([Схема]) \" (самостійно)\") <s>"
        );
        assert_eq!(
            doc_call(
                &list,
                "tab(caption: [Т], columns: 2, header: ([А], [Б]), label: <t>, [a], [1])"
            ),
            "figure(table(table.header(src([А]), src([Б])), src([a]), src([1]), columns: 2), caption: src([Т])) <t>"
        );
        assert_eq!(
            doc_call(&list, "nheading[Вступ]"),
            "heading(src([Вступ]), depth: 1, numbering: none)"
        );
        // Невідома функція документа: модель обходить виклик сама
        assert_eq!(doc_call(&list, "mystery[x]"), "src(mystery[x])");
    }

    #[test]
    fn package_and_toml() {
        // SAFETY: змінну задають лише тести з пакунками, і завжди однаково
        unsafe { std::env::set_var("TYPST_PACKAGE_PATH", "/pkgs") };
        let manifest =
            "[package]\nname = \"t\"\nversion = \"0.1.0\"\nentrypoint = \"src/lib.typ\"\n";
        let lib = "#import \"/src/style.typ\"\n#let tpl = style.dstu\n";
        let style = "#let dstu(it, size: 11pt, ..rest) = { set text(size: size); it }\n";
        let main = "#import \"@local/t:0.1.0\": tpl\n";
        let list = [
            ("/p/main.typ", main),
            ("/p/doc.toml", "[meta]\nn = 1\n"),
            ("/pkgs/local/t/0.1.0/typst.toml", manifest),
            ("/pkgs/local/t/0.1.0/src/lib.typ", lib),
            ("/pkgs/local/t/0.1.0/src/style.typ", style),
        ];
        assert_eq!(show(&list, "tpl"), "text(size)[BODY]");
        assert_eq!(
            show(&list, "tpl.with(..toml(\"/doc.toml\").meta)"),
            "text(size)[BODY]"
        );
    }
}
