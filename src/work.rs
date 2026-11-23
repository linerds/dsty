//! Нейтральна модель роботи: факти рівня ДСТУ без типів typst.
//!
//! Шари:
//! 1. читання коду ([`crate::eval`], обхід у [`crate::model`]);
//! 2. рушій typst: налаштування ([`crate::style`]) і оформлення елементів
//!    ([`crate::look`]) одразу як факти цієї моделі;
//! 3. ця модель;
//! 4. правила ДСТУ над нею: статичні ([`crate::rules::run`]) і правила
//!    розкладки ([`crate::rules::run_layout`]).
//!
//! Обхід коду ([`crate::model::build`]) дає цю модель і вихідний код
//! ([`crate::model::Code`]): текст для виправлень і `set`-правила, де
//! варто задати типове значення.
//!
//! Кожне значення — [`Fact`]: відоме з певною підставою ([`Basis`]) і місцем
//! ([`At`]) або невідоме. Правило саме вирішує, яким підставам вірити.
//! Повна перевірка ([`crate::full::lay_out`]) кладе в ту саму модель те, що
//! видно лише на сторінках (факти з підставою [`Basis::Layout`]), а
//! надруковане без відповідника в коді — новими елементами.

use std::collections::HashMap;
use std::rc::Rc;

use crate::style::Origin;
use crate::text::{Loc, Text};

// ---------------------------------------------------------------- спільні з моделлю коду

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

/// Мітка елемента: `<назва>`, `label: <назва>`, `#label("назва")`; мітку з
/// виразу (`#label(str(...))` у циклі) з коду не дізнатися.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Label {
    Name(String),
    Dynamic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FigureKind {
    Image,
    Table,
}

#[derive(Debug)]
pub struct Header {
    pub cells: Vec<Text>,
    /// Головка в один рядок: клітинок не більше, ніж колонок.
    pub single_row: bool,
}

impl Header {
    /// Кількість колонок `columns`, якщо її видно з коду.
    pub(crate) fn new(cells: Vec<Text>, columns: Option<usize>) -> Self {
        Header {
            single_row: columns.is_some_and(|c| cells.len() <= c),
            cells,
        }
    }
}

#[derive(Debug)]
pub struct Item {
    pub text: Text,
    pub loc: Loc,
    /// За текстом позиції йде вкладений перелік.
    pub has_nested: bool,
}

// ---------------------------------------------------------------- факти

/// Значення з підставою.
#[derive(Debug, Clone, Default)]
pub enum Fact<T> {
    /// `Why` спільний: багато фактів мають одне джерело (одне `set`-правило).
    Known(T, Rc<Why>),
    #[default]
    Unknown,
}

/// Чому значення таке і де його виправляти.
#[derive(Debug, Clone)]
pub struct Why {
    pub basis: Basis,
    pub at: At,
}

/// Підстава значення, від найпевнішої.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Basis {
    /// Записано в коді документа чи шаблону.
    Code,
    /// Типове значення typst.
    Default,
    /// Виведено рушієм: лічильники, нумерація, show-функції.
    Computed,
    /// Евристика: титульний аркуш, «Примітка…», «де…».
    Inferred,
    /// З компіляції (розташування на сторінках): лише в повній перевірці.
    Layout,
}

/// Де значення задано: туди вказує зауваження.
#[derive(Debug, Clone)]
pub enum At {
    /// `set`- чи show-правило (у шаблоні — рядок, що його застосовує).
    Rule(Origin),
    /// Типове значення: зауваження стоїть на налаштуванні `target` з
    /// аргументами `args` (за `within` — усередині show-правила для цього
    /// елемента), а якщо його немає, на початку документа.
    Default {
        target: &'static str,
        args: &'static [&'static str],
        within: Option<&'static str>,
    },
    /// Сам елемент.
    Element(Loc),
}

impl At {
    pub fn default(target: &'static str, args: &'static [&'static str]) -> Self {
        At::Default {
            target,
            args,
            within: None,
        }
    }

    pub fn default_in(
        within: &'static str,
        target: &'static str,
        args: &'static [&'static str],
    ) -> Self {
        At::Default {
            target,
            args,
            within: Some(within),
        }
    }
}

impl<T> Fact<T> {
    pub fn new(value: T, basis: Basis, at: At) -> Self {
        Fact::Known(value, Rc::new(Why { basis, at }))
    }

    /// Значення з будь-якою підставою.
    pub fn known(&self) -> Option<&T> {
        match self {
            Fact::Known(v, _) => Some(v),
            Fact::Unknown => None,
        }
    }

    /// Значення, якому можна вірити без застережень (не евристика).
    pub fn sure(&self) -> Option<&T> {
        match self {
            Fact::Known(v, w) if w.basis != Basis::Inferred => Some(v),
            _ => None,
        }
    }

    pub fn why(&self) -> Option<&Why> {
        match self {
            Fact::Known(_, w) => Some(w),
            Fact::Unknown => None,
        }
    }

    pub fn at(&self) -> Option<&At> {
        self.why().map(|w| &w.at)
    }

    /// Задано правилом коду.
    pub fn set(value: T, origin: Origin) -> Self {
        Fact::new(value, Basis::Code, At::Rule(origin))
    }

    /// Типове значення typst; зауваження про нього стоїть на `at`.
    pub fn typst(value: T, at: At) -> Self {
        Fact::new(value, Basis::Default, at)
    }

    /// Правило, що задає значення.
    pub fn origin(&self) -> Option<&Origin> {
        match self.at() {
            Some(At::Rule(o)) => Some(o),
            _ => None,
        }
    }

    /// Обчислене рушієм значення, що стоїть на самому елементі.
    pub fn computed(value: Option<T>, loc: &Loc) -> Self {
        match value {
            Some(v) => Fact::new(v, Basis::Computed, At::Element(loc.clone())),
            None => Fact::Unknown,
        }
    }

    /// Те саме значення, але типове переноситься на `at` (місце залежить від
    /// того, де значення діє); задане правилом лишається на правилі.
    pub fn placed(&self, at: impl FnOnce() -> At) -> Self
    where
        T: Clone,
    {
        self.derive(self.known().cloned(), at)
    }

    /// Похідне значення `value` з тією самою підставою; типове — на `at`.
    pub fn derive<U>(&self, value: Option<U>, at: impl FnOnce() -> At) -> Fact<U> {
        match (self, value) {
            (Fact::Known(_, w), Some(v)) if matches!(w.at, At::Rule(_)) => {
                Fact::Known(v, w.clone())
            }
            (Fact::Known(_, w), Some(v)) => Fact::new(v, w.basis, at()),
            _ => Fact::Unknown,
        }
    }

    /// Те саме значення як типове typst (вбудоване оформлення елемента),
    /// зауваження про яке стоїть на `at`.
    pub fn as_typst(&self, at: At) -> Self
    where
        T: Clone,
    {
        match self {
            Fact::Known(v, _) => Fact::typst(v.clone(), at),
            Fact::Unknown => Fact::Unknown,
        }
    }
}

impl<T: PartialEq> Fact<T> {
    pub fn is(&self, v: &T) -> bool {
        self.known() == Some(v)
    }
}

// ---------------------------------------------------------------- оформлення

/// Як виглядає текст. Довжини в пунктах; `em` і `auto` уже розв'язано.
#[derive(Debug, Clone)]
pub struct TextLook {
    /// Кегль, пт.
    pub size: Fact<f64>,
    /// Перший шрифт переліку, як записано в коді.
    pub font: Fact<String>,
    /// 400 звичайний, 700 напівжирний.
    pub weight: Fact<i64>,
    pub italic: Fact<bool>,
    pub black: Fact<bool>,
    /// Великі літери (`upper`).
    pub upper: Fact<bool>,
    /// `leading` в em, якщо з нього виводиться крок рядків. Інтервал з нього
    /// рахують правила: висота великої літери різна в різних шрифтах.
    pub leading: Fact<f64>,
    /// Переноси слів (`auto` з вирівнюванням уже розв'язано).
    pub hyphenate: Fact<bool>,
}

/// Розмір сторінки, пт (`None`: без межі).
#[derive(Debug, Clone)]
pub struct Paper {
    pub name: Option<String>,
    pub w: Option<f64>,
    pub h: Option<f64>,
}

/// Оформлення абзацу й сторінки в точці документа.
#[derive(Debug, Clone)]
pub struct Look {
    pub text: TextLook,
    /// Абзацний відступ, пт.
    pub indent: Fact<f64>,
    /// Відступ і в першому абзаці після заголовка, рисунка, переліку.
    pub indent_all: bool,
    pub paper: Fact<Paper>,
    /// Береги, пт: верхній, правий, нижній, лівий (`auto` розв'язано).
    pub margins: [Fact<f64>; 4],
}

/// Вирівнювання; `Right` охоплює й `end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

// ---------------------------------------------------------------- робота

/// Уся робота чи фрагмент.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    /// Має власні налаштування чи шаблон, або перевірка `--full`: типові
    /// значення typst до неї стосуються.
    Whole,
    /// Окремий файл без налаштувань: про типові значення мовчимо.
    #[default]
    Fragment,
}

#[derive(Debug, Default)]
pub struct Work {
    pub scope: Scope,
    /// Посилання за мітками: місця на одній шкалі з [`Figure::tick`].
    pub refs: HashMap<String, Vec<usize>>,
    /// Посилання в тексті за порядком.
    pub ref_uses: Vec<RefUse>,
    /// Як друкуються посилання (`@мітка`).
    pub ref_style: RefStyle,
    /// `bibliography(…)` і `set bibliography(…)`.
    pub bibs: Vec<Bib>,
    /// У документі є перелік джерел (`bibliography` чи шаблон).
    pub has_bibliography: bool,
    /// Елементи за порядком.
    pub elements: Vec<Element>,
    /// Оформлення абзаців, на які посилаються елементи.
    pub looks: Vec<Look>,
    /// Текст з власним оформленням у рядку (`#text(…)[…]`, `_…_`).
    pub runs: Vec<Run>,
    pub footnotes: Footnotes,
    pub pages: Option<PageNumbers>,
    /// `counter(page).update(…)` у документі.
    pub page_updates: Vec<Loc>,
    /// Оформлення тексту змісту, якщо зміст є і його видно.
    pub contents: Option<TextLook>,
    /// Скільки сторінок (повна перевірка).
    pub page_count: Fact<usize>,
    /// Рядки, де typst не знайшов шрифту Times New Roman.
    pub missing_fonts: Fact<Vec<Loc>>,
    pub counts: Fact<Counts>,
    /// Надруковані групи посилань на джерела до додатків: «[1, 2]».
    pub citations: Fact<Vec<(Loc, String)>>,
}

/// Як друкуються посилання.
#[derive(Debug, Default)]
pub struct RefStyle {
    /// Власне show-правило `ref` чи `set ref(supplement: …)`: з коду не видно.
    pub custom: bool,
    /// Підпис, заданий документом, з малої літери (`set figure(supplement:
    /// [рис.])`): регістр посилань на ці елементи не перевіряється.
    pub lower_figure: bool,
    pub lower_equation: bool,
    pub lower_heading: bool,
}

impl Work {
    /// Індекс першого заголовка: до нього титульний аркуш.
    pub fn first_heading(&self) -> Option<usize> {
        self.elements
            .iter()
            .position(|e| matches!(e, Element::Heading(_)))
    }
}

#[derive(Debug)]
pub enum Element {
    Heading(Heading),
    Figure(Figure),
    Equation(Equation),
    List(List),
    Par(Par),
    /// Ключові слова з аргументу шаблону (`abstract(keywords: …)`); шаблон
    /// сам сортує їх за абеткою.
    Keywords {
        count: usize,
        loc: Loc,
    },
    /// Ліва колонка переліку скорочень: терміни `/ ЕОМ: …` чи перша колонка
    /// таблиці.
    Terms(Vec<Text>),
    /// Зміна лічильника: нічого не друкує.
    Counter,
    /// Таблиця поза фігурою (лише з компіляції).
    Table(Table),
}

// ---------------------------------------------------------------- елементи

#[derive(Debug)]
pub struct Heading {
    pub level: usize,
    pub title: Text,
    pub loc: Loc,
    pub label: Option<Label>,
    pub numbered: bool,
    /// Текст друкує шаблон («Реферат»): у коді його немає, тож без виправлень.
    pub generated: bool,
    /// Заголовок розмітки (`= Назва`): місце вузла й код назви без мітки
    /// для виправлення на `#heading(…)[…]`.
    pub markup: Option<(Loc, String)>,
    /// Заголовок у додатках.
    pub appendix: bool,
    /// Починається з нової сторінки.
    pub new_page: Fact<bool>,
    /// Абзацний відступ там, де стоїть заголовок, пт.
    pub par_indent: Fact<f64>,
    /// Як надруковано; `None` для тексту, який друкує шаблон.
    pub look: Option<Box<HeadingLook>>,
    /// Як надрукувала компіляція.
    pub printed: Fact<Box<HeadingPrint>>,
}

#[derive(Debug)]
pub struct HeadingLook {
    /// Оформлення тексту, якщо його видно.
    pub text: Option<TextLook>,
    /// Надрукований номер; `None` усередині: без номера.
    pub number: Fact<Option<String>>,
    /// Текст перед назвою («1.2», «ДОДАТОК А»).
    pub prefix: Option<String>,
    pub align: Fact<Align>,
    /// Відступ першого рядка від лівого берега, пт.
    pub offset: Fact<f64>,
}

#[derive(Debug)]
pub struct Figure {
    pub kind: FigureKind,
    pub loc: Loc,
    pub label: Option<Label>,
    /// Місце на одній шкалі з посиланнями ([`Work::refs`]).
    pub tick: usize,
    /// Плаваюча (`placement`): її місце в тексті з коду не видно.
    pub floating: bool,
    /// Головка таблиці.
    pub header: Option<Header>,
    pub caption: Option<Text>,
    pub appendix: bool,
    pub look: Rc<CaptionLook>,
    /// Номер розділу (перше число лічильника заголовків).
    pub chapter: Option<i64>,
    /// Надрукований номер.
    pub number: Fact<String>,
    /// Сторінка й назва, як надрукувала компіляція.
    pub printed: Fact<FigurePrint>,
    /// Таблиця переходить на іншу сторінку (`None`: не переходить).
    pub split: Fact<Option<TableSplit>>,
}

#[derive(Debug, Default)]
pub struct CaptionLook {
    /// Слово перед номером, як надруковано («Рисунок»).
    pub supplement: Fact<String>,
    pub separator: Fact<String>,
    /// Назва над елементом.
    pub top: Fact<bool>,
    /// Номер першої такої фігури, як надруковано; `None` усередині:
    /// фігури без номера.
    pub numbering: Fact<Option<String>>,
}

#[derive(Debug)]
pub struct Equation {
    pub loc: Loc,
    pub label: Option<Label>,
    /// Одразу за іншою формулою: між ними лише проміжки й невидимий код.
    pub follows: bool,
    /// На формулу посилаються в тексті (мітку з виразу вважаємо посиланою).
    pub referenced: bool,
    pub appendix: bool,
    pub look: Rc<EquationLook>,
    pub chapter: Option<i64>,
    /// Надрукований номер («(1.2)»).
    pub number: Fact<String>,
    /// На скільки надрукований номер не доходить до правого берега, пт.
    pub number_gap: Fact<f64>,
}

#[derive(Debug, Default)]
pub struct EquationLook {
    /// Номер першої формули, як надруковано; `None` усередині: без номера.
    pub numbering: Fact<Option<String>>,
    /// Розміщення номера, як записано (`right`, `start + horizon`).
    pub number_align: Fact<String>,
    /// Вирівнювання формули, як записано; відоме, лише коли формулу не
    /// змінюють show-правила.
    pub align: Option<String>,
}

#[derive(Debug)]
pub struct List {
    pub items: Vec<Item>,
    /// Абзац безпосередньо перед переліком (індекс у [`Work::elements`]).
    pub prev_par: Option<usize>,
    pub nested: bool,
    pub enumerated: bool,
    pub section: Section,
    /// Позначка першої позиції, як надруковано («–», «а)»).
    pub marker: Fact<String>,
    /// Відступ позицій від берега, пт.
    pub indent: Fact<f64>,
    /// Оформлення там, де стоїть перелік ([`Work::looks`]).
    pub look: usize,
}

#[derive(Debug)]
pub struct Par {
    pub text: Text,
    pub section: Section,
    pub after_equation: bool,
    /// Оформлення абзацу ([`Work::looks`]).
    pub look: usize,
    /// Перший рядок починається не з тексту.
    pub lead: Option<Lead>,
    /// Абзац починається з `#text(…)[…]`: оформлення цього тексту
    /// ([`Work::looks`]).
    pub first_look: Option<usize>,
}

#[derive(Debug, Clone)]
pub enum Lead {
    /// `\`: перший рядок порожній.
    Break,
    /// `#h(…)`, пт (`None`, якщо довжини не видно).
    Space(Option<f64>),
}

#[derive(Debug)]
pub struct Run {
    pub loc: Loc,
    pub text: String,
    /// Оформлення тексту й абзацу навколо ([`Work::looks`]).
    pub look: usize,
    pub par: usize,
    /// Курсив розмітки (`_…_`).
    pub emph: bool,
    /// Скільки елементів було перед ним.
    pub element: usize,
}

// ---------------------------------------------------------------- виноски, сторінки

#[derive(Debug, Default)]
pub struct Footnotes {
    /// Місця виносок у коді за порядком.
    pub at: Vec<Loc>,
    /// Оформлення, якщо виноски є.
    pub look: Option<FootnoteLook>,
    /// Сторінки з виносками: (сторінка, скільки виносок, місце першої).
    pub pages: Fact<Vec<(usize, usize, Loc)>>,
}

#[derive(Debug)]
pub struct FootnoteLook {
    /// Позначка першої виноски («1», «1)»).
    pub marker: Fact<String>,
    /// Довжина лінії над виносками, пт (0: без лінії).
    pub separator: Fact<f64>,
    pub text: Option<TextLook>,
}

#[derive(Debug)]
pub struct PageNumbers {
    /// Номер першої нумерованої сторінки основного тексту; `None`: без номерів.
    pub body: Fact<Option<String>>,
    /// Котра це сторінка від початку.
    pub first_page: i64,
    /// Номер на титульному аркуші, якщо аркуш є.
    pub title: Option<Fact<Option<String>>>,
    /// Де друкується номер, як записано (`right+top`).
    pub align: Fact<String>,
    /// Лічильник виносок скидається на кожній сторінці.
    pub footnote_reset: Option<bool>,
}

// ---------------------------------------------------------------- розкладка

/// Як typst надрукував заголовок (повна перевірка).
#[derive(Debug)]
pub struct HeadingPrint {
    pub level: usize,
    /// Текст, як надруковано.
    pub body: String,
    /// Нумерація ввімкнена.
    pub numbering: bool,
    /// Надрукований номер перед назвою.
    pub number: Option<String>,
    /// Сторінка, де закінчується заголовок.
    pub last_page: usize,
    /// Наступний вміст: сторінка й чи це заголовок розділу.
    pub next: Option<(usize, bool)>,
    /// Скільки сторінок до наступного заголовка першого рівня.
    pub span: Option<usize>,
    /// Надрукований текст розділу (лише для реферату: відомості про обсяг).
    pub text: Option<String>,
}

/// Як typst надрукував рисунок чи таблицю.
#[derive(Debug)]
pub struct FigurePrint {
    /// Назва, як надруковано.
    pub caption: Option<String>,
    pub page: usize,
    /// Сторінка першого посилання, якщо воно раніше за неплаваючу фігуру поза
    /// додатками.
    pub ref_page: Option<usize>,
}

/// Таблиця, що переходить на іншу сторінку.
#[derive(Debug)]
pub struct TableSplit {
    /// Остання сторінка таблиці.
    pub to: usize,
    pub header: bool,
    pub repeat: bool,
    /// Сторінки перенесених частин без «Продовження таблиці».
    pub unmarked: Vec<usize>,
}

/// Перенесена таблиця поза рисунком чи таблицею моделі: її знає лише
/// компіляція.
#[derive(Debug)]
pub struct Table {
    pub loc: Loc,
    /// Назва фігури, у якій вона стоїть, як надруковано.
    pub caption: Option<String>,
    pub split: Fact<TableSplit>,
}

/// Скільки в роботі того, що перелічують у рефераті.
#[derive(Debug, Default)]
pub struct Counts {
    /// Пронумеровані рисунки й таблиці.
    pub images: usize,
    pub tables: usize,
    /// Джерела, на які є посилання.
    pub sources: usize,
    pub appendices: usize,
    /// Нумеровані розділи (не структурні елементи й не додатки).
    pub chapters: usize,
}
