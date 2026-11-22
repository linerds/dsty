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
        label: Option<String>,
        #[serde(default)]
        counter: Vec<i64>,
    },
    Figure {
        kind: String,
        num: Option<String>,
        label: Option<String>,
        floating: bool,
        caption: Option<String>,
        supplement: String,
    },
    Equation {
        num: Option<String>,
        label: Option<String>,
    },
    Footnote {
        num: Option<String>,
        #[serde(default)]
        counter: Vec<i64>,
        again: bool,
    },
    Ref {
        target: String,
    },
    Cite {
        key: String,
    },
    List {
        marker: String,
        indent: f64,
        #[serde(rename = "indent-em")]
        indent_em: f64,
    },
    Enum {
        num: Option<String>,
        indent: f64,
        #[serde(rename = "indent-em")]
        indent_em: f64,
    },
    /// Прогін тексту (лише вартий перевірки).
    Run(Run),
    /// Абзац: стилі на його початку.
    Par(Par),
    /// Ширина підпису фігури.
    Caption {
        w: f64,
    },
    Table {
        header: bool,
        repeat: bool,
    },
    TableEnd,
    EqStyle {
        align: String,
    },
    Page(Page),
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Run {
    /// Вид вмісту за кодом: основний текст (`""`), заголовок (`h`/`H`), підпис
    /// (`c`), фігура (`f`), таблиця (`t`), зміст (`o`), формула (`e`), перелік
    /// (`l`), слово «де» в основному тексті (`w`, для відступу пояснень під
    /// формулою). Заголовки, зміст, підписи й формули записуються по прогонах,
    /// для решти записуються лише збіги: ключові слова, позначки переліків, курсив.
    pub kind: String,
    pub t: String,
    pub size: f64,
    pub font: String,
    pub style: String,
    pub black: bool,
    #[serde(default)]
    pub weight: Option<serde_json::Value>,
    #[serde(default)]
    pub hyph: Option<bool>,
    #[serde(default)]
    pub w: Option<f64>,
    #[serde(default)]
    pub upper: Option<bool>,
}

impl Run {
    /// Накреслення числом: «semibold» -> 600.
    pub fn weight(&self) -> Option<u32> {
        match self.weight.as_ref()? {
            serde_json::Value::Number(n) => n.as_u64().map(|n| n as u32),
            serde_json::Value::String(s) => Some(match s.as_str() {
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
}

#[derive(Debug, Clone, Deserialize)]
pub struct Par {
    pub kind: String,
    pub size: f64,
    /// Шрифт, накреслення й колір на початку абзацу (для позначених видів
    /// колір завжди `black`).
    #[serde(default)]
    pub font: String,
    #[serde(default)]
    pub style: String,
    #[serde(default)]
    pub black: bool,
    pub indent: f64,
    pub all: bool,
    /// Абзац починається з розриву рядка (`\`): перший рядок порожній.
    #[serde(default, rename = "lead-break")]
    pub lead_break: bool,
    /// Крок рядків (від основи до основи), pt.
    pub pitch: f64,
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
    /// Номер, як його надруковано (`None`, якщо номера немає).
    pub num: Option<String>,
    pub numbered: bool,
    pub align: String,
    /// Колонтитул типовий (`auto`): номер друкує сам typst.
    pub header: bool,
    /// Довжина лінії над виносками: (pt, частка ширини тексту).
    pub sep: Option<(f64, f64)>,
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
