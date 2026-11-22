//! Модель документа з вихідного коду, без компіляції.
//!
//! Від main-файлу обходимо синтаксичне дерево (`typst-syntax`), заходимо в
//! `#include` локальних файлів і збираємо в порядку документа те, що перевіряє
//! стандарт: заголовки, рисунки й таблиці, переліки, абзаци, формули,
//! посилання. Виклики функцій з коду (шаблонів, пакунків, самого документа)
//! обчислює [`crate::eval`]: які вбудовані елементи вони створюють
//! (`heading`, `figure`, `table`, `bibliography`, `outline`), з якими
//! налаштуваннями ([`crate::style`], нумерація, додатки) і де стоїть вміст
//! документа. Назви обгорток значення не мають: функцію, якої не обчислити,
//! модель обходить як контейнер, не вгадуючи, що вона робить.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use typst_syntax::ast::{self, AstNode, Expr};
use typst_syntax::{FileId, LinkedNode, RootedPath, Source, SyntaxNode, VirtualPath, VirtualRoot};

use std::rc::Rc;

use crate::eval::{self, Callee, Effect, Elem, Evaluator, Item as Part, SetArg, SetEffect, Value};
use crate::style::{Origin, Style};
use crate::text::{Loc, Text};

/// Спільний id для всіх файлів: span шукаємо лише в його власному дереві,
/// тож різні id не потрібні, а інтернер `typst-syntax` не росте з кожною
/// перевіркою.
static FILE_ID: LazyLock<FileId> = LazyLock::new(|| {
    RootedPath::new(VirtualRoot::Project, VirtualPath::new("/dsty.typ").unwrap()).intern()
});

/// Розбирає текст файлу.
pub fn parse(text: String) -> Source {
    Source::new(*FILE_ID, text)
}

pub struct SourceFile {
    pub path: PathBuf,
    pub source: Source,
}

#[derive(Default)]
pub struct Doc {
    pub files: Vec<SourceFile>,
    pub blocks: Vec<Block>,
    /// Посилання за міткою: порядкові номери в документі, за зростанням.
    pub refs: HashMap<String, Vec<usize>>,
    /// Є `bibliography`/`bib` (чи `bib-path` рецепта).
    pub has_bibliography: bool,
    /// `#include` файлів, яких не вдалося прочитати.
    pub missing: Vec<(Loc, PathBuf)>,
    /// Виноски в порядку документа (крім посилань на наявну виноску).
    pub footnotes: Vec<Loc>,
    /// Перше застосування шаблону (`#show: шаблон.with(…)`): сюди повна
    /// перевірка ставить зауваження до налаштувань, яких у коді не видно.
    pub template: Option<Loc>,
    /// `set`-правила (і show-set) у порядку документа.
    pub sets: Vec<SetRule>,
    /// Виклики й `set`-правила `bibliography`.
    pub bibs: Vec<Bib>,
    /// Кожне посилання на мітку (`@мітка`, `ref(<мітка>)`) з текстом перед ним.
    pub ref_uses: Vec<RefUse>,
    /// Власне show-правило `ref` у коді документа: як друкуються посилання,
    /// з коду не видно.
    pub custom_ref: bool,
    /// Налаштування тексту й сторінки ([`Par::style`]); перше — типові typst.
    pub styles: Vec<Style>,
}

/// Посилання на мітку в тексті.
#[derive(Debug)]
pub struct RefUse {
    pub target: String,
    pub loc: Loc,
    /// Підпис задано (`@x[рис.]`, `@x[]`, `ref(<x>, supplement: …)`).
    pub supplement: bool,
    /// Текст того самого абзацу перед посиланням (до 60 байтів).
    pub before: String,
}

/// `bibliography(…)` чи `set bibliography(…)`: аргументи, які перевіряє стандарт.
#[derive(Debug)]
pub struct Bib {
    /// Місце значення `full: true`.
    pub full: Option<Loc>,
    /// Назва й місце рядка в `style: "назва"`.
    pub style: Option<(String, Loc)>,
}

/// Канонічна назва структурного елемента, у якому стоїть блок
/// ([`crate::rules::BIBLIOGRAPHY`], [`crate::rules::ABBREVIATIONS`], …).
pub type Section = Option<&'static str>;

/// `set ціль(аргументи)`: де задано налаштування документа.
#[derive(Debug)]
pub struct SetRule {
    /// `page`, `text`, `par`, `footnote.entry`, …
    pub target: String,
    /// Назви іменованих аргументів.
    pub args: Vec<String>,
    /// Код значень іменованих аргументів (у порядку `args`).
    pub values: Vec<String>,
    /// Елемент show-set-правила (`show heading: set text(…)` -> `heading`):
    /// налаштування діє лише в ньому.
    pub selector: Option<String>,
    pub loc: Loc,
}

impl SetRule {
    fn matches(&self, target: &str, args: &[&str]) -> bool {
        self.target == target
            && (args.is_empty() || self.args.iter().any(|a| args.contains(&a.as_str())))
    }

    /// Код значення аргументу `name`.
    pub fn value(&self, name: &str) -> Option<&str> {
        let i = self.args.iter().position(|a| a == name)?;
        self.values.get(i).map(String::as_str)
    }
}

impl Doc {
    /// Місце налаштування `target` з аргументом `arg` (останнє в документі),
    /// інакше будь-якого `set target(…)`, інакше рядок шаблону, інакше перший
    /// непорожній рядок main-файлу. Show-set-правила (`show heading: set
    /// text(…)`) діють лише в елементі, тож тут не враховуються.
    pub fn setting(&self, target: &str, args: &[&str]) -> Loc {
        self.find_set(None, target, args)
            .or_else(|| self.find_set(None, target, &[]))
            .unwrap_or_else(|| self.anchor())
    }

    /// Налаштування `target` усередині елемента `selector` (`show heading:
    /// set text(size: …)`), інакше налаштування самого `selector`
    /// (`set heading(…)`), інакше те саме, що [`Self::setting`].
    pub fn setting_in(&self, selector: &str, target: &str, args: &[&str]) -> Loc {
        self.find_set(Some(selector), target, args)
            .or_else(|| self.find_set(None, selector, &[]))
            .unwrap_or_else(|| self.setting(target, args))
    }

    fn find_set(&self, selector: Option<&str>, target: &str, args: &[&str]) -> Option<Loc> {
        self.sets
            .iter()
            .rev()
            .find(|s| s.selector.as_deref() == selector && s.matches(target, args))
            .map(|s| s.loc.clone())
    }

    /// Останнє `set target(arg: …)` без селектора.
    pub fn set_rule(&self, target: &str, arg: &str) -> Option<&SetRule> {
        self.sets
            .iter()
            .rev()
            .find(|s| s.selector.is_none() && s.matches(target, &[arg]))
    }

    /// Рядок шаблону або перший непорожній рядок main-файлу.
    pub fn anchor(&self) -> Loc {
        if let Some(loc) = &self.template {
            return loc.clone();
        }
        let text = self.files.first().map_or("", |f| f.source.text());
        let mut start = 0;
        for line in text.split_inclusive('\n') {
            let trimmed = line.trim();
            if !trimmed.is_empty() && !trimmed.starts_with("//") {
                let lead = line.len() - line.trim_start().len();
                return Loc {
                    file: 0,
                    range: start + lead..start + lead + trimmed.len(),
                };
            }
            start += line.len();
        }
        Loc {
            file: 0,
            range: 0..0,
        }
    }
}

#[derive(Debug)]
pub enum Block {
    Heading(Heading),
    Figure(Figure),
    List(List),
    Par(Par),
    Equation {
        loc: Loc,
        label: Option<Label>,
        /// Формула йде одразу за іншою: між ними немає ні тексту, ні іншого
        /// блока (коду, таблиці, переліку), лише проміжки й невидимий код.
        follows: bool,
    },
    /// Ключові слова з аргументу шаблону (`abstract(keywords: …)`); шаблон
    /// сам сортує їх за абеткою.
    Keywords {
        count: usize,
        loc: Loc,
    },
    /// Ліва колонка переліку скорочень (7.14): терміни `/ ЕОМ: …` чи перша
    /// колонка таблиці.
    Terms(Vec<Text>),
}

/// Мітка елемента: `<назва>`, `label: <назва>`, `#label("назва")`; мітку з
/// виразу (`#label(str(...))` у циклі) з коду не дізнатися.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Label {
    Name(String),
    Dynamic,
}

#[derive(Debug)]
pub struct Heading {
    pub level: usize,
    pub title: Text,
    pub numbered: bool,
    /// Заголовок у додатках (`appendices`, `supplement: [Додаток]`).
    pub appendix: bool,
    pub label: Option<Label>,
    pub loc: Loc,
    /// Заголовок, який друкує шаблон (`abstract` -> «Реферат», `bib` -> «Перелік
    /// джерел посилання»): у коді його тексту немає, тож без виправлень.
    pub generated: bool,
    /// Заголовок розмітки (`= Назва`): місце вузла й код назви без мітки
    /// для виправлення на `#heading(…)[…]`.
    pub markup: Option<(Loc, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FigureKind {
    Image,
    Table,
}

#[derive(Debug)]
pub struct Figure {
    /// Місце в документі на одній шкалі з посиланнями ([`Doc::refs`]): чи
    /// стоїть рисунок раніше за перше посилання на нього.
    pub tick: usize,
    pub kind: FigureKind,
    pub caption: Option<Text>,
    pub label: Option<Label>,
    /// Головка таблиці (`table.header` / `header:` у `tab`).
    pub header: Option<Header>,
    /// Плаваюча фігура (`placement`): її місце в документі з коду не видно.
    pub floating: bool,
    pub loc: Loc,
}

#[derive(Debug)]
pub struct Header {
    pub cells: Vec<Text>,
    /// Головка в один рядок: клітинок не більше, ніж колонок.
    pub single_row: bool,
}

impl Header {
    /// Кількість колонок `columns`, якщо її видно з коду.
    fn new(cells: Vec<Text>, columns: Option<usize>) -> Self {
        Header {
            single_row: columns.is_some_and(|c| cells.len() <= c),
            cells,
        }
    }
}

#[derive(Debug)]
pub struct List {
    pub items: Vec<Item>,
    /// Абзац (індекс у `Doc::blocks`), що стоїть безпосередньо перед переліком.
    pub prev_par: Option<usize>,
    /// Вкладений перелік (позиція іншого переліку).
    pub nested: bool,
    pub section: Section,
}

#[derive(Debug)]
pub struct Item {
    pub text: Text,
    pub loc: Loc,
    /// За текстом позиції йде вкладений перелік.
    pub has_nested: bool,
}

#[derive(Debug)]
pub struct Par {
    pub text: Text,
    /// Абзац іде одразу після виокремленої формули.
    pub after_equation: bool,
    pub section: Section,
    /// Налаштування, що діють на абзац: індекс у [`Doc::styles`].
    pub style: usize,
}

/// Будує модель документа. `load` читає файл (відкритий буфер чи диск).
pub fn build(root: &Path, main: &Path, load: &dyn Fn(&Path) -> Option<String>) -> Option<Doc> {
    let text = load(main)?;
    let mut walker = Walker {
        root: root.to_path_buf(),
        load,
        ev: Evaluator::new(root, load),
        para_style: 0,
        doc: Doc {
            styles: vec![Style::default()],
            ..Doc::default()
        },
        tick: 0,
        including: vec![main.to_path_buf()],
        para: Text::default(),
        last: Last::None,
        section: None,
        template_text: String::new(),
        walked: 0,
    };
    let file = walker.add_file(main.to_path_buf(), text);
    let cx = walker.cx(file);
    let mut scope = Scope::default();
    walker.markup(&cx, cx.root().cast().unwrap(), &mut scope);
    walker.flush();
    Some(walker.doc)
}

/// Корінь проєкту для шляхів `/…` (як `typst --root`): тека main-файлу, а
/// якщо імпорти й `#include` main-файлу вказують на `/…`, якого там немає, чи
/// виходять за неї через `..`, то найближча тека вище, у якій усі вони
/// знаходяться. Вище теки з `typst.toml` чи `.git` не піднімається.
pub fn detect_root(main: &Path, load: &dyn Fn(&Path) -> Option<String>) -> PathBuf {
    let Some(dir) = main.parent() else {
        return PathBuf::from("/");
    };
    let Some(text) = load(main) else {
        return dir.to_path_buf();
    };
    let mut paths = Vec::new();
    module_paths(parse(text).root(), &mut paths);
    let fits = |root: &Path| {
        paths.iter().all(|p| match p.strip_prefix('/') {
            Some(rel) => root.join(rel).exists(),
            None => normalize(&dir.join(p)).starts_with(root),
        })
    };
    let mut candidate = dir;
    loop {
        if fits(candidate) {
            return candidate.to_path_buf();
        }
        if candidate.join("typst.toml").exists() || candidate.join(".git").exists() {
            break;
        }
        match candidate.parent() {
            Some(up) => candidate = up,
            None => break,
        }
    }
    dir.to_path_buf()
}

/// Шляхи локальних `import` і `include` у дереві.
fn module_paths(node: &SyntaxNode, out: &mut Vec<String>) {
    let source = node
        .cast::<ast::ModuleImport>()
        .map(|i| i.source())
        .or_else(|| node.cast::<ast::ModuleInclude>().map(|i| i.source()));
    if let Some(Expr::Str(s)) = source
        && !s.get().starts_with('@')
    {
        out.push(s.get().to_string());
    }
    for child in node.children() {
        module_paths(child, out);
    }
}

/// Налаштування, що діють на наступний вміст (як `set`/`show` у Typst).
#[derive(Debug, Clone, Copy, Default)]
struct Scope {
    /// Заголовки нумеруються: `set heading(numbering: …)` чи застосований шаблон.
    numbered: bool,
    appendix: bool,
    /// Налаштування тексту й сторінки: індекс у [`Doc::styles`].
    style: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Last {
    None,
    Par(usize),
    Equation,
    Other,
}

struct Cx {
    file: usize,
    source: Source,
}

impl Cx {
    fn root(&self) -> &SyntaxNode {
        self.source.root()
    }

    fn range(&self, node: &SyntaxNode) -> std::ops::Range<usize> {
        self.source
            .find(node.span())
            .map(|n| n.range())
            .unwrap_or(0..0)
    }

    fn loc(&self, node: &SyntaxNode) -> Loc {
        Loc {
            file: self.file,
            range: self.range(node),
        }
    }
}

struct Walker<'a> {
    root: PathBuf,
    load: &'a dyn Fn(&Path) -> Option<String>,
    ev: Evaluator<'a>,
    /// Налаштування на початку абзацу, що набирається.
    para_style: usize,
    doc: Doc,
    tick: usize,
    including: Vec<PathBuf>,
    /// Абзац, що набирається.
    para: Text,
    last: Last,
    /// Структурний елемент, у якому стоїмо, і рівень його заголовка.
    section: Option<(&'static str, usize)>,
    /// Текст, який друкує шаблон (не документ), у поточному абзаці: окремий
    /// рядок «РЕФЕРАТ» чи «ЗМІСТ» стає заголовком структурного елемента.
    template_text: String,
    /// Скільки разів обійдено вміст документа з аргументів обгорток.
    walked: usize,
}

impl Walker<'_> {
    fn add_file(&mut self, path: PathBuf, text: String) -> usize {
        let source = parse(text);
        self.ev.adopt(&path, source.clone());
        self.doc.files.push(SourceFile { path, source });
        self.doc.files.len() - 1
    }

    fn cx(&self, file: usize) -> Cx {
        Cx {
            file,
            source: self.doc.files[file].source.clone(),
        }
    }

    fn next_tick(&mut self) -> usize {
        self.tick += 1;
        self.tick
    }

    fn emit(&mut self, block: Block) {
        self.last = match &block {
            Block::Par(_) => Last::Par(self.doc.blocks.len()),
            Block::Equation { .. } => Last::Equation,
            Block::Keywords { .. } => self.last,
            _ => Last::Other,
        };
        self.doc.blocks.push(block);
    }

    /// Завершує абзац, що набирається.
    fn flush(&mut self) {
        let text = std::mem::take(&mut self.para);
        if text.trimmed().is_empty() {
            return;
        }
        let after_equation = self.last == Last::Equation;
        self.emit(Block::Par(Par {
            text,
            after_equation,
            section: self.section(),
            style: self.para_style,
        }));
    }

    fn section(&self) -> Section {
        self.section.map(|(name, _)| name)
    }

    fn add_ref(&mut self, target: &str) {
        let tick = self.next_tick();
        self.doc
            .refs
            .entry(target.to_owned())
            .or_default()
            .push(tick);
    }

    /// Запам'ятовує посилання з текстом абзацу перед ним.
    fn ref_use(&mut self, cx: &Cx, node: &SyntaxNode, target: &str, supplement: bool, out: &Text) {
        let text = out.as_str();
        let mut from = text.len().saturating_sub(60);
        while !text.is_char_boundary(from) {
            from += 1;
        }
        self.doc.ref_uses.push(RefUse {
            target: target.to_owned(),
            loc: cx.loc(node),
            supplement,
            before: text[from..].to_owned(),
        });
    }

    /// Посилання в коді, якого обхід тексту не читає (тіло фігури, клітинки
    /// таблиці в `tab`): лише щоб знати, на що посилаються.
    fn refs_in(&mut self, node: &SyntaxNode) {
        if let Some(r) = node.cast::<ast::Ref>() {
            self.add_ref(r.target());
            return;
        }
        if let Some(call) = node.cast::<ast::FuncCall>()
            && matches!(callee_name(call), Some("ref" | "link"))
            && let Some(Expr::Label(l)) = positional(call).first()
        {
            self.add_ref(l.get());
        }
        for child in node.children() {
            self.refs_in(child);
        }
    }

    // --- Розмітка -----------------------------------------------------------

    fn markup(&mut self, cx: &Cx, markup: ast::Markup, scope: &mut Scope) {
        let exprs: Vec<Expr> = markup.exprs().collect();
        self.markup_exprs(cx, &exprs, scope);
    }

    fn markup_exprs(&mut self, cx: &Cx, exprs: &[Expr], scope: &mut Scope) {
        let mut i = 0;
        while i < exprs.len() {
            let expr = exprs[i];
            if self.para.trimmed().is_empty() {
                self.para_style = scope.style;
            }
            match expr {
                // `#show: шаблон` діє на решту розмітки: шаблон сам вирішує, де її вивести
                Expr::ShowRule(show) if show.selector().is_none() => {
                    self.show_rule(cx, show, scope, &exprs[i + 1..], true);
                    return;
                }
                Expr::Parbreak(_) => self.flush(),
                Expr::Heading(h) => {
                    self.flush();
                    self.markup_heading(cx, h, *scope, label_after(exprs, i));
                }
                Expr::ListItem(_) | Expr::EnumItem(_) => {
                    // Позиції одного переліку: того самого виду, між ними лише пробіли
                    let enumerated = matches!(expr, Expr::EnumItem(_));
                    let mut items = vec![expr];
                    let mut last = i;
                    let mut j = i + 1;
                    while j < exprs.len() {
                        match exprs[j] {
                            Expr::Space(_) | Expr::Parbreak(_) => {}
                            Expr::ListItem(_) if !enumerated => {
                                items.push(exprs[j]);
                                last = j;
                            }
                            Expr::EnumItem(_) if enumerated => {
                                items.push(exprs[j]);
                                last = j;
                            }
                            _ => break,
                        }
                        j += 1;
                    }
                    self.flush();
                    let bodies = items
                        .iter()
                        .filter_map(|&item| Some((item.to_untyped().clone(), item_body(item)?)))
                        .collect::<Vec<_>>();
                    self.list(cx, &bodies, false, *scope);
                    i = last + 1;
                    continue;
                }
                Expr::Equation(eq) if eq.block() => {
                    self.flush();
                    let follows = self.last == Last::Equation;
                    let label = label_after(exprs, i);
                    self.emit(Block::Equation {
                        loc: cx.loc(expr.to_untyped()),
                        label,
                        follows,
                    });
                }
                Expr::Raw(raw) if raw.block() => {
                    self.flush();
                    self.last = Last::Other;
                }
                Expr::TermItem(_) => {
                    // Терміни одного переліку: між ними лише пробіли
                    let mut terms = Vec::new();
                    let mut j = i;
                    let mut last = i;
                    while j < exprs.len() {
                        match exprs[j] {
                            Expr::TermItem(term) => {
                                terms.push(term);
                                last = j;
                            }
                            Expr::Space(_) | Expr::Parbreak(_) => {}
                            _ => break,
                        }
                        j += 1;
                    }
                    self.flush();
                    let mut texts = Vec::new();
                    for term in terms {
                        let mut text = Text::default();
                        self.inline_markup(cx, term.term(), &mut text);
                        texts.push(text);
                        // Опис обходимо заради посилань у ньому
                        let mut ignored = Text::default();
                        self.inline_markup(cx, term.description(), &mut ignored);
                    }
                    self.terms(texts);
                    self.last = Last::Other;
                    i = last + 1;
                    continue;
                }
                Expr::FuncCall(call) => self.call(cx, call, scope, label_after(exprs, i), true),
                _ if is_inline(expr) => {
                    let mut text = std::mem::take(&mut self.para);
                    self.inline(cx, expr, &mut text);
                    self.para = text;
                }
                _ => self.code_expr(cx, expr, scope),
            }
            i += 1;
        }
    }

    /// Вирази блоку коду: `#show: шаблон` діє на решту блоку.
    fn code_exprs(&mut self, cx: &Cx, exprs: &[Expr], scope: &mut Scope) {
        for (i, &e) in exprs.iter().enumerate() {
            if let Expr::ShowRule(show) = e
                && show.selector().is_none()
            {
                self.show_rule(cx, show, scope, &exprs[i + 1..], false);
                return;
            }
            self.code_expr(cx, e, scope);
        }
    }

    /// Вираз у режимі коду (тіло `#{…}`, `#if`, вміст з аргументів обгорток).
    fn code_expr(&mut self, cx: &Cx, expr: Expr, scope: &mut Scope) {
        match expr {
            Expr::FuncCall(call) => self.call(cx, call, scope, None, false),
            Expr::ContentBlock(block) => {
                let mut inner = *scope;
                self.markup(cx, block.body(), &mut inner);
            }
            Expr::CodeBlock(block) => {
                let mut inner = *scope;
                let exprs: Vec<Expr> = block.body().exprs().collect();
                self.code_exprs(cx, &exprs, &mut inner);
            }
            Expr::Parenthesized(p) => self.code_expr(cx, p.expr(), scope),
            Expr::Array(array) => {
                for item in array.items() {
                    if let ast::ArrayItem::Pos(e) = item {
                        self.code_expr(cx, e, scope);
                    }
                }
            }
            Expr::SetRule(set) => {
                self.record_set(cx, set, None);
                if callee_path(set.target()).last() == Some(&"bibliography") {
                    self.bib(cx, set.args());
                }
                let file = self.doc.files[cx.file].path.clone();
                if let Some(effect) = self.ev.set_in(&file, set) {
                    self.restyle(
                        scope,
                        Some(&[Effect::Set(effect)]),
                        &cx.loc(set.to_untyped()),
                    );
                }
            }
            Expr::ShowRule(show) if show.selector().is_some() => {
                self.show_selector(cx, show, scope)
            }
            Expr::ShowRule(show) => self.show_rule(cx, show, scope, &[], false),
            Expr::ModuleInclude(include) => {
                self.flush();
                self.include(cx, include, *scope);
            }
            Expr::Conditional(cond) => {
                self.code_expr(cx, cond.if_body(), &mut scope.clone());
                if let Some(body) = cond.else_body() {
                    self.code_expr(cx, body, &mut scope.clone());
                }
            }
            Expr::ForLoop(f) => self.code_expr(cx, f.body(), &mut scope.clone()),
            Expr::WhileLoop(w) => self.code_expr(cx, w.body(), &mut scope.clone()),
            Expr::Contextual(c) => self.code_expr(cx, c.body(), scope),
            // Визначення (`let`), імпорти, значення: друкованого вмісту не дають
            _ => {}
        }
    }

    fn markup_heading(&mut self, cx: &Cx, h: ast::Heading, scope: Scope, after: Option<Label>) {
        let mut title = Text::default();
        let mut label = after;
        for e in h.body().exprs() {
            match e {
                Expr::Label(l) => label = Some(Label::Name(l.get().to_owned())),
                _ => self.inline(cx, e, &mut title),
            }
        }
        code_title(cx, &mut title, h.body().to_untyped());
        let loc = title.whole().unwrap_or_else(|| cx.loc(h.to_untyped()));
        // Код назви без мітки всередині (`= Вступ <intro>`)
        let body: String = h
            .body()
            .exprs()
            .filter(|e| !matches!(e, Expr::Label(_)))
            .map(|e| e.to_untyped().full_text().to_string())
            .collect();
        self.heading(Heading {
            level: h.depth().get(),
            title,
            numbered: scope.numbered,
            appendix: scope.appendix,
            label,
            loc,
            generated: false,
            markup: Some((cx.loc(h.to_untyped()), body.trim().to_owned())),
        });
    }

    fn heading(&mut self, heading: Heading) {
        match crate::rules::structural(heading.title.trimmed()) {
            Some((name, _)) if heading.level <= 2 => self.section = Some((name, heading.level)),
            _ if self
                .section
                .is_some_and(|(_, level)| heading.level <= level) =>
            {
                self.section = None;
            }
            _ => {}
        }
        self.emit(Block::Heading(heading));
    }

    /// Заголовок, який друкує шаблон.
    fn generated_heading(&mut self, title: &str, loc: Loc) {
        let mut text = Text::default();
        text.push_mapped(title, loc.file, loc.range.clone());
        self.heading(Heading {
            level: 1,
            title: text,
            numbered: false,
            appendix: false,
            label: None,
            loc,
            generated: true,
            markup: None,
        });
    }

    /// Перелік: тексти позицій без вкладених переліків; вкладені йдуть окремо
    /// після нього, без перевірки двокрапки.
    fn list(&mut self, cx: &Cx, items: &[(SyntaxNode, ast::Markup)], nested: bool, scope: Scope) {
        let prev_par = match self.last {
            Last::Par(i) if !nested => Some(i),
            _ => None,
        };
        let mut out = Vec::new();
        let mut rest: Vec<Vec<(SyntaxNode, ast::Markup)>> = Vec::new();
        let mut blocks: Vec<Expr> = Vec::new();
        for (node, body) in items {
            let mut text = Text::default();
            let mut inner: Vec<(SyntaxNode, ast::Markup)> = Vec::new();
            let mut inner_enum = None;
            for e in body.exprs() {
                match e {
                    Expr::ListItem(_) | Expr::EnumItem(_) => {
                        let is_enum = matches!(e, Expr::EnumItem(_));
                        // Інший вид вкладеного переліку стає окремим переліком
                        if inner_enum.is_some_and(|en| en != is_enum) {
                            rest.push(std::mem::take(&mut inner));
                        }
                        inner_enum = Some(is_enum);
                        if let Some(b) = item_body(e) {
                            inner.push((e.to_untyped().clone(), b));
                        }
                    }
                    Expr::Parbreak(_) => text.push_mapped(" ", cx.file, cx.range(e.to_untyped())),
                    Expr::Equation(eq) if eq.block() => blocks.push(e),
                    Expr::FuncCall(call) if self.is_block_call(cx, call) => blocks.push(e),
                    Expr::FuncCall(_) => self.inline(cx, e, &mut text),
                    _ if is_inline(e) => self.inline(cx, e, &mut text),
                    _ => blocks.push(e),
                }
            }
            if !inner.is_empty() {
                rest.push(inner);
            }
            out.push(Item {
                text,
                loc: cx.loc(node),
                has_nested: inner_enum.is_some(),
            });
        }
        let section = self.section();
        self.emit(Block::List(List {
            items: out,
            prev_par,
            nested,
            section,
        }));
        for inner in rest {
            self.list(cx, &inner, true, scope);
        }
        let mut scope = scope;
        for e in blocks {
            self.code_expr(cx, e, &mut scope);
            self.flush();
        }
        self.last = Last::Other;
    }

    // --- Текст абзацу ---------------------------------------------------------

    fn inline_markup(&mut self, cx: &Cx, markup: ast::Markup, out: &mut Text) {
        for e in markup.exprs() {
            if let Expr::Parbreak(_) = e {
                out.push_mapped(" ", cx.file, cx.range(e.to_untyped()));
            } else {
                self.inline(cx, e, out);
            }
        }
    }

    /// Текст вбудованого елемента; посилання записуються.
    fn inline(&mut self, cx: &Cx, expr: Expr, out: &mut Text) {
        let node = expr.to_untyped();
        match expr {
            Expr::Text(t) => out.push_exact(t.get(), cx.file, cx.range(node).start),
            Expr::Space(_) | Expr::Linebreak(_) => out.push_mapped(" ", cx.file, cx.range(node)),
            Expr::Escape(e) => out.push_mapped(&e.get().to_string(), cx.file, cx.range(node)),
            Expr::Shorthand(s) => out.push_mapped(&s.get().to_string(), cx.file, cx.range(node)),
            Expr::SmartQuote(q) => {
                out.push_mapped(if q.double() { "\"" } else { "'" }, cx.file, cx.range(node))
            }
            Expr::Strong(s) => self.inline_markup(cx, s.body(), out),
            Expr::Emph(e) => self.inline_markup(cx, e.body(), out),
            Expr::Raw(raw) => {
                let text: Vec<&str> = raw.lines().map(|l| l.get().as_str()).collect();
                out.push_mapped(&text.join(" "), cx.file, cx.range(node));
            }
            Expr::Link(link) => out.push_exact(link.get(), cx.file, cx.range(node).start),
            Expr::Ref(r) => {
                self.add_ref(r.target());
                self.ref_use(cx, node, r.target(), r.supplement().is_some(), out);
                out.push_mapped(&node.full_text(), cx.file, cx.range(node));
            }
            Expr::Equation(_) => out.push_mapped(&node.full_text(), cx.file, cx.range(node)),
            Expr::FuncCall(call) => self.inline_call(cx, call, out),
            Expr::ContentBlock(block) => self.inline_markup(cx, block.body(), out),
            Expr::Str(s) => out.push_mapped(&s.get(), cx.file, cx.range(node)),
            // `#змінна`, `#x.y`, мітки, правила: тексту в коді немає
            _ => {}
        }
    }

    /// Вбудований виклик: текст його аргументів-вмісту.
    fn inline_call(&mut self, cx: &Cx, call: ast::FuncCall, out: &mut Text) {
        match callee_name(call) {
            // Текст виноски друкується внизу сторінки, а не в рядку
            Some("footnote") => {
                if !matches!(positional(call).first(), Some(Expr::Label(_))) {
                    self.doc.footnotes.push(cx.loc(call.to_untyped()));
                }
                for arg in content_args(call) {
                    let mut ignored = Text::default();
                    self.inline(cx, arg, &mut ignored);
                }
            }
            Some("ref") => {
                let node = call.to_untyped();
                if let Some(Expr::Label(l)) = positional(call).first() {
                    self.add_ref(l.get());
                    let supplement = named(call, "supplement").is_some();
                    self.ref_use(cx, node, l.get(), supplement, out);
                }
                out.push_mapped(&node.full_text(), cx.file, cx.range(node));
            }
            // `link(<мітка>)[текст]` теж посилається на елемент
            Some("link") => {
                if let Some(Expr::Label(l)) = positional(call).first() {
                    self.add_ref(l.get());
                }
                for arg in content_args(call) {
                    self.inline(cx, arg, out);
                }
            }
            _ => {
                for arg in content_args(call) {
                    self.inline(cx, arg, out);
                }
            }
        }
    }

    // --- Виклики функцій --------------------------------------------------------

    /// Виклик у розмітці (`markup`) чи в коді: вбудовані елементи, функції з
    /// коду (шаблону, пакунка, документа) обчислюються, невідомі
    /// обробляються як контейнер чи текст.
    fn call(
        &mut self,
        cx: &Cx,
        call: ast::FuncCall,
        scope: &mut Scope,
        label: Option<Label>,
        in_markup: bool,
    ) {
        self.keywords(cx, call.args());
        let file = self.doc.files[cx.file].path.clone();
        match self.ev.callee_in(&file, call) {
            Callee::Builtin(name) => self.builtin_call(cx, call, &name, scope, label, in_markup),
            Callee::User => {
                let node = call.to_untyped();
                let value = self.ev.eval_doc_in(&file, Expr::FuncCall(call));
                let site = cx.loc(node);
                let walked = self.walked;
                let mut inner = *scope;
                let mut rest = None;
                self.items(
                    cx,
                    &value.items(),
                    &mut inner,
                    &site,
                    label,
                    &mut rest,
                    in_markup,
                );
                self.flush_template(&site);
                // Функцію не обчислити: її вміст однаково перевіряється, але
                // налаштування для нього невідомі
                if self.walked == walked && has_content_args(call) {
                    let mut inner = *scope;
                    self.restyle(&mut inner, None, &site);
                    self.generic_call(cx, call, "", &mut inner, in_markup);
                }
            }
            Callee::Unknown => self.generic_call(cx, call, "", scope, in_markup),
        }
    }

    /// Вбудована функція документа.
    fn builtin_call(
        &mut self,
        cx: &Cx,
        call: ast::FuncCall,
        name: &str,
        scope: &mut Scope,
        label: Option<Label>,
        in_markup: bool,
    ) {
        match name {
            // Елементи, які модель збирає: ті самі, що з обгорток, але з коду
            // документа (виправлення на місці, `full:` переліку джерел)
            "heading" | "figure" | "list" | "enum" | "bibliography" | "outline" => {
                self.flush();
                if name == "bibliography" {
                    self.bib(cx, call.args());
                }
                let file = self.doc.files[cx.file].path.clone();
                let value = self.ev.eval_doc_in(&file, Expr::FuncCall(call));
                let site = cx.loc(call.to_untyped());
                let mut label = label;
                for item in value.items() {
                    if let Part::Elem(e) = item {
                        self.elem(cx, &e, scope, label.take(), &site, true);
                    }
                }
            }
            _ => self.generic_call(cx, call, name, scope, in_markup),
        }
    }

    /// Виклик без особливого значення для моделі: контейнер (`align`, `block`,
    /// невідома обгортка з абзацами) чи текст у рядку.
    fn generic_call(
        &mut self,
        cx: &Cx,
        call: ast::FuncCall,
        name: &str,
        scope: &mut Scope,
        in_markup: bool,
    ) {
        if is_block_name(name) || !in_markup || self.has_blocks(cx, call) {
            // Контейнер: його вміст складається з окремих абзаців
            self.flush();
            if matches!(name, "table" | "grid") {
                let file = self.doc.files[cx.file].path.clone();
                let value = self.ev.eval_doc_in(&file, Expr::FuncCall(call));
                let site = cx.loc(call.to_untyped());
                if let Some(table) = find_elem(&value, name) {
                    self.elem_terms(cx, &table, &site);
                }
            }
            let mut inner = *scope;
            // `#text(size: 12pt)[…]`: налаштування для вмісту
            if matches!(name, "text" | "par" | "page") {
                let file = self.doc.files[cx.file].path.clone();
                if let Some(effect) = self.ev.call_set_in(&file, call) {
                    self.restyle(
                        &mut inner,
                        Some(&[Effect::Set(effect)]),
                        &cx.loc(call.to_untyped()),
                    );
                }
            }
            for arg in content_args(call) {
                self.code_expr(cx, arg, &mut inner);
                self.flush();
            }
            // Проміжок чи розрив сторінки між формулами не відокремлює їх
            // текстом (7.10.11)
            let spacing = matches!(name, "v" | "pagebreak" | "colbreak");
            if is_block_name(name) && !(spacing && self.last == Last::Equation) {
                self.last = Last::Other;
            }
        } else {
            let mut text = std::mem::take(&mut self.para);
            self.inline_call(cx, call, &mut text);
            self.para = text;
        }
    }

    /// Чи дає виклик блок (заголовок, рисунок, контейнер), а не текст у рядку.
    fn is_block_call(&mut self, cx: &Cx, call: ast::FuncCall) -> bool {
        let file = self.doc.files[cx.file].path.clone();
        match self.ev.callee_in(&file, call) {
            Callee::Builtin(name) => {
                is_block_name(&name) || matches!(name.as_str(), "heading" | "figure")
            }
            Callee::User => {
                let value = self.ev.eval_doc_in(&file, Expr::FuncCall(call));
                has_block(&value.items())
            }
            Callee::Unknown => false,
        }
    }

    /// Чи є у вмісті виклику абзаци, заголовки чи переліки.
    fn has_blocks(&mut self, cx: &Cx, call: ast::FuncCall) -> bool {
        for arg in content_args(call) {
            let Expr::ContentBlock(block) = arg else {
                continue;
            };
            for e in block.body().exprs() {
                let block = match e {
                    Expr::Parbreak(_)
                    | Expr::Heading(_)
                    | Expr::ListItem(_)
                    | Expr::EnumItem(_) => true,
                    Expr::Equation(eq) => eq.block(),
                    Expr::FuncCall(c) => self.is_block_call(cx, c),
                    _ => false,
                };
                if block {
                    return true;
                }
            }
        }
        false
    }

    // --- Вміст із шаблонів і обгорток -------------------------------------------

    /// Вміст, який дала функція з коду: вміст документа (його код) обходиться
    /// на своєму місці під налаштуваннями шаблону, вбудовані елементи стають
    /// блоками моделі. `rest`: решта документа після `#show: шаблон` (де її
    /// виводить шаблон, [`Part::Body`]); `site`: виклик у документі.
    #[allow(clippy::too_many_arguments)]
    fn items<'e>(
        &mut self,
        cx: &Cx,
        items: &[Part],
        scope: &mut Scope,
        site: &Loc,
        mut label: Option<Label>,
        rest: &mut Option<&[Expr<'e>]>,
        markup: bool,
    ) {
        for (k, item) in items.iter().enumerate() {
            match item {
                Part::Text(t) => self.template_text.push_str(t),
                Part::Space => self.template_text.push(' '),
                Part::Parbreak => self.flush_template(site),
                Part::Unknown | Part::Label(_) => {}
                Part::Source(node) => {
                    self.flush_template(site);
                    self.walked += 1;
                    if let Some(expr) = node.cast::<Expr>() {
                        self.code_expr(cx, expr, scope);
                    }
                }
                Part::Body => {
                    self.flush_template(site);
                    if let Some(exprs) = rest.take() {
                        let mut inner = *scope;
                        if markup {
                            self.markup_exprs(cx, exprs, &mut inner);
                        } else {
                            self.code_exprs(cx, exprs, &mut inner);
                        }
                    }
                }
                Part::Block(inner) => {
                    self.flush_template(site);
                    self.flush();
                    self.items(cx, inner, &mut scope.clone(), site, None, rest, markup);
                    self.flush_template(site);
                    self.flush();
                    self.last = Last::Other;
                }
                Part::Styled(effects, inner) => {
                    let mut scope = *scope;
                    self.restyle(&mut scope, Some(effects), site);
                    let label = label.take();
                    self.items(cx, inner, &mut scope, site, label, rest, markup);
                }
                Part::Maybe(inner) => {
                    if has_elem(inner, "bibliography") {
                        self.doc.has_bibliography = true;
                    }
                }
                Part::Elem(e) => {
                    self.flush_template(site);
                    // Мітка після елемента (`[#figure(…) #label]`) чи після виклику
                    let after = items[k + 1..]
                        .iter()
                        .find(|i| !matches!(i, Part::Space))
                        .and_then(|i| match i {
                            Part::Label(l) if l.is_empty() => Some(Label::Dynamic),
                            Part::Label(l) => Some(Label::Name(l.clone())),
                            // Щось невідоме після елемента може бути міткою
                            Part::Unknown => Some(Label::Dynamic),
                            _ => None,
                        });
                    let lbl = match e.func.as_str() {
                        "heading" | "figure" => after.or_else(|| label.take()),
                        _ => after,
                    };
                    self.elem(cx, e, scope, lbl, site, false);
                }
            }
        }
    }

    /// Текст шаблону в абзаці закінчився: «РЕФЕРАТ» окремим рядком є
    /// заголовком структурного елемента.
    fn flush_template(&mut self, site: &Loc) {
        let text = std::mem::take(&mut self.template_text);
        let text = text.trim();
        if !text.is_empty()
            && text.chars().count() <= 40
            && crate::rules::structural(text).is_some()
        {
            self.flush();
            self.generated_heading(text, site.clone());
        }
    }

    /// Вбудований елемент із вмісту шаблону чи обгортки; `doc`: виклик
    /// просто в коді документа.
    fn elem(
        &mut self,
        cx: &Cx,
        e: &Elem,
        scope: &mut Scope,
        label: Option<Label>,
        site: &Loc,
        doc: bool,
    ) {
        match e.func.as_str() {
            "heading" => {
                self.flush();
                let body = e.body();
                let mut title = Text::default();
                if let Some(body) = body {
                    self.value_text(cx, body, site, true, &mut title);
                    if let Some(node) = first_source(body) {
                        code_title(cx, &mut title, &node);
                    }
                }
                let level = ["level", "depth"]
                    .iter()
                    .find_map(|n| match e.named(n) {
                        Some(Value::Int(i)) => Some((*i).max(1) as usize),
                        _ => None,
                    })
                    .unwrap_or(1);
                let numbered = match e.named("numbering") {
                    Some(Value::None) => false,
                    Some(_) => true,
                    None => scope.numbered,
                };
                let generated = !doc && !body.is_some_and(|b| first_source(b).is_some());
                let loc = title.whole().unwrap_or_else(|| site.clone());
                self.heading(Heading {
                    level,
                    title,
                    numbered,
                    appendix: scope.appendix,
                    label,
                    loc,
                    generated,
                    markup: None,
                });
            }
            "figure" => {
                self.flush();
                self.last = Last::Other;
                self.figure(cx, e, label, site);
            }
            "bibliography" => {
                self.flush();
                // `full:` і `style:` перевіряються лише з коду документа: шаблон
                // може сам друкувати перелік інакше (`show bibliography`)
                self.doc.has_bibliography = true;
                match e.named("title") {
                    Some(Value::None) => {}
                    Some(v @ (Value::Content(_) | Value::Str(_))) => {
                        let mut title = Text::default();
                        self.value_text(cx, v, site, true, &mut title);
                        let title = title.trimmed().to_owned();
                        self.generated_heading(&title, site.clone());
                    }
                    _ => self.generated_heading("Перелік джерел посилання", site.clone()),
                }
            }
            "outline" => {
                self.flush();
                if !matches!(e.named("title"), Some(Value::None)) {
                    self.generated_heading("Зміст", site.clone());
                }
            }
            "list" | "enum" => {
                self.flush();
                let nodes: Vec<SyntaxNode> = e.args.pos.iter().filter_map(first_source).collect();
                let items: Vec<(SyntaxNode, ast::Markup)> = nodes
                    .iter()
                    .filter_map(|node| {
                        let body = node.cast::<ast::ContentBlock>()?.body();
                        Some((node.clone(), body))
                    })
                    .collect();
                if !items.is_empty() {
                    self.list(cx, &items, false, *scope);
                }
            }
            "table" | "grid" => {
                self.flush();
                self.elem_terms(cx, e, site);
                self.last = Last::Other;
            }
            "image" | "raw" | "math.equation" => {
                self.flush();
                self.last = Last::Other;
            }
            _ => {}
        }
    }

    /// Текст значення: вміст документа з місцями в коді, текст шаблону (якщо
    /// `template`) на місці виклику.
    fn value_text(&mut self, cx: &Cx, value: &Value, site: &Loc, template: bool, out: &mut Text) {
        match value {
            Value::Str(s) => self.str_text(cx, s, site, template, out),
            Value::Content(items) => self.items_text(cx, items, site, template, out),
            _ => {}
        }
    }

    /// Рядок, що прийшов з коду документа (`fig(…, "Підпис")`): шукаємо його
    /// літерал у виклику, щоб текст мав місце в коді. Решта рядка (дописана
    /// шаблоном) — на місці виклику, якщо `template`.
    fn str_text(&mut self, cx: &Cx, s: &str, site: &Loc, template: bool, out: &mut Text) {
        let mut best: Option<(String, std::ops::Range<usize>, bool)> = None;
        str_literals(
            &LinkedNode::new(cx.root()),
            &site.range,
            &mut |value, range, exact| {
                if !value.is_empty()
                    && s.starts_with(value.as_str())
                    && best.as_ref().is_none_or(|(b, _, _)| value.len() > b.len())
                {
                    best = Some((value, range, exact));
                }
            },
        );
        let used = match best {
            Some((value, range, exact)) => {
                if exact {
                    out.push_exact(&value, site.file, range.start + 1);
                } else {
                    out.push_mapped(&value, site.file, range);
                }
                value.len()
            }
            None => 0,
        };
        if template || used == 0 {
            out.push_mapped(&s[used..], site.file, site.range.clone());
        }
    }

    fn items_text(&mut self, cx: &Cx, items: &[Part], site: &Loc, template: bool, out: &mut Text) {
        for item in items {
            match item {
                Part::Source(node) => {
                    if let Some(expr) = node.cast::<Expr>() {
                        self.inline(cx, expr, out);
                    }
                }
                Part::Text(t) if template => out.push_mapped(t, site.file, site.range.clone()),
                Part::Space if template => out.push_mapped(" ", site.file, site.range.clone()),
                Part::Styled(_, inner) | Part::Block(inner) => {
                    self.items_text(cx, inner, site, template, out)
                }
                _ => {}
            }
        }
    }

    fn figure(&mut self, cx: &Cx, e: &Elem, label: Option<Label>, site: &Loc) {
        let body = e.args.pos.first().or_else(|| e.named("body"));
        // Посилання в тілі (клітинки таблиці, підписи всередині)
        if let Some(Value::Content(items)) = body {
            self.refs_in_items(items);
        }
        if matches!(e.named("numbering"), Some(Value::None)) {
            return;
        }
        let table = body.and_then(|b| find_elem(b, "table"));
        if let Some(t) = &table {
            self.elem_terms(cx, t, site);
        }
        let kind = match e.named("kind") {
            Some(Value::Func(f)) => match &**f {
                eval::Func::Builtin(k) if k == "table" => FigureKind::Table,
                eval::Func::Builtin(k) if k == "image" => FigureKind::Image,
                // Лістинги й власні види фігур стандарт так не описує
                _ => return,
            },
            Some(_) => return,
            None => match body.map(|b| self.body_kind(cx, b)) {
                Some(BodyKind::Table) => FigureKind::Table,
                Some(BodyKind::Image) | None => FigureKind::Image,
                Some(BodyKind::Raw | BodyKind::Unknown) => return,
            },
        };
        let caption = e.named("caption").and_then(|c| {
            let c = match c {
                // `figure.caption(…)[назва]`
                Value::Content(items) => match items.first() {
                    Some(Part::Elem(inner)) if inner.func == "figure.caption" => inner.body()?,
                    _ => c,
                },
                Value::None => return None,
                c => c,
            };
            let mut text = Text::default();
            self.value_text(cx, c, site, false, &mut text);
            // Підпис лише з тексту шаблону чи виразу (`"…" + $b$`)
            if text.trimmed().is_empty() {
                self.value_text(cx, c, site, true, &mut text);
            }
            Some(text)
        });
        let header = table.as_ref().and_then(|t| {
            let columns = match t.named("columns") {
                None => Some(1),
                Some(Value::Int(i)) => Some((*i).max(1) as usize),
                Some(Value::Array(a)) => Some(a.len().max(1)),
                Some(_) => None,
            };
            t.args.pos.iter().find_map(|v| {
                let h = find_elem(v, "table.header")?;
                let cells: Vec<Text> = h
                    .args
                    .pos
                    .iter()
                    .map(|cell| {
                        let cell = match find_elem(cell, "table.cell")
                            .as_ref()
                            .and_then(|c| c.body().cloned())
                        {
                            Some(body) => body,
                            None => cell.clone(),
                        };
                        let mut text = Text::default();
                        self.value_text(cx, &cell, site, false, &mut text);
                        text
                    })
                    .collect();
                Some(Header::new(cells, columns))
            })
        });
        let floating = e
            .named("placement")
            .is_some_and(|p| !matches!(p, Value::None));
        let tick = self.next_tick();
        self.emit(Block::Figure(Figure {
            tick,
            kind,
            caption,
            label,
            header,
            floating,
            loc: site.clone(),
        }));
    }

    /// Що в тілі рисунка: таблиця, лістинг, зображення.
    fn body_kind(&mut self, cx: &Cx, value: &Value) -> BodyKind {
        let Value::Content(items) = value else {
            return BodyKind::Image;
        };
        let mut found = BodyKind::Image;
        for item in items.iter() {
            let kind = match item {
                Part::Elem(e) => match e.func.as_str() {
                    "table" => BodyKind::Table,
                    "raw" => BodyKind::Raw,
                    "image" => BodyKind::Image,
                    _ => continue,
                },
                Part::Block(inner) | Part::Styled(_, inner) => {
                    self.body_kind(cx, &Value::Content(inner.clone()))
                }
                // Вміст відомої функції, якого не обчислити (`cetz.canvas`)
                Part::Unknown => continue,
                Part::Source(node) => self.source_kind(cx, node),
                _ => continue,
            };
            match kind {
                BodyKind::Image => {}
                BodyKind::Unknown => found = BodyKind::Unknown,
                decisive => return decisive,
            }
        }
        found
    }

    /// Що в коді документа (тілі рисунка): виклики обчислюються.
    fn source_kind(&mut self, cx: &Cx, node: &SyntaxNode) -> BodyKind {
        let Some(expr) = node.cast::<Expr>() else {
            return BodyKind::Image;
        };
        let exprs: Vec<Expr> = match expr {
            Expr::Raw(_) => return BodyKind::Raw,
            Expr::ContentBlock(b) => b.body().exprs().collect(),
            e => vec![e],
        };
        let file = self.doc.files[cx.file].path.clone();
        let mut found = BodyKind::Image;
        for e in exprs {
            let kind = match e {
                Expr::Raw(_) => BodyKind::Raw,
                Expr::FuncCall(call) => {
                    let value = self.ev.eval_doc_in(&file, e);
                    match value.items().as_slice() {
                        // Невідома функція: що вона дає, не видно
                        [Part::Source(n)] if n.span() == call.span() => BodyKind::Unknown,
                        _ => self.body_kind(cx, &value),
                    }
                }
                _ => continue,
            };
            match kind {
                BodyKind::Image => {}
                BodyKind::Unknown => found = BodyKind::Unknown,
                decisive => return decisive,
            }
        }
        found
    }

    /// Посилання у вмісті документа з аргументів.
    fn refs_in_items(&mut self, items: &[Part]) {
        for item in items {
            match item {
                Part::Source(node) => self.refs_in(node),
                Part::Block(inner) | Part::Styled(_, inner) => self.refs_in_items(inner),
                Part::Elem(e) => {
                    for v in e.args.pos.iter().chain(e.args.named.iter().map(|(_, v)| v)) {
                        if let Value::Content(inner) = v {
                            self.refs_in_items(inner);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Перша колонка таблиці-елемента в переліку скорочень.
    fn elem_terms(&mut self, cx: &Cx, table: &Elem, site: &Loc) {
        if self.section() != Some(crate::rules::ABBREVIATIONS) {
            return;
        }
        let columns = match table.named("columns") {
            None => 1,
            Some(Value::Int(i)) => (*i).max(1) as usize,
            Some(Value::Array(a)) => a.len().max(1),
            Some(_) => return,
        };
        let cells: Vec<Value> = table
            .args
            .pos
            .iter()
            .filter(|v| {
                !matches!(v, Value::Content(items) if matches!(
                    items.first(),
                    Some(Part::Elem(e)) if e.func.ends_with("header")
                        || e.func.ends_with("footer")
                        || e.func.ends_with("hline")
                        || e.func.ends_with("vline")
                ))
            })
            .cloned()
            .collect();
        let texts = cells
            .iter()
            .step_by(columns)
            .map(|cell| {
                let mut text = Text::default();
                self.value_text(cx, cell, site, false, &mut text);
                text
            })
            .collect();
        self.terms(texts);
    }

    /// `keywords: (…)` у виклику (`abstract`) чи словнику рецепта.
    fn keywords(&mut self, cx: &Cx, args: ast::Args) {
        for arg in args.items() {
            if let ast::Arg::Named(n) = arg {
                self.keywords_named(cx, n);
            }
        }
    }

    fn keywords_named(&mut self, cx: &Cx, n: ast::Named) {
        match (n.name().as_str(), n.expr()) {
            ("keywords", Expr::Array(array)) => {
                let mut count = 0;
                for item in array.items() {
                    match item {
                        ast::ArrayItem::Pos(_) => count += 1,
                        // `..змінна`: кількість з коду не видно
                        ast::ArrayItem::Spread(_) => return,
                    }
                }
                self.emit(Block::Keywords {
                    count,
                    loc: cx.loc(array.to_untyped()),
                });
            }
            (_, Expr::Dict(dict)) => {
                for item in dict.items() {
                    if let ast::DictItem::Named(inner) = item {
                        self.keywords_named(cx, inner);
                    }
                }
            }
            _ => {}
        }
    }

    /// Аргументи `bibliography`, які перевіряє стандарт.
    fn bib(&mut self, cx: &Cx, args: ast::Args) {
        let full = match named_in(args, "full") {
            Some(e @ Expr::Bool(b)) if b.get() => Some(cx.loc(e.to_untyped())),
            _ => None,
        };
        let style = match named_in(args, "style") {
            Some(e @ Expr::Str(s)) => Some((s.get().to_string(), cx.loc(e.to_untyped()))),
            _ => None,
        };
        self.doc.bibs.push(Bib { full, style });
    }

    /// Терміни переліку скорочень (лише в ньому).
    fn terms(&mut self, terms: Vec<Text>) {
        if self.section() == Some(crate::rules::ABBREVIATIONS) && terms.len() > 1 {
            self.emit(Block::Terms(terms));
        }
    }

    // --- Правила й файли ----------------------------------------------------------

    fn record_set(&mut self, cx: &Cx, set: ast::SetRule, selector: Option<String>) {
        let target = callee_path(set.target()).join(".");
        let (args, values) = set
            .args()
            .items()
            .filter_map(|arg| match arg {
                ast::Arg::Named(n) => Some((
                    n.name().as_str().to_owned(),
                    n.expr().to_untyped().full_text().to_string(),
                )),
                _ => None,
            })
            .unzip();
        self.doc.sets.push(SetRule {
            target,
            args,
            values,
            selector,
            loc: cx.loc(set.to_untyped()),
        });
    }

    /// `show селектор: …`: налаштування елемента.
    fn show_selector(&mut self, cx: &Cx, show: ast::ShowRule, scope: &mut Scope) {
        let Some(selector) = show.selector() else {
            return;
        };
        // `show heading.where(level: 1): …` -> `heading`
        let mut path = callee_path(selector);
        if path.last() == Some(&"where") {
            path.pop();
        }
        let name = path.join(".");
        let body = eval::is_body_selector(selector);
        match show.transform() {
            // `show footnote.entry: set text(size: 12pt)` налаштовує сам елемент
            Expr::SetRule(set) => {
                self.record_set(cx, set, Some(name.clone()));
                // `show par: set text(…)` діє на весь основний текст
                if body {
                    let file = self.doc.files[cx.file].path.clone();
                    if let Some(effect) = self.ev.set_in(&file, set) {
                        self.restyle(
                            scope,
                            Some(&[Effect::Set(effect)]),
                            &cx.loc(set.to_untyped()),
                        );
                    }
                }
            }
            // `show par: it => …`: абзаци змінено невідомо як
            _ if body => self.restyle(scope, None, &cx.loc(show.to_untyped())),
            _ if name == "ref" => self.doc.custom_ref = true,
            _ => {}
        }
    }

    /// `#show: шаблон`: шаблон обчислюється над рештою документа (`rest`) і
    /// сам вирішує, де й з якими налаштуваннями її вивести.
    fn show_rule(
        &mut self,
        cx: &Cx,
        show: ast::ShowRule,
        scope: &mut Scope,
        rest: &[Expr],
        markup: bool,
    ) {
        let walk = |this: &mut Self, exprs: &[Expr], scope: &mut Scope| {
            if markup {
                this.markup_exprs(cx, exprs, scope);
            } else {
                this.code_exprs(cx, exprs, scope);
            }
        };
        let transform = show.transform();
        // `show: set text(…)`: те саме, що `set`
        if matches!(transform, Expr::SetRule(_)) {
            self.code_expr(cx, transform, scope);
            walk(self, rest, scope);
            return;
        }
        let site = cx.loc(show.to_untyped());
        if self.doc.template.is_none() {
            self.doc.template = Some(site.clone());
        }
        if let Expr::FuncCall(call) = transform {
            self.keywords(cx, call.args());
        }
        let file = self.doc.files[cx.file].path.clone();
        let shown = self.ev.show_in(&file, transform);
        let mut pending = Some(rest);
        self.items(
            cx,
            &shown.items(),
            &mut scope.clone(),
            &site,
            None,
            &mut pending,
            markup,
        );
        self.flush_template(&site);
        // Де шаблон виводить решту документа, не видно: її однаково
        // перевіряємо, але налаштування для неї невідомі
        if let Some(rest) = pending {
            let mut inner = *scope;
            self.restyle(&mut inner, None, &site);
            walk(self, rest, &mut inner);
        }
    }

    /// Нові налаштування для вмісту після `effects`; `None`: невідомо які
    /// (шаблону не простежити). `site`: що в документі їх застосовує.
    fn restyle(&mut self, scope: &mut Scope, effects: Option<&[Effect]>, site: &Loc) {
        let mut style = self.doc.styles[scope.style].clone();
        match effects {
            None => style.forget(),
            Some(effects) => {
                for effect in effects {
                    match effect {
                        Effect::Opaque => style.forget(),
                        Effect::Set(set) => {
                            style.apply(set, &|arg: &SetArg| self.origin(set, arg, site));
                            if set.target == "heading" {
                                heading_set(scope, set);
                            }
                        }
                    }
                }
            }
        }
        self.doc.styles.push(style);
        scope.style = self.doc.styles.len() - 1;
    }

    /// Де задано аргумент `set`: у файлі документа чи (для шаблону поза
    /// документом) рядок, що застосовує шаблон.
    fn origin(&self, set: &SetEffect, arg: &SetArg, site: &Loc) -> Origin {
        if let Some(file) = self.doc.files.iter().position(|f| f.path == set.path) {
            let source = &self.doc.files[file].source;
            let range = |span| source.find(span).map(|n| n.range());
            if let Some(r) = range(set.span) {
                return Origin {
                    loc: Loc { file, range: r },
                    value: range(arg.span).map(|range| Loc { file, range }),
                    template: None,
                };
            }
        }
        let shown = set
            .path
            .strip_prefix(&self.root)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| {
                set.path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
        Origin {
            loc: site.clone(),
            value: None,
            template: Some(shown),
        }
    }

    fn include(&mut self, cx: &Cx, include: ast::ModuleInclude, scope: Scope) {
        let Expr::Str(path) = include.source() else {
            return;
        };
        let path = path.get();
        let resolved = match path.strip_prefix('/') {
            Some(rel) => self.root.join(rel),
            None => self.doc.files[cx.file]
                .path
                .parent()
                .unwrap_or(&self.root)
                .join(path.as_str()),
        };
        let resolved = normalize(&resolved);
        if self.including.contains(&resolved) {
            return;
        }
        let file = match self.doc.files.iter().position(|f| f.path == resolved) {
            Some(file) => file,
            None => match (self.load)(&resolved) {
                Some(text) => self.add_file(resolved.clone(), text),
                None => {
                    self.doc
                        .missing
                        .push((cx.loc(include.to_untyped()), resolved));
                    return;
                }
            },
        };
        let inner_cx = self.cx(file);
        self.including.push(resolved);
        let mut inner = scope;
        let root = inner_cx.root().clone();
        if let Some(markup) = root.cast::<ast::Markup>() {
            self.markup(&inner_cx, markup, &mut inner);
        }
        self.flush();
        self.including.pop();
    }
}

fn item_body(e: Expr) -> Option<ast::Markup> {
    match e {
        Expr::ListItem(it) => Some(it.body()),
        Expr::EnumItem(it) => Some(it.body()),
        _ => None,
    }
}

/// Мітка після елемента в розмітці: `#figure(…) <мітка>`, `#label("мітка")`.
fn label_after(exprs: &[Expr], i: usize) -> Option<Label> {
    for &e in &exprs[i + 1..] {
        match e {
            Expr::Space(_) => continue,
            Expr::Label(l) => return Some(Label::Name(l.get().to_owned())),
            Expr::FuncCall(call) if callee_name(call) == Some("label") => {
                return Some(label_value(e));
            }
            _ => return None,
        }
    }
    None
}

/// Мітка з виразу: `<x>`, `label("x")`, інакше невідома.
fn label_value(expr: Expr) -> Label {
    match expr {
        Expr::Label(l) => Label::Name(l.get().to_owned()),
        Expr::FuncCall(call) if callee_name(call) == Some("label") => {
            match positional(call).first() {
                Some(Expr::Str(s)) => Label::Name(s.get().to_string()),
                _ => Label::Dynamic,
            }
        }
        _ => Label::Dynamic,
    }
}

/// Заголовок, увесь текст якого є кодом (`= #lorem(3)`, `#heading[#f]`): для
/// повідомлень береться сам код.
fn code_title(cx: &Cx, title: &mut Text, body: &SyntaxNode) {
    if title.trimmed().is_empty() {
        let code = body.full_text();
        let code = code.trim();
        let content = code
            .strip_prefix('[')
            .and_then(|c| c.strip_suffix(']'))
            .unwrap_or(code)
            .trim();
        *title = Text::default();
        title.push_mapped(content, cx.file, cx.range(body));
    }
}

/// `set heading(…)`: нумерація й додатки (`supplement: [Додаток]`, нумерація
/// літерою `"А.1"`).
fn heading_set(scope: &mut Scope, set: &SetEffect) {
    if let Some(a) = set.arg("numbering") {
        match &a.value {
            Value::None => scope.numbered = false,
            Value::Unknown => {}
            Value::Str(p) => {
                scope.numbered = true;
                if p.trim_start().starts_with(['А', 'A']) {
                    scope.appendix = true;
                }
            }
            _ => scope.numbered = true,
        }
    }
    // Функція нумерації дає першому заголовку літеру: «А»
    if set
        .arg("numbering-first")
        .and_then(|a| a.value.text())
        .is_some_and(|t| t.trim_start().starts_with(['А', 'A']))
    {
        scope.appendix = true;
    }
    if set
        .arg("supplement")
        .and_then(|a| a.value.text())
        .is_some_and(|t| t.contains("Додаток"))
    {
        scope.appendix = true;
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum BodyKind {
    Image,
    Table,
    Raw,
    /// Тіло з функції, якої не обчислити.
    Unknown,
}

/// Рядкові літерали всередині `range`: (значення, місце, чи текст у коді
/// збігається зі значенням, тобто без екранування).
fn str_literals(
    node: &LinkedNode,
    range: &std::ops::Range<usize>,
    f: &mut dyn FnMut(String, std::ops::Range<usize>, bool),
) {
    let r = node.range();
    if r.end <= range.start || r.start >= range.end {
        return;
    }
    if let Some(lit) = node.get().cast::<ast::Str>() {
        let value = lit.get().to_string();
        let code = node.get().full_text();
        let exact = code.len() >= 2 && code[1..code.len() - 1] == *value;
        f(value, r, exact);
        return;
    }
    for child in node.children() {
        str_literals(&child, range, f);
    }
}

/// Перший вузол документа у значенні.
fn first_source(value: &Value) -> Option<SyntaxNode> {
    fn find(items: &[Part]) -> Option<SyntaxNode> {
        items.iter().find_map(|i| match i {
            Part::Source(node) => Some(node.clone()),
            Part::Styled(_, inner) | Part::Block(inner) => find(inner),
            _ => None,
        })
    }
    match value {
        Value::Content(items) => find(items),
        _ => None,
    }
}

/// Перший елемент `func` у значенні (крізь блоки й налаштування).
fn find_elem(value: &Value, func: &str) -> Option<Rc<Elem>> {
    fn find(items: &[Part], func: &str) -> Option<Rc<Elem>> {
        items.iter().find_map(|i| match i {
            Part::Elem(e) if e.func == func => Some(e.clone()),
            Part::Styled(_, inner) | Part::Block(inner) => find(inner, func),
            _ => None,
        })
    }
    match value {
        Value::Content(items) => find(items, func),
        _ => None,
    }
}

/// Чи є у вмісті елемент `func` (зокрема лише можливий).
fn has_elem(items: &[Part], func: &str) -> bool {
    items.iter().any(|i| match i {
        Part::Elem(e) => e.func == func,
        Part::Styled(_, inner) | Part::Block(inner) | Part::Maybe(inner) => has_elem(inner, func),
        _ => false,
    })
}

/// Чи дає вміст блок, а не текст у рядку.
fn has_block(items: &[Part]) -> bool {
    items.iter().any(|i| match i {
        Part::Elem(e) => matches!(
            e.func.as_str(),
            "heading"
                | "figure"
                | "table"
                | "grid"
                | "list"
                | "enum"
                | "bibliography"
                | "outline"
                | "image"
        ),
        Part::Block(_) | Part::Parbreak => true,
        Part::Styled(_, inner) => has_block(inner),
        _ => false,
    })
}

/// Чи є у виклику аргументи-вміст документа.
fn has_content_args(call: ast::FuncCall) -> bool {
    positional(call)
        .iter()
        .any(|e| matches!(e, Expr::ContentBlock(_) | Expr::ModuleInclude(_)))
}

/// Шлях виклику: `style.appendices` -> `["style", "appendices"]`.
fn callee_path<'a>(expr: Expr<'a>) -> Vec<&'a str> {
    match expr {
        Expr::Ident(i) => vec![i.as_str()],
        Expr::FieldAccess(f) => {
            let mut path = callee_path(f.target());
            path.push(f.field().as_str());
            path
        }
        Expr::FuncCall(c) => callee_path(c.callee()),
        _ => Vec::new(),
    }
}

/// Назва функції без `.with`: `style.dstu.with(…)` -> `dstu`.
fn template_name<'a>(path: &[&'a str]) -> Option<&'a str> {
    match path {
        [.., name, "with"] => Some(name),
        [.., name] => Some(name),
        [] => None,
    }
}

fn callee_name<'a>(call: ast::FuncCall<'a>) -> Option<&'a str> {
    template_name(&callee_path(call.callee()))
}

fn positional(call: ast::FuncCall) -> Vec<Expr> {
    call.args()
        .items()
        .filter_map(|arg| match arg {
            ast::Arg::Pos(e) => Some(e),
            _ => None,
        })
        .collect()
}

fn named<'a>(call: ast::FuncCall<'a>, name: &str) -> Option<Expr<'a>> {
    named_in(call.args(), name)
}

fn named_in<'a>(args: ast::Args<'a>, name: &str) -> Option<Expr<'a>> {
    args.items().find_map(|arg| match arg {
        ast::Arg::Named(n) if n.name().as_str() == name => Some(n.expr()),
        _ => None,
    })
}

/// Позиційні аргументи-вміст: `[…]` і вирази, що його дають.
fn content_args(call: ast::FuncCall) -> Vec<Expr> {
    positional(call)
        .into_iter()
        .filter(|e| {
            matches!(
                e,
                Expr::ContentBlock(_) | Expr::FuncCall(_) | Expr::Strong(_) | Expr::Emph(_)
            )
        })
        .collect()
}

/// Вбудовані елементи абзацу.
fn is_inline(expr: Expr) -> bool {
    match expr {
        Expr::Text(_)
        | Expr::Space(_)
        | Expr::Linebreak(_)
        | Expr::Escape(_)
        | Expr::Shorthand(_)
        | Expr::SmartQuote(_)
        | Expr::Strong(_)
        | Expr::Emph(_)
        | Expr::Link(_)
        | Expr::Ref(_)
        | Expr::Label(_)
        | Expr::Ident(_)
        | Expr::FieldAccess(_)
        | Expr::Str(_) => true,
        Expr::Raw(raw) => !raw.block(),
        Expr::Equation(eq) => !eq.block(),
        _ => false,
    }
}

/// Функції, що дають блок, а не текст у рядку.
fn is_block_name(name: &str) -> bool {
    matches!(
        name,
        "align"
            | "block"
            | "columns"
            | "colbreak"
            | "grid"
            | "image"
            | "line"
            | "pad"
            | "pagebreak"
            | "place"
            | "quote"
            | "rect"
            | "stack"
            | "table"
            | "v"
    )
}

/// Шлях без `.` і `..` (файлу може ще не бути, тож без `canonicalize`).
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}
