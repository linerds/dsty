//! Модель документа з вихідного коду, без компіляції.
//!
//! Від main-файлу обходимо синтаксичне дерево (`typst-syntax`), заходимо в
//! `#include` локальних файлів і збираємо в порядку документа те, що перевіряє
//! стандарт: заголовки, рисунки й таблиці, переліки, абзаци, формули,
//! посилання. Пакунки (`@preview/…`) не читаються: обгортки шаблонів
//! (`fig`, `tab`, `bib`, `appendices`, `nheading`…) розпізнаються за назвою.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use typst_syntax::ast::{self, AstNode, Expr};
use typst_syntax::{FileId, RootedPath, Source, SyntaxNode, VirtualPath, VirtualRoot};

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
    /// інакше рядок шаблону, інакше перший непорожній рядок main-файлу.
    /// Show-set-правила (`show heading: set text(…)`) діють лише в елементі,
    /// тож тут не враховуються.
    pub fn setting(&self, target: &str, args: &[&str]) -> Loc {
        self.find_set(None, target, args)
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
}

/// Будує модель документа. `load` читає файл (відкритий буфер чи диск).
pub fn build(root: &Path, main: &Path, load: &dyn Fn(&Path) -> Option<String>) -> Option<Doc> {
    let text = load(main)?;
    let mut walker = Walker {
        root: root.to_path_buf(),
        load,
        doc: Doc::default(),
        tick: 0,
        including: vec![main.to_path_buf()],
        para: Text::default(),
        last: Last::None,
        deferred: Vec::new(),
        section: None,
    };
    let file = walker.add_file(main.to_path_buf(), text);
    let cx = walker.cx(file);
    let mut scope = Scope::default();
    walker.markup(&cx, cx.root().cast().unwrap(), &mut scope);
    walker.flush();
    walker.run_deferred();
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
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Last {
    None,
    Par(usize),
    Equation,
    Other,
}

/// Вміст рецепта шаблону, який друкується в кінці документа
/// (`#show: coursework.with(bib-path: …, appendices: […])`).
enum Deferred {
    Bibliography(Loc),
    Appendices(usize, SyntaxNode),
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
    doc: Doc,
    tick: usize,
    including: Vec<PathBuf>,
    /// Абзац, що набирається.
    para: Text,
    last: Last,
    deferred: Vec<Deferred>,
    /// Структурний елемент, у якому стоїмо, і рівень його заголовка.
    section: Option<(&'static str, usize)>,
}

impl Walker<'_> {
    fn add_file(&mut self, path: PathBuf, text: String) -> usize {
        self.doc.files.push(SourceFile {
            path,
            source: parse(text),
        });
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
        let mut i = 0;
        while i < exprs.len() {
            let expr = exprs[i];
            match expr {
                Expr::Parbreak(_) => self.flush(),
                Expr::Heading(h) => {
                    self.flush();
                    self.markup_heading(cx, h, *scope, label_after(&exprs, i));
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
                    let label = label_after(&exprs, i);
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
                Expr::FuncCall(call) => self.call(cx, call, scope, label_after(&exprs, i), true),
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

    /// Вираз у режимі коду (тіло `#{…}`, `#if`, аргумент `appendices`).
    fn code_expr(&mut self, cx: &Cx, expr: Expr, scope: &mut Scope) {
        match expr {
            Expr::FuncCall(call) => self.call(cx, call, scope, None, false),
            Expr::ContentBlock(block) => {
                let mut inner = *scope;
                self.markup(cx, block.body(), &mut inner);
            }
            Expr::CodeBlock(block) => {
                let mut inner = *scope;
                for e in block.body().exprs() {
                    self.code_expr(cx, e, &mut inner);
                }
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
                apply_set(set, scope);
            }
            Expr::ShowRule(show) => self.show_rule(cx, show, scope),
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
                    Expr::FuncCall(call) if is_block_call(call) => blocks.push(e),
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

    /// Текст одного виразу (назва, клітинка).
    fn text_of(&mut self, cx: &Cx, expr: Expr) -> Text {
        let mut text = Text::default();
        self.inline(cx, expr, &mut text);
        text
    }

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

    /// Виклик у розмітці (`markup`) чи в коді: блокові елементи, обгортки
    /// шаблонів, решта обробляється як вбудований текст.
    fn call(
        &mut self,
        cx: &Cx,
        call: ast::FuncCall,
        scope: &mut Scope,
        label: Option<Label>,
        in_markup: bool,
    ) {
        self.keywords(cx, call.args());
        let name = callee_name(call).unwrap_or_default();
        let node = call.to_untyped();
        match name {
            "heading" | "nheading" => {
                self.flush();
                let mut title = Text::default();
                // Назва буває вмістом або рядком: `#heading("ІСПИТ")`
                let body = named(call, "body")
                    .or_else(|| content_args(call).last().copied())
                    .or_else(|| {
                        positional(call)
                            .into_iter()
                            .rfind(|e| matches!(e, Expr::Str(_)))
                    });
                if let Some(body) = body {
                    self.inline(cx, body, &mut title);
                    code_title(cx, &mut title, body.to_untyped());
                }
                let level = ["level", "depth"]
                    .iter()
                    .find_map(|n| match named(call, n) {
                        Some(Expr::Int(i)) => Some(i.get().max(1) as usize),
                        _ => None,
                    })
                    .unwrap_or(1);
                let numbered = name == "heading"
                    && match named(call, "numbering") {
                        Some(Expr::None(_)) => false,
                        Some(_) => true,
                        None => scope.numbered,
                    };
                let loc = title.whole().unwrap_or_else(|| cx.loc(node));
                self.heading(Heading {
                    level,
                    title,
                    numbered,
                    appendix: scope.appendix,
                    label,
                    loc,
                    generated: false,
                    markup: None,
                });
            }
            "figure" => {
                self.flush();
                self.figure(cx, call, label);
            }
            "fig" | "dstu-figure" => {
                self.flush();
                let caption = named(call, "caption").or_else(|| positional(call).get(1).copied());
                let caption = caption.map(|c| self.text_of(cx, c));
                let label = label_arg(call).or(label);
                let tick = self.next_tick();
                self.emit(Block::Figure(Figure {
                    tick,
                    kind: FigureKind::Image,
                    caption,
                    label,
                    header: None,
                    floating: false,
                    loc: cx.loc(node),
                }));
            }
            "tab" | "dstu-table" => {
                self.flush();
                for cell in positional(call) {
                    self.refs_in(cell.to_untyped());
                }
                let caption = named(call, "caption").map(|c| self.text_of(cx, c));
                let columns = columns(named(call, "columns"));
                let header = match named(call, "header") {
                    Some(Expr::Array(array)) => {
                        let cells: Vec<Text> = array
                            .items()
                            .filter_map(|item| match item {
                                ast::ArrayItem::Pos(e) => Some(e),
                                _ => None,
                            })
                            .map(|e| self.text_of(cx, e))
                            .collect();
                        Some(Header::new(cells, columns))
                    }
                    _ => None,
                };
                let label = label_arg(call).or(label);
                let tick = self.next_tick();
                self.emit(Block::Figure(Figure {
                    tick,
                    kind: FigureKind::Table,
                    caption,
                    label,
                    header,
                    floating: false,
                    loc: cx.loc(node),
                }));
            }
            "list" | "enum" => {
                self.flush();
                let items: Vec<(SyntaxNode, ast::Markup)> = positional(call)
                    .into_iter()
                    .filter_map(|e| match e {
                        Expr::ContentBlock(b) => Some((e.to_untyped().clone(), b.body())),
                        _ => None,
                    })
                    .collect();
                if !items.is_empty() {
                    self.list(cx, &items, false, *scope);
                }
            }
            "bibliography" | "bib" | "dstu-bibliography" => {
                self.flush();
                self.doc.has_bibliography = true;
                self.bib(cx, call.args());
                if !matches!(named(call, "title"), Some(Expr::None(_))) {
                    self.generated_heading("Перелік джерел посилання", cx.loc(node));
                }
            }
            "outline" | "contents" => {
                self.flush();
                if !matches!(named(call, "title"), Some(Expr::None(_))) {
                    self.generated_heading("Зміст", cx.loc(node));
                }
            }
            "abstract" => {
                self.flush();
                self.generated_heading("Реферат", cx.loc(node));
                for arg in content_args(call) {
                    let mut inner = *scope;
                    self.code_expr(cx, arg, &mut inner);
                    self.flush();
                }
            }
            "appendices" => {
                self.flush();
                let mut inner = Scope {
                    numbered: true,
                    appendix: true,
                };
                for arg in positional(call) {
                    self.code_expr(cx, arg, &mut inner);
                }
                self.flush();
            }
            _ if is_block_name(name) || !in_markup || has_blocks(call) => {
                // Контейнер (`align`, `block`, власна обгортка з абзацами): його
                // вміст складається з окремих абзаців
                self.flush();
                if matches!(name, "table" | "grid") {
                    self.table_terms(cx, call);
                }
                let mut inner = *scope;
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
            }
            _ => {
                let mut text = std::mem::take(&mut self.para);
                self.inline_call(cx, call, &mut text);
                self.para = text;
            }
        }
    }

    fn figure(&mut self, cx: &Cx, call: ast::FuncCall, label: Option<Label>) {
        let node = call.to_untyped();
        let body = positional(call).first().copied();
        if let Some(body) = body {
            self.refs_in(body.to_untyped());
        }
        if matches!(named(call, "numbering"), Some(Expr::None(_))) {
            return;
        }
        let table = body.and_then(find_table);
        if let Some(t) = table {
            self.table_terms(cx, t);
        }
        let kind = match named(call, "kind") {
            Some(Expr::Ident(i)) if i.as_str() == "table" => FigureKind::Table,
            Some(Expr::Ident(i)) if i.as_str() == "image" => FigureKind::Image,
            // Лістинги й власні види фігур стандарт так не описує
            Some(_) => return,
            None if table.is_some() => FigureKind::Table,
            None if body.is_some_and(is_listing) => return,
            None => FigureKind::Image,
        };
        let caption = named(call, "caption").map(|c| {
            let c = match c {
                // `figure.caption(…)[назва]`
                Expr::FuncCall(inner) => content_args(inner).last().copied().unwrap_or(c),
                c => c,
            };
            self.text_of(cx, c)
        });
        let header = table.and_then(|t| {
            let columns = columns(named(t, "columns"));
            positional(t).into_iter().find_map(|e| match e {
                Expr::FuncCall(h) if callee_path(h.callee()).ends_with(&["table", "header"]) => {
                    let cells: Vec<Text> = positional(h)
                        .into_iter()
                        .map(|cell| {
                            let cell = match cell {
                                Expr::FuncCall(c) => {
                                    content_args(c).last().copied().unwrap_or(cell)
                                }
                                cell => cell,
                            };
                            self.text_of(cx, cell)
                        })
                        .collect();
                    Some(Header::new(cells, columns))
                }
                _ => None,
            })
        });
        let floating = named(call, "placement").is_some_and(|p| !matches!(p, Expr::None(_)));
        let tick = self.next_tick();
        self.emit(Block::Figure(Figure {
            tick,
            kind,
            caption,
            label,
            header,
            floating,
            loc: cx.loc(node),
        }));
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

    /// Перша колонка таблиці в переліку скорочень.
    fn table_terms(&mut self, cx: &Cx, table: ast::FuncCall) {
        if self.section() != Some(crate::rules::ABBREVIATIONS) {
            return;
        }
        // Кількість колонок з виразу (`(auto,) * 2`, змінна) не видно: без
        // неї першої колонки не знайти
        let Some(columns) = columns(named(table, "columns")) else {
            return;
        };
        let cells: Vec<Expr> = positional(table)
            .into_iter()
            .filter(|e| {
                !matches!(e, Expr::FuncCall(c) if matches!(
                    callee_path(c.callee()).last(),
                    Some(&("header" | "footer" | "hline" | "vline"))
                ))
            })
            .collect();
        let texts = cells
            .into_iter()
            .step_by(columns)
            .map(|cell| self.text_of(cx, cell))
            .collect();
        self.terms(texts);
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

    fn show_rule(&mut self, cx: &Cx, show: ast::ShowRule, scope: &mut Scope) {
        if let Some(selector) = show.selector() {
            // `show heading.where(level: 1): …` -> `heading`
            let mut path = callee_path(selector);
            if path.last() == Some(&"where") {
                path.pop();
            }
            let name = path.join(".");
            match show.transform() {
                // `show footnote.entry: set text(size: 12pt)` налаштовує сам елемент
                Expr::SetRule(set) => self.record_set(cx, set, Some(name)),
                _ if name == "ref" => self.doc.custom_ref = true,
                _ => {}
            }
            return;
        }
        if self.doc.template.is_none() {
            self.doc.template = Some(cx.loc(show.to_untyped()));
        }
        let transform = show.transform();
        let (path, call) = match transform {
            Expr::FuncCall(call) => (callee_path(call.callee()), Some(call)),
            e => (callee_path(e), None),
        };
        let name = template_name(&path);
        if name == Some("appendices") {
            self.flush();
            scope.appendix = true;
            scope.numbered = true;
            return;
        }
        // `#show: шаблон` вмикає нумерацію заголовків, як `style.dstu`
        scope.numbered = true;
        let Some(call) = call else { return };
        for arg in call.args().items() {
            let ast::Arg::Named(n) = arg else { continue };
            match n.name().as_str() {
                "abstract" if !matches!(n.expr(), Expr::None(_)) => {
                    self.generated_heading("Реферат", cx.loc(n.to_untyped()));
                    self.keywords_named(cx, n);
                }
                "bib-path" | "bib" | "bibliography" if !matches!(n.expr(), Expr::None(_)) => {
                    self.doc.has_bibliography = true;
                    self.deferred
                        .push(Deferred::Bibliography(cx.loc(n.to_untyped())));
                }
                "appendices" => self
                    .deferred
                    .push(Deferred::Appendices(cx.file, n.expr().to_untyped().clone())),
                _ => {}
            }
        }
    }

    fn run_deferred(&mut self) {
        for deferred in std::mem::take(&mut self.deferred) {
            match deferred {
                Deferred::Bibliography(loc) => {
                    self.generated_heading("Перелік джерел посилання", loc)
                }
                Deferred::Appendices(file, node) => {
                    let cx = self.cx(file);
                    let mut scope = Scope {
                        numbered: true,
                        appendix: true,
                    };
                    if let Some(expr) = node.cast::<Expr>() {
                        self.code_expr(&cx, expr, &mut scope);
                    }
                    self.flush();
                }
            }
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

fn apply_set(set: ast::SetRule, scope: &mut Scope) {
    if callee_path(set.target()).last() != Some(&"heading") {
        return;
    }
    for arg in set.args().items() {
        let ast::Arg::Named(n) = arg else { continue };
        match n.name().as_str() {
            "numbering" => scope.numbered = !matches!(n.expr(), Expr::None(_)),
            "supplement" => {
                let text = n.expr().to_untyped().full_text();
                if text.contains("Додаток") {
                    scope.appendix = true;
                }
            }
            _ => {}
        }
    }
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

fn label_arg(call: ast::FuncCall) -> Option<Label> {
    named(call, "label").and_then(|e| match e {
        Expr::None(_) => None,
        e => Some(label_value(e)),
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

/// Кількість колонок таблиці: без аргументу одна, як у typst; `None`, якщо
/// її задано виразом, значення якого з коду не видно.
fn columns(expr: Option<Expr>) -> Option<usize> {
    match expr {
        None => Some(1),
        Some(Expr::Int(i)) => Some(i.get().max(1) as usize),
        Some(Expr::Array(a)) if a.items().all(|i| matches!(i, ast::ArrayItem::Pos(_))) => {
            Some(a.items().count().max(1))
        }
        _ => None,
    }
}

/// Перша `table(…)` у тілі фігури.
fn find_table(expr: Expr) -> Option<ast::FuncCall> {
    match expr {
        Expr::FuncCall(call) => {
            if callee_path(call.callee()).last() == Some(&"table") {
                return Some(call);
            }
            positional(call).into_iter().find_map(find_table)
        }
        Expr::ContentBlock(block) => block.body().exprs().find_map(find_table),
        Expr::CodeBlock(block) => block.body().exprs().find_map(find_table),
        Expr::Parenthesized(p) => find_table(p.expr()),
        _ => None,
    }
}

/// Лістинг: `raw` чи обгортка над ним (`code(read(…))`, `listing`, `sourcecode`).
fn is_listing(expr: Expr) -> bool {
    match expr {
        Expr::Raw(_) => true,
        Expr::FuncCall(call) => callee_name(call).is_some_and(|name| {
            let name = name.to_lowercase();
            ["raw", "code", "listing", "lst", "sourcecode"]
                .iter()
                .any(|n| name.contains(n))
        }),
        Expr::ContentBlock(block) => block.body().exprs().any(is_listing),
        _ => false,
    }
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
            | "title-heading"
            | "title-page"
    )
}

fn is_block_call(call: ast::FuncCall) -> bool {
    callee_name(call).is_some_and(|name| {
        is_block_name(name)
            || matches!(
                name,
                "heading" | "nheading" | "figure" | "fig" | "dstu-figure" | "tab" | "dstu-table"
            )
    })
}

/// Чи є у вмісті виклику абзаци, заголовки чи переліки.
fn has_blocks(call: ast::FuncCall) -> bool {
    content_args(call).into_iter().any(|arg| match arg {
        Expr::ContentBlock(block) => block.body().exprs().any(|e| {
            matches!(
                e,
                Expr::Parbreak(_) | Expr::Heading(_) | Expr::ListItem(_) | Expr::EnumItem(_)
            ) || matches!(e, Expr::Equation(eq) if eq.block())
                || matches!(e, Expr::FuncCall(c) if is_block_call(c))
        }),
        _ => false,
    })
}

/// Шлях без `.` і `..` (файлу може ще не бути, тож без `canonicalize`).
fn normalize(path: &Path) -> PathBuf {
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
