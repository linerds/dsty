//! Текст елемента без розмітки з картою назад у вихідний код.
//!
//! Правила працюють з текстом, який бачить читач (`--` -> «–», `~` -> пробіл,
//! без коментарів і виносок), а зауваження й виправлення мають вказувати на
//! вихідний код. Тому кожен шматок тексту пам'ятає, звідки він узявся.

use std::ops::Range;

/// Місце у вихідному коді: номер файлу в [`crate::model::Code::files`] і байти.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loc {
    pub file: usize,
    pub range: Range<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct Text {
    s: String,
    segs: Vec<Seg>,
}

/// Походження сегмента тексту: правила прози перевіряють лише [`SegKind::Prose`],
/// щоб код, формули й посилання не давали хибних спрацювань.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegKind {
    Prose,
    Code,
    Math,
    Link,
    Ref,
    Mapped,
}

#[derive(Debug, Clone)]
struct Seg {
    /// Байти в `s`.
    at: Range<usize>,
    file: usize,
    /// Байти у файлі.
    src: Range<usize>,
    /// Текст сегмента збігається з вихідним кодом байт у байт.
    exact: bool,
    kind: SegKind,
}

impl Text {
    pub fn as_str(&self) -> &str {
        &self.s
    }

    /// Текст, що збігається з кодом з позиції `src_start` (проза).
    pub fn push_exact(&mut self, s: &str, file: usize, src_start: usize) {
        self.push(
            s,
            file,
            src_start..src_start + s.len(),
            true,
            SegKind::Prose,
        );
    }

    /// Текст прози, що відповідає вузлу коду цілком (`--`, `\#`, пробіл, лапки).
    pub fn push_prose_mapped(&mut self, s: &str, file: usize, src: Range<usize>) {
        self.push(s, file, src, false, SegKind::Prose);
    }

    /// Текст, що відповідає вузлу коду цілком (виклик функції, змінна).
    pub fn push_mapped(&mut self, s: &str, file: usize, src: Range<usize>) {
        self.push(s, file, src, false, SegKind::Mapped);
    }

    /// Сегмент певного семантичного виду (`Code`, `Math`, `Link`, `Ref`).
    pub fn push_kind(
        &mut self,
        s: &str,
        file: usize,
        src: Range<usize>,
        kind: SegKind,
        exact: bool,
    ) {
        self.push(s, file, src, exact, kind);
    }

    fn push(&mut self, s: &str, file: usize, src: Range<usize>, exact: bool, kind: SegKind) {
        if s.is_empty() {
            return;
        }
        let start = self.s.len();
        self.s.push_str(s);
        self.segs.push(Seg {
            at: start..self.s.len(),
            file,
            src,
            exact,
            kind,
        });
    }

    /// Дописує інший текст з його місцями в коді.
    pub fn append(&mut self, other: Text) {
        let shift = self.s.len();
        self.s.push_str(&other.s);
        self.segs.extend(other.segs.into_iter().map(|mut seg| {
            seg.at = seg.at.start + shift..seg.at.end + shift;
            seg
        }));
    }

    /// Байти тексту без пробілів на початку й у кінці.
    pub fn trimmed_range(&self) -> Range<usize> {
        let start = self.s.len() - self.s.trim_start().len();
        let end = self.s.trim_end().len().max(start);
        start..end
    }

    pub fn trimmed(&self) -> &str {
        &self.s[self.trimmed_range()]
    }

    /// Сегмент з байтом `byte` (або останній, якщо `byte` стоїть у кінці тексту).
    /// Сегменти йдуть підряд, тож пошук двійковий.
    fn seg_at(&self, byte: usize) -> Option<&Seg> {
        let i = self.segs.partition_point(|seg| seg.at.end <= byte);
        self.segs
            .get(i)
            .filter(|seg| seg.at.contains(&byte))
            .or_else(|| self.segs.last().filter(|seg| seg.at.end == byte))
    }

    /// Чи байт `byte` тексту стоїть у коді дослівно (а не з `raw`, формули чи
    /// виклику функції).
    pub fn is_exact_at(&self, byte: usize) -> bool {
        self.seg_at(byte)
            .is_some_and(|seg| seg.exact && seg.kind == SegKind::Prose)
    }

    /// Чи кожен сегмент, що перетинає `range`, належить до звичайної прози
    /// (а не до `raw`, формули, посилання чи виклику).
    pub fn is_prose(&self, range: Range<usize>) -> bool {
        if range.is_empty() {
            return self
                .seg_at(range.start)
                .is_some_and(|seg| seg.kind == SegKind::Prose);
        }
        let mut i = self.segs.partition_point(|seg| seg.at.end <= range.start);
        let mut covered = false;
        while let Some(seg) = self.segs.get(i) {
            if seg.at.start >= range.end {
                break;
            }
            if seg.kind != SegKind::Prose {
                return false;
            }
            covered = true;
            i += 1;
        }
        covered
    }

    /// Місце шматка тексту в коді. Якщо шматок вийшов з одного точного
    /// сегмента, це самі ці байти, інакше від початку першого вузла до кінця
    /// останнього.
    pub fn loc(&self, range: Range<usize>) -> Option<Loc> {
        if range.is_empty() {
            return self.after(range.start);
        }
        let first = self.seg_at(range.start)?;
        let last = self.seg_at(range.end - 1)?;
        let start = if first.exact {
            first.src.start + (range.start - first.at.start)
        } else {
            first.src.start
        };
        let end = if last.exact {
            last.src.start + (range.end - last.at.start)
        } else {
            last.src.end
        };
        if first.file != last.file || end < start {
            return Some(Loc {
                file: first.file,
                range: first.src.clone(),
            });
        }
        Some(Loc {
            file: first.file,
            range: start..end,
        })
    }

    /// Місце шматка, лише якщо його байти в коді ті самі, що в тексті: тоді
    /// їх можна безпечно замінити виправленням.
    pub fn exact_loc(&self, range: Range<usize>) -> Option<Loc> {
        let seg = self.seg_at(range.start)?;
        if !seg.exact || range.end > seg.at.end {
            return None;
        }
        let start = seg.src.start + (range.start - seg.at.start);
        Some(Loc {
            file: seg.file,
            range: start..start + range.len(),
        })
    }

    /// Місце в коді одразу після байта `byte - 1` тексту: туди вставляють
    /// пропущений розділовий знак.
    pub fn after(&self, byte: usize) -> Option<Loc> {
        let i = self.segs.partition_point(|seg| seg.at.end < byte);
        let seg = self.segs.get(i).filter(|seg| byte > seg.at.start)?;
        let at = if seg.exact {
            seg.src.start + (byte - seg.at.start)
        } else {
            seg.src.end
        };
        Some(Loc {
            file: seg.file,
            range: at..at,
        })
    }

    /// Місце всього тексту (без крайніх пробілів).
    pub fn whole(&self) -> Option<Loc> {
        let range = self.trimmed_range();
        if range.is_empty() {
            return None;
        }
        self.loc(range)
    }
}

/// Регістр першого слова.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Case {
    Upper,
    Lower,
}

/// Регістр першої літери: `None`, якщо рядок починається не з літери чи з
/// абревіатури (ХНУРЕ, HTTP).
pub fn first_case(s: &str) -> Option<Case> {
    let word = s.split_whitespace().next()?;
    let mut chars = word.chars();
    let first = chars.next()?;
    if !first.is_alphabetic() {
        return None;
    }
    if first.is_lowercase() || !first.is_uppercase() {
        return Some(Case::Lower);
    }
    let rest: String = chars.collect();
    if !rest.is_empty() && rest == rest.to_uppercase() {
        None
    } else {
        Some(Case::Upper)
    }
}

/// Крапка в кінці, але не три крапки.
pub fn ends_with_period(s: &str) -> bool {
    s.ends_with('.') && !s.ends_with("...")
}

/// Скорочує довгий текст для повідомлення: «початок…».
pub fn quote(s: &str) -> String {
    let s = s.trim();
    let chars: Vec<char> = s.chars().collect();
    if chars.len() > 60 {
        format!("«{}…»", chars[..60].iter().collect::<String>())
    } else {
        format!("«{s}»")
    }
}

/// Кінець довгого тексту для повідомлення: «…кінець».
pub fn tail(s: &str) -> String {
    let s = s.trim();
    let chars: Vec<char> = s.chars().collect();
    if chars.len() > 40 {
        format!(
            "«…{}»",
            chars[chars.len() - 40..].iter().collect::<String>()
        )
    } else {
        format!("«{s}»")
    }
}

/// Довжина в пунктах як міліметри: «25 мм», «12,5 мм».
pub fn mm(pt: f64) -> String {
    let v = pt / crate::eval::MM;
    if (v - v.round()).abs() < 0.05 {
        format!("{} мм", v.round())
    } else {
        format!("{v:.1} мм").replace('.', ",")
    }
}

/// Число з комою, до двох знаків: «1,5», «14».
pub fn fmt_num(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0')
        .trim_end_matches('.')
        .replace('.', ",")
}

/// Кегль чи довжина в пунктах: «12,5 pt».
pub fn fmt_pt(v: f64) -> String {
    fmt_num(v) + " pt"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cases() {
        assert_eq!(first_case("мала літера"), Some(Case::Lower));
        assert_eq!(first_case("Велика"), Some(Case::Upper));
        assert_eq!(first_case("HTTP запити"), None);
        assert_eq!(first_case("А"), Some(Case::Upper));
        assert_eq!(first_case("1 рядок"), None);
        assert_eq!(first_case(""), None);
    }

    #[test]
    fn maps_back() {
        // Код: `= Розділ -- кінець.`
        let code = "= Розділ -- кінець.";
        let dash = code.find("--").unwrap();
        let mut t = Text::default();
        t.push_exact("Розділ ", 0, 2);
        t.push_mapped("–", 0, dash..dash + 2);
        t.push_exact(" кінець.", 0, dash + 2);
        let end = t.as_str().len();
        // Крапка в кінці є точним сегментом
        assert_eq!(
            t.exact_loc(end - 1..end).unwrap().range,
            code.len() - 1..code.len()
        );
        assert_eq!(t.after(end).unwrap().range, code.len()..code.len());
        // Тире відповідає вузлу цілком, без точного виправлення
        let at = t.as_str().find('–').unwrap();
        assert!(t.exact_loc(at..at + '–'.len_utf8()).is_none());
        assert_eq!(
            t.after(at + '–'.len_utf8()).unwrap().range,
            dash + 2..dash + 2
        );
        assert_eq!(t.loc(0..end).unwrap().range, 2..code.len());
    }
}
