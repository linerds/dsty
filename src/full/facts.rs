//! Факти з компіляції: впорядкований потік елементів документа й записів проб,
//! як його повертає вираз `query.typ`.

use serde::Deserialize;

/// Запис потоку: сторінка й позиція (pt) плюс сам елемент.
#[derive(Debug, Clone, Deserialize)]
pub struct Entry {
    pub pg: usize,
    pub x: f64,
    pub y: f64,
    #[serde(flatten)]
    pub item: Item,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "k", rename_all = "kebab-case")]
pub enum Item {
    Heading {
        level: usize,
        num: Option<String>,
        body: String,
    },
    Figure {
        kind: String,
        num: Option<String>,
        label: Option<String>,
        floating: bool,
        caption: Option<String>,
    },
    Equation {
        num: Option<String>,
        label: Option<String>,
    },
    Footnote {
        again: bool,
        #[serde(default)]
        head: Option<String>,
    },
    Ref {
        target: String,
    },
    Cite {
        key: String,
    },
    List {
        #[serde(default)]
        head: String,
    },
    Enum {
        #[serde(default)]
        head: String,
    },
    Raw,
    Image,
    /// Прогін тексту (лише вартий перевірки).
    Run(Run),
    /// Абзац: вид і початок тексту.
    Par(Par),
    Table {
        header: bool,
        repeat: bool,
    },
    TableEnd,
    Page(Page),
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Run {
    /// Вид вмісту за кодом: основний текст (`""`), заголовок (`h`/`H`), підпис
    /// (`c`), фігура (`f`), таблиця (`t`), формула (`e`), надруковане
    /// посилання на джерело (`z`). Заголовки й посилання записуються по
    /// прогонах, з підписів лише слово «Рисунок» і номер, з формул лише номер,
    /// для решти лише збіги ключових слів.
    pub kind: String,
    pub t: String,
    /// Ширина прогону, pt (лише номер формули).
    #[serde(default)]
    pub w: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Par {
    pub kind: String,
    /// Початок тексту: за ним абзац прив'язується до коду.
    pub head: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Page {
    pub w: f64,
    pub h: f64,
    pub top: Option<f64>,
    pub bottom: Option<f64>,
    pub left: Option<f64>,
    pub right: Option<f64>,
}

impl Page {
    /// Береги: явні або типові для typst (2,5/21 меншої сторони).
    pub fn margins(&self) -> (f64, f64, f64, f64) {
        let auto = self.w.min(self.h) * 2.5 / 21.0;
        (
            self.top.unwrap_or(auto),
            self.right.unwrap_or(auto),
            self.bottom.unwrap_or(auto),
            self.left.unwrap_or(auto),
        )
    }

    /// Ліва й права межі тексту.
    pub fn text_area(&self) -> (f64, f64) {
        let (_, right, _, left) = self.margins();
        (left, self.w - right)
    }
}

/// Попередження typst: текст і місце (файл відносно кореня typst, рядок з 1).
#[derive(Debug, Clone)]
pub struct Warning {
    pub message: String,
    pub file: Option<String>,
    pub line: Option<usize>,
}

/// Усе, що дала одна компіляція.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub entries: Vec<Entry>,
    pub warnings: Vec<Warning>,
}
