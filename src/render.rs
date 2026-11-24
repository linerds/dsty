//! Зауваження для терміналу у вигляді, як їх друкує typst (codespan):
//!
//! ```text
//! warning[list-punct]: крапка в кінці позиції посеред переліку
//!     ┌─ src/main.typ:571:3
//!     │
//! 570 │ - регулярне оновлення програмного
//! 571 │   забезпечення.
//!     │   ^^^^^^^^^^^^^
//!     │
//!     = п. 7.7.2
//!     = виправлення: Замінити «.» на «;»
//! ```
//!
//! Довгі рядки переносяться по [`WIDTH`] символів, щоб позначка стояла під
//! своїм місцем, а не під перенесеним терміналом рядком.

use std::path::Path;

use crate::lint::{Diagnostic, Severity};

/// Ширина рядка коду в знаках.
pub const WIDTH: usize = 80;
/// Скільки рядків контексту (позиції переліку, формули) показувати найбільше.
const MAX_CONTEXT: usize = 6;
/// Скільки шматків одного довгого рядка показувати навколо позначки.
const MAX_CHUNKS: usize = 3;

/// Позначка в коді: основна (`^`) чи пов'язане місце (`-` з підписом).
struct Label {
    line: usize,
    /// Знаки в рядку: [start, end).
    start: usize,
    end: usize,
    primary: bool,
    note: Option<String>,
}

struct Paint {
    on: bool,
}

impl Paint {
    fn paint(&self, code: &str, s: &str) -> String {
        if self.on && !s.is_empty() {
            format!("\x1b[0m\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_owned()
        }
    }
}

const BLUE: &str = "34";

/// Колір заголовка й позначки рівня.
fn colors(s: Severity) -> (&'static str, &'static str, &'static str) {
    match s {
        Severity::Error => ("error", "1;38;5;9", "31"),
        Severity::Warning => ("warning", "1;38;5;11", "33"),
        Severity::Info => ("info", "1;38;5;14", "36"),
        Severity::Hint => ("hint", "1;38;5;10", "32"),
    }
}

/// Зауваження з кодом. `file` є шляхом для показу, `text` поточним текстом
/// файлу.
pub fn diagnostic(d: &Diagnostic, file: &str, text: &str, color: bool) -> String {
    let p = Paint { on: color };
    let (name, header, tint) = colors(d.severity);
    let lines: Vec<Vec<char>> = text
        .split('\n')
        .map(|l| {
            l.trim_end_matches('\r')
                .chars()
                .map(|c| if c == '\t' { ' ' } else { c })
                .collect()
        })
        .collect();
    let len = |line: usize| lines.get(line).map_or(0, Vec::len);

    // Основна позначка займає один рядок (зауваження обрізано до першого)
    let line = d.start.line;
    let start = d.start.column.min(len(line));
    let end = if d.end.line == line {
        d.end.column.min(len(line))
    } else {
        len(line)
    };
    let mut labels = vec![Label {
        line,
        start,
        end: end.max(start + 1),
        primary: true,
        note: None,
    }];
    let mut notes: Vec<String> = Vec::new();
    for (path, from, to, note) in &d.related {
        if path == &d.path {
            let end = if to.line == from.line {
                to.column
            } else {
                len(from.line)
            };
            labels.push(Label {
                line: from.line,
                start: from.column.min(len(from.line)),
                end: end.min(len(from.line)).max(from.column + 1),
                primary: false,
                note: Some(note.clone()),
            });
        } else {
            notes.push(format!(
                "{note}: {}:{}:{}",
                show_path(path),
                from.line + 1,
                from.column + 1
            ));
        }
    }

    // Рядки: позначки й контекст навколо основної
    let mut shown: Vec<usize> = labels.iter().map(|l| l.line).collect();
    if let Some((from, to)) = d.context {
        let (from, to) = (from.min(line), to.max(line));
        let (from, to) = if to - from < MAX_CONTEXT {
            (from, to)
        } else {
            // Вікно навколо основної позначки
            let lo = line
                .saturating_sub(MAX_CONTEXT / 2)
                .max(from)
                .min(to + 1 - MAX_CONTEXT);
            (lo, lo + MAX_CONTEXT - 1)
        };
        shown.extend(from..=to);
    }
    shown.retain(|&l| l < lines.len());
    shown.sort_unstable();
    shown.dedup();
    // Один пропущений рядок показуємо, а не позначаємо пропуск
    let mut filled = Vec::new();
    for (k, &l) in shown.iter().enumerate() {
        if k > 0 && l == shown[k - 1] + 2 {
            filled.push(l - 1);
        }
        filled.push(l);
    }
    let shown = filled;

    let width = shown.last().map_or(1, |l| (l + 1).to_string().len());
    let pad = " ".repeat(width);
    let bar = p.paint(BLUE, "│");
    let mut out = String::new();
    let rule = if d.rule.is_empty() {
        name.to_owned()
    } else {
        format!("{name}[{}]", d.rule)
    };
    out.push_str(&format!(
        "{}{}\n",
        p.paint(header, &rule),
        p.paint("1", &format!(": {}", d.msg))
    ));
    out.push_str(&format!(
        "{pad} {} {file}:{}:{}\n",
        p.paint(BLUE, "┌─"),
        d.start.line + 1,
        d.start.column + 1
    ));
    out.push_str(&format!("{pad} {bar}\n"));

    let mut prev: Option<usize> = None;
    for &l in &shown {
        if prev.is_some_and(|p| l > p + 1) {
            out.push_str(&format!("{pad} {}\n", p.paint(BLUE, "·")));
        }
        prev = Some(l);
        let here: Vec<&Label> = labels.iter().filter(|x| x.line == l).collect();
        let chars = &lines[l];
        let chunks = chunks(chars);
        // Шматки з позначками й по одному поруч
        let marked: Vec<usize> = (0..chunks.len())
            .filter(|&k| {
                here.iter()
                    .any(|x| x.start < chunks[k].1.max(chunks[k].0 + 1) && x.end > chunks[k].0)
            })
            .collect();
        let keep: Vec<usize> = if chunks.len() <= MAX_CHUNKS || marked.is_empty() {
            (0..chunks.len().min(MAX_CHUNKS.max(1))).collect()
        } else {
            let first = marked[0].saturating_sub(1);
            let last = (marked[marked.len() - 1] + 1).min(chunks.len() - 1);
            (first..=last).collect()
        };
        for (n, &k) in keep.iter().enumerate() {
            let (from, to) = chunks[k];
            let gutter = if n == 0 {
                p.paint(BLUE, &format!("{:>width$}", l + 1))
            } else {
                pad.clone()
            };
            // Продовження перенесеного рядка має відступ самого рядка
            let indent = hanging_indent(chars).min(WIDTH / 4);
            let lead = match (k > 0, n == 0) {
                (false, _) => String::new(),
                (true, true) => format!("{}…", " ".repeat(indent)),
                (true, false) => " ".repeat(indent),
            };
            let tail = if n + 1 == keep.len() && k + 1 < chunks.len() {
                "…"
            } else {
                ""
            };
            // Текст шматка; позначені знаки друкуються кольором позначки
            let style = |col: usize| match here.iter().find(|x| x.start <= col && col < x.end) {
                Some(x) if x.primary => Some(tint),
                Some(_) => Some(BLUE),
                None => None,
            };
            let body = runs(&p, (from..to).map(|col| (chars[col], style(col))));
            out.push_str(&format!("{gutter} {bar} {lead}{body}{tail}\n").replace(" \n", "\n"));
            // Рядок позначок під шматком
            let mut marks = String::new();
            let mut note = None;
            let shift = lead.chars().count();
            for col in from..to.max(from + 1) {
                let mark = here.iter().find(|x| x.start <= col && col < x.end);
                marks.push(match mark {
                    Some(x) if x.primary => '^',
                    Some(_) => '-',
                    None => ' ',
                });
            }
            // Позначка після кінця рядка (вставка в кінці)
            if to == chars.len() && here.iter().any(|x| x.start >= chars.len()) {
                marks.push('^');
            }
            for x in &here {
                if x.end > from && x.end <= to.max(from + 1) && x.note.is_some() {
                    note = x.note.clone();
                }
            }
            let marks = marks.trim_end();
            if !marks.is_empty() {
                let painted = runs(
                    &p,
                    marks.chars().map(|c| {
                        let style = match c {
                            '^' => Some(tint),
                            '-' => Some(BLUE),
                            _ => None,
                        };
                        (c, style)
                    }),
                );
                let note = note
                    .map(|n| format!(" {}", p.paint(BLUE, &n)))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "{pad} {bar} {}{painted}{note}\n",
                    " ".repeat(shift)
                ));
            }
        }
    }

    if !d.clause.is_empty() {
        notes.insert(0, format!("п. {}", d.clause));
    }
    let fix = d.fixes.iter().find(|f| f.is_correction).map(|f| &f.title);
    if let Some(help) = d.help.as_ref().or(fix) {
        notes.push(format!("виправлення: {help}"));
    }
    if let Some(assumed) = d.assumed {
        notes.push(format!("припущення: {assumed}"));
    }
    if !notes.is_empty() {
        out.push_str(&format!("{pad} {bar}\n"));
        for note in notes {
            out.push_str(&format!("{pad} {} {note}\n", p.paint(BLUE, "=")));
        }
    }
    out.push('\n');
    out
}

/// Знаки з кольорами: сусідні однакового кольору йдуть одним фрагментом.
fn runs<'a>(p: &Paint, chars: impl Iterator<Item = (char, Option<&'a str>)>) -> String {
    let mut out = String::new();
    let mut run = String::new();
    let mut current: Option<&str> = None;
    let flush = |run: &mut String, style: Option<&str>, out: &mut String| {
        match style {
            Some(code) => out.push_str(&p.paint(code, run)),
            None => out.push_str(run),
        }
        run.clear();
    };
    for (c, style) in chars {
        if style != current {
            flush(&mut run, current, &mut out);
            current = style;
        }
        run.push(c);
    }
    flush(&mut run, current, &mut out);
    out
}

/// Відступ продовження рядка: пробіли на початку, а в позиції переліку
/// (`- `, `+ `, `1. `) ще й позначка.
fn hanging_indent(line: &[char]) -> usize {
    let lead = line.iter().take_while(|c| **c == ' ').count();
    let rest = &line[lead..];
    let digits = rest.iter().take_while(|c| c.is_ascii_digit()).count();
    let marker = match rest {
        ['-' | '+', ' ', ..] => 2,
        _ if digits > 0 && rest.get(digits) == Some(&'.') && rest.get(digits + 1) == Some(&' ') => {
            digits + 2
        }
        _ => 0,
    };
    lead + marker
}

/// Шматки рядка завширшки до [`WIDTH`] знаків, розірвані по пробілу:
/// (початок, кінець) у знаках.
fn chunks(line: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    while line.len() - start > WIDTH {
        let limit = start + WIDTH;
        // Останній пробіл у другій половині шматка, інакше жорсткий розрив
        let cut = (start + WIDTH / 2..limit)
            .rev()
            .find(|&i| line[i] == ' ')
            .map_or(limit, |i| i + 1);
        out.push((start, cut));
        start = cut;
    }
    out.push((start, line.len()));
    out
}

/// Шлях для показу: відносно поточної теки, якщо файл у ній.
pub fn show_path(path: &Path) -> String {
    let cwd = std::env::current_dir().unwrap_or_default();
    path.strip_prefix(&cwd)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Ширина терміналу в стовпцях, якщо її вдається дізнатися.
pub fn term_width() -> Option<usize> {
    #[cfg(unix)]
    {
        // SAFETY: TIOCGWINSZ лише заповнює передану структуру
        let mut size: libc::winsize = unsafe { std::mem::zeroed() };
        let ok = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) } == 0;
        if ok && size.ws_col > 0 {
            return Some(size.ws_col as usize);
        }
    }
    std::env::var("COLUMNS").ok()?.trim().parse().ok()
}

/// Опис правила для `dsty explain`: заголовок із пунктом стандарту, текст
/// пунктів із рамкою зліва, перенесений по ширині терміналу (не ширше
/// [`WIDTH`]), і коментар, що вимикає правило.
///
/// ```text
/// heading-single — ДСТУ 3008:2015, п. 7.4.6 · статична перевірка
///
///   │ 7.4.6 Якщо розділ або підрозділ складається з одного пункту, або пункт
///   │ складається з одного підпункту, його не нумерують.
///
/// Вимкнути для рядка: // dstu-ignore-next-line: heading-single
/// ```
pub fn explain(rule: &crate::rules::Rule, color: bool, width: Option<usize>) -> String {
    const DIM: &str = "2";
    const BOLD: &str = "1";
    let p = Paint { on: color };
    let width = width.unwrap_or(WIDTH).clamp(40, WIDTH);
    let marks = rule.marks().join(" · ").replace('`', "");

    let source = if rule.clause.is_empty() {
        "службове правило dsty".to_owned()
    } else {
        format!("ДСТУ 3008:2015, п. {}", rule.clause)
    };
    let mut out = format!(
        "{} — {}{}\n\n",
        p.paint(BOLD, rule.code),
        source,
        p.paint(DIM, &format!(" · {marks}"))
    );

    let clauses = crate::standard::clauses_of(rule.clause);
    if clauses.is_empty() {
        // Службове правило: пункту немає, показуємо опис
        for line in wrap(rule.desc, width - 2) {
            out.push_str(&format!("  {line}\n"));
        }
    } else {
        let bar = p.paint(DIM, "│");
        for (i, (id, text)) in clauses.iter().enumerate() {
            if i > 0 {
                out.push_str(&format!("  {bar}\n"));
            }
            for (j, line) in wrap(&format!("{id} {text}"), width - 4).iter().enumerate() {
                let line = match line.strip_prefix(id) {
                    Some(rest) if j == 0 => format!("{}{rest}", p.paint(BOLD, id)),
                    _ => line.clone(),
                };
                out.push_str(&format!("  {bar} {line}\n"));
            }
        }
    }

    out.push_str(&format!(
        "\nВимкнути для рядка: {}\n",
        p.paint(DIM, &format!("// dstu-ignore-next-line: {}", rule.code))
    ));
    out
}

/// Таблиця правил для `dsty rules`: назва, вид перевірки, пункти
/// стандарту й опис. Пункти переносяться по комах у вузькій колонці, опис
/// по словах до ширини `width` (без неї до [`RULES_WIDTH`]).
///
/// ```text
/// ПРАВИЛО          ВИД               ПУНКТ          ОПИС
/// heading-single   static            7.4.6          Єдиний пункт розділу чи…
/// font-size        static+full       7.1.5, 7.9.6   Основний текст і…
/// new-page         static            4.2.1, 4.3.1,  Структурні елементи й
///                                    4.4.1, 4.5.2,  додатки починають з нової
/// ```
pub fn rules_table(rules: &[crate::rules::Rule], color: bool, width: Option<usize>) -> String {
    const GAP: &str = "  ";
    let p = Paint { on: color };
    let header = ["ПРАВИЛО", "ВИД", "ПУНКТ", "ОПИС"];

    // Вид: статична чи повна, вмикається окремо, нетиповий рівень
    let kinds: Vec<Vec<(&str, &str)>> = rules
        .iter()
        .map(|r| {
            let mut kind = vec![match r.kind() {
                "full" => ("full", BLUE),
                "static+full" => ("static+full", BLUE),
                _ => ("static", "2"),
            }];
            if r.optional {
                kind.push(("enable", "33"));
            }
            match r.severity {
                Severity::Warning => {}
                s => kind.push((s.name(), colors(s).2)),
            }
            kind
        })
        .collect();
    let kind_len =
        |k: &[(&str, &str)]| k.iter().map(|(w, _)| w.chars().count() + 1).sum::<usize>() - 1;

    let len = |s: &str| s.chars().count();
    let code_w = rules
        .iter()
        .map(|r| len(r.code))
        .chain([len(header[0])])
        .max()
        .unwrap_or(0);
    let kind_w = kinds
        .iter()
        .map(|k| kind_len(k))
        .chain([len(header[1])])
        .max()
        .unwrap_or(0);
    // Ширина колонки пунктів береться за найдовшим пунктом із комою, але не вужча за два
    // короткі пункти в рядку
    let clause_w = rules
        .iter()
        .flat_map(|r| r.clause.split(',').map(|c| len(c.trim()) + 1))
        .chain([len(header[2]), 13])
        .max()
        .unwrap_or(0);
    let desc_x = code_w + kind_w + clause_w + 3 * GAP.len();
    let desc_w = width.unwrap_or(RULES_WIDTH).saturating_sub(desc_x).max(30);

    let pad = |s: &str, w: usize| " ".repeat(w.saturating_sub(len(s)));
    let mut out = String::new();
    let head: Vec<String> = header
        .iter()
        .zip([code_w, kind_w, clause_w, 0])
        .map(|(h, w)| format!("{}{}", p.paint("1", h), pad(h, w)))
        .collect();
    out.push_str(head.join(GAP).trim_end());
    out.push('\n');

    for (rule, kind) in rules.iter().zip(&kinds) {
        let clauses = if rule.clause.is_empty() {
            vec!["—".to_owned()]
        } else {
            wrap_list(rule.clause, clause_w)
        };
        let desc = wrap(rule.desc, desc_w);
        for i in 0..clauses.len().max(desc.len()) {
            let (code, kind_cell) = if i == 0 {
                let painted: Vec<String> = kind.iter().map(|(w, c)| p.paint(c, w)).collect();
                (
                    rule.code,
                    format!(
                        "{}{}",
                        painted.join(" "),
                        " ".repeat(kind_w - kind_len(kind))
                    ),
                )
            } else {
                ("", " ".repeat(kind_w))
            };
            let clause = clauses.get(i).map_or("", String::as_str);
            let line = format!(
                "{code}{}{GAP}{kind_cell}{GAP}{clause}{}{GAP}{}",
                pad(code, code_w),
                pad(clause, clause_w),
                desc.get(i).map_or("", String::as_str)
            );
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
    out
}

/// Ширина таблиці правил, коли ширина терміналу невідома.
pub const RULES_WIDTH: usize = 120;

/// Перелік через кому, перенесений по елементах на рядки не довші за `width`
/// (кома лишається в кінці рядка).
fn wrap_list(list: &str, width: usize) -> Vec<String> {
    let items: Vec<&str> = list.split(',').map(str::trim).collect();
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for (i, item) in items.iter().enumerate() {
        let item = if i + 1 < items.len() {
            format!("{item},")
        } else {
            (*item).to_owned()
        };
        if !line.is_empty() && line.chars().count() + 1 + item.chars().count() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&item);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Перенесення тексту по словах на рядки не довші за `width` знаків (довше
/// слово лишається цілим на окремому рядку).
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut len = 0;
    for word in text.split_whitespace() {
        let n = word.chars().count();
        if len > 0 && len + 1 + n > width {
            lines.push(std::mem::take(&mut line));
            len = 0;
        }
        if len > 0 {
            line.push(' ');
            len += 1;
        }
        line.push_str(word);
        len += n;
    }
    if len > 0 {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    #[test]
    fn rules_table_columns() {
        let rules = crate::rules::RULES;
        let table = super::rules_table(rules, false, Some(100));
        let lines: Vec<&str> = table.lines().collect();
        assert!(lines[0].starts_with("ПРАВИЛО"), "{table}");
        assert!(lines.iter().all(|l| l.chars().count() <= 100), "{table}");
        assert!(!table.contains("--"), "{table}");
        // Довгий перелік пунктів переноситься по комах у своїй колонці
        let at = lines
            .iter()
            .position(|l| l.starts_with("new-page "))
            .unwrap();
        assert!(
            lines[at].contains(" static ") && lines[at].contains("4.2.1, 4.3.1,"),
            "{table}"
        );
        assert!(lines[at + 1].trim_start().starts_with("4.4.1,"), "{table}");
    }

    #[test]
    fn explain_wraps_clause() {
        let rule = crate::rules::find("heading-single").unwrap();
        let text = super::explain(rule, false, Some(60));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[0],
            "heading-single — ДСТУ 3008:2015, п. 7.4.6 · статична перевірка"
        );
        assert!(lines[2].starts_with("  │ 7.4.6 Якщо розділ"), "{text}");
        assert!(
            lines
                .iter()
                .all(|l| l.chars().count() <= 60 || l.starts_with("heading")),
            "{text}"
        );
        assert!(text.ends_with("Вимкнути для рядка: // dstu-ignore-next-line: heading-single\n"));
    }

    use super::*;
    use crate::lint::{Fix, Pos};
    use std::path::PathBuf;

    fn diag(text: &str, line: usize, start: usize, end: usize) -> Diagnostic {
        let _ = text;
        Diagnostic {
            path: PathBuf::from("/m.typ"),
            range: 0..0,
            start: Pos {
                line,
                column: start,
                utf16: start,
            },
            end: Pos {
                line,
                column: end,
                utf16: end,
            },
            rule: "list-punct",
            clause: "7.7.2",
            msg: "крапка".into(),
            help: None,
            assumed: None,
            severity: Severity::Warning,
            unnecessary: false,
            related: Vec::new(),
            context: None,
            fixes: vec![Fix {
                title: "Замінити «.» на «;»".into(),
                edits: Vec::new(),
                is_correction: true,
            }],
        }
    }

    #[test]
    fn like_typst() {
        let text = "Перелік:\n- перше\n  оновлення.\n- друге.\n";
        let mut d = diag(text, 2, 2, 12);
        d.context = Some((1, 2));
        assert_eq!(
            diagnostic(&d, "m.typ", text, false),
            "warning[list-punct]: крапка\n  ┌─ m.typ:3:3\n  │\n2 │ - перше\n3 │   оновлення.\n  │   ^^^^^^^^^^\n  │\n  = п. 7.7.2\n  = виправлення: Замінити «.» на «;»\n\n"
        );
    }

    #[test]
    fn long_lines_wrap_under_the_mark() {
        let line = format!("{} @code кінець", "слово ".repeat(30));
        let at = line.chars().position(|c| c == '@').unwrap();
        let d = diag(&line, 0, at, at + 5);
        let out = diagnostic(&d, "m.typ", &line, false);
        for row in out.lines().filter(|r| r.contains(" │ ")) {
            assert!(row.chars().count() <= WIDTH + 10, "{row}");
        }
        // Позначка стоїть під `@code`
        let rows: Vec<&str> = out.lines().collect();
        let mark = rows.iter().position(|r| r.contains('^')).unwrap();
        let col = rows[mark].chars().position(|c| c == '^').unwrap();
        let above: Vec<char> = rows[mark - 1].chars().collect();
        assert_eq!(above[col], '@');
    }
}
