//! Перевірка документа: модель -> правила -> зауваження з позиціями й
//! виправленнями, з урахуванням коментарів `// dstu-ignore`.
//!
//! Коментарі:
//! - `// dstu-ignore: правило, …` -- у тому ж рядку, що й зауваження;
//! - `// dstu-ignore-next-line: правило` -- для наступного рядка;
//! - `// dstu-ignore-file: правило` -- для всього файлу.
//!
//! Без переліку правил коментар вимикає всі.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use typst_syntax::{Source, SyntaxKind, SyntaxNode};

use crate::full::Facts;
use crate::model::{self, Doc};
pub use crate::rules::Severity;
use crate::rules::{self, FixDraft, Found, RULES, Rule};
use crate::text::Loc;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Options {
    /// Правила, які не перевіряти.
    pub ignore: Vec<String>,
    /// Додаткові правила (`optional`), які ввімкнути.
    pub enable: Vec<String>,
    /// Рівень зауважень правил (типово -- [`Rule::severity`]).
    pub severity: HashMap<String, Severity>,
}

impl Options {
    pub fn severity_of(&self, rule: &Rule) -> Severity {
        self.severity
            .get(rule.code)
            .copied()
            .unwrap_or(rule.severity)
    }
}

/// Позиція у файлі: рядок (з 0), колонка в символах і в UTF-16 (з 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pos {
    pub line: usize,
    pub column: usize,
    pub utf16: usize,
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub path: PathBuf,
    pub range: Range<usize>,
    pub start: Pos,
    pub end: Pos,
    pub rule: &'static str,
    pub clause: &'static str,
    pub msg: String,
    pub severity: Severity,
    /// Зайвий код (коментар, що нічого не вимикає): редактор його притінює.
    pub unnecessary: bool,
    /// Пов'язані місця: (файл, початок, кінець, пояснення).
    pub related: Vec<(PathBuf, Pos, Pos, String)>,
    /// Рядки (з 0, включно) елемента навколо зауваження, які варто показати.
    pub context: Option<(usize, usize)>,
    /// Виправлення: спершу суть, останнім -- вимкнути правило для рядка.
    pub fixes: Vec<Fix>,
}

impl Diagnostic {
    /// Текст повідомлення для редактора й консолі.
    pub fn message(&self) -> String {
        if self.clause.is_empty() {
            self.msg.clone()
        } else {
            format!("{} (п. {})", self.msg, self.clause)
        }
    }
}

#[derive(Debug, Clone)]
pub struct Fix {
    pub title: String,
    /// Заміни у файлі зауваження.
    pub edits: Vec<Edit>,
    /// Виправлення суті, а не вимкнення правила: таке застосовує `--fix`.
    pub is_correction: bool,
}

#[derive(Debug, Clone)]
pub struct Edit {
    pub range: Range<usize>,
    pub start: Pos,
    pub end: Pos,
    /// Текст, що зараз стоїть на цьому місці: за ним перевіряють, що файл не
    /// змінився з часу перевірки.
    pub old: String,
    pub new: String,
}

pub struct Report {
    pub diagnostics: Vec<Diagnostic>,
    /// Усі прочитані файли документа.
    pub files: Vec<PathBuf>,
    /// `#include` файлів, яких немає: (файл, позиція, шлях).
    pub missing: Vec<(PathBuf, Pos, PathBuf)>,
}

/// Перевіряє правила й повертає помилку для невідомих назв.
pub fn validate(options: &Options) -> anyhow::Result<()> {
    for rule in options
        .ignore
        .iter()
        .chain(&options.enable)
        .chain(options.severity.keys())
    {
        if rules::find(rule).is_none() {
            let known: Vec<_> = RULES.iter().map(|r| r.code).collect();
            anyhow::bail!("невідоме правило «{rule}», відомі: {}", known.join(", "));
        }
    }
    Ok(())
}

/// Чи перевіряти правило: не вимкнене, а необов'язкове -- увімкнене.
pub fn is_active(options: &Options, code: &str) -> bool {
    !options.ignore.iter().any(|r| r == code)
        && (!rules::find(code).is_some_and(|r| r.optional)
            || options.enable.iter().any(|r| r == code))
}

/// Перевіряє документ від `main`. `load` читає файл: відкритий буфер чи диск.
pub fn check(
    root: &Path,
    main: &Path,
    load: &dyn Fn(&Path) -> Option<String>,
    options: &Options,
) -> anyhow::Result<Report> {
    check_full(root, main, load, options, None)
}

/// Перевірка зі статичними правилами й, якщо є факти компіляції
/// ([`crate::full`]), з правилами повної перевірки.
pub fn check_full(
    root: &Path,
    main: &Path,
    load: &dyn Fn(&Path) -> Option<String>,
    options: &Options,
    facts: Option<&Facts>,
) -> anyhow::Result<Report> {
    validate(options)?;
    let Some(doc) = model::build(root, main, load) else {
        anyhow::bail!("не вдалося прочитати {}", main.display());
    };
    let active = |code: &str| is_active(options, code);
    let mut found = rules::run(&doc, &active);
    if let Some(facts) = facts {
        // Повна перевірка не повторює статичне зауваження того самого правила в тому самому рядку
        let line = |f: &Found| {
            let source = &doc.files[f.loc.file].source;
            (
                f.loc.file,
                position(source, f.loc.range.start).line,
                f.rule.code,
            )
        };
        let static_lines: std::collections::HashSet<_> = found.iter().map(line).collect();
        found.extend(
            crate::full::check(&doc, facts, &active)
                .into_iter()
                .filter(|f| !static_lines.contains(&line(f))),
        );
    }

    let mut suppressions: Vec<Suppressions> = doc
        .files
        .iter()
        .map(|f| Suppressions::scan(&f.source))
        .collect();
    let mut found: Vec<Found> = found
        .into_iter()
        .filter(|f| {
            let source = &doc.files[f.loc.file].source;
            let line = position(source, f.loc.range.start).line;
            !suppressions[f.loc.file].covers(f.rule.code, line)
        })
        .collect();
    // Самі коментари: невідомі правила й коментарі, що нічого не вимикають
    for (file, s) in suppressions.iter().enumerate() {
        found.extend(s.report(file, &doc.files[file].source, facts.is_some(), &active));
    }
    let mut diagnostics: Vec<Diagnostic> = merge_groups(found)
        .into_iter()
        .map(|f| diagnostic(&doc, options, f))
        .collect();
    diagnostics.sort_by(|a, b| {
        (&a.path, a.start.line, a.start.column).cmp(&(&b.path, b.start.line, b.start.column))
    });
    diagnostics.dedup_by(|a, b| {
        a.path == b.path && a.range == b.range && a.rule == b.rule && a.msg == b.msg
    });

    let missing = doc
        .missing
        .iter()
        .map(|(loc, path)| {
            let file = &doc.files[loc.file];
            (
                file.path.clone(),
                position(&file.source, loc.range.start),
                path.clone(),
            )
        })
        .collect();
    Ok(Report {
        diagnostics,
        files: doc.files.iter().map(|f| f.path.clone()).collect(),
        missing,
    })
}

/// Скільки однакових зауважень в одному переліку зводяться в одне.
const GROUP_MIN: usize = 3;

/// Зводить однакові зауваження групи (перелік, у якому кілька позицій мають
/// ту саму ваду) в одне на першій позиції з одним виправленням на всі.
fn merge_groups(found: Vec<Found>) -> Vec<Found> {
    let key = |f: &Found| f.group.as_ref().map(|g| (g.id, f.rule.code));
    let mut counts: HashMap<(usize, &str), usize> = HashMap::new();
    for f in &found {
        if let Some(k) = key(f) {
            *counts.entry(k).or_default() += 1;
        }
    }
    let mut out: Vec<Found> = Vec::new();
    // Ключ групи -> (зведене зауваження в `out`, скільки позицій виправляється)
    let mut merged: HashMap<(usize, &str), (usize, usize)> = HashMap::new();
    for mut f in found {
        let Some(k) = key(&f).filter(|k| counts[k] >= GROUP_MIN) else {
            out.push(f);
            continue;
        };
        let fix = f.fixes.drain(..).next();
        let (at, fixed) = match merged.get(&k) {
            Some(&slot) => slot,
            None => {
                let group = f.group.take().expect("ключ групи");
                f.msg = format!(
                    "{} з {} {}; перша: {}",
                    counts[&k], group.total, group.summary, group.example
                );
                out.push(f);
                (out.len() - 1, 0)
            }
        };
        let fixed = match fix {
            Some(fix) => {
                let target = &mut out[at].fixes;
                if target.is_empty() {
                    target.push(FixDraft {
                        title: String::new(),
                        edits: Vec::new(),
                    });
                }
                target[0].edits.extend(fix.edits);
                fixed + 1
            }
            None => fixed,
        };
        merged.insert(k, (at, fixed));
    }
    for (at, fixed) in merged.into_values() {
        if let Some(fix) = out[at].fixes.first_mut() {
            fix.title = format!("Виправити {fixed} {}", positions(fixed));
        }
    }
    out
}

/// «позицію», «позиції», «позицій» після числа.
fn positions(n: usize) -> &'static str {
    match (n % 10, n % 100) {
        (1, r) if r != 11 => "позицію",
        (2..=4, r) if !(12..=14).contains(&r) => "позиції",
        _ => "позицій",
    }
}

fn diagnostic(doc: &Doc, options: &Options, found: Found) -> Diagnostic {
    let file = &doc.files[found.loc.file];
    let source = &file.source;
    let range = first_line(source, found.loc.range.clone());
    let related = found
        .related
        .iter()
        .map(|(loc, note)| {
            let f = &doc.files[loc.file];
            let r = first_line(&f.source, loc.range.clone());
            (
                f.path.clone(),
                position(&f.source, r.start),
                position(&f.source, r.end),
                note.clone(),
            )
        })
        .collect();
    let mut fixes: Vec<Fix> = found
        .fixes
        .into_iter()
        .filter(|fix| fix.edits.iter().all(|(loc, _)| loc.file == found.loc.file))
        .map(|fix| Fix {
            title: fix.title,
            edits: fix
                .edits
                .into_iter()
                .map(|(loc, new)| edit(source, loc, new))
                .collect(),
            is_correction: true,
        })
        .collect();
    // Зауваження до самих коментарів вимикати нема чого
    if !found.rule.code.starts_with("ignore-") {
        fixes.push(suppress_fix(source, range.start, found.rule.code));
    }
    Diagnostic {
        path: file.path.clone(),
        start: position(source, range.start),
        end: position(source, range.end),
        range,
        rule: found.rule.code,
        clause: found.clause.unwrap_or(found.rule.clause),
        msg: found.msg,
        severity: options.severity_of(found.rule),
        unnecessary: found.rule.code == "ignore-unused",
        related,
        context: found
            .context
            .as_ref()
            .filter(|c| c.file == found.loc.file)
            .map(|c| {
                // Останній байт елемента -- на межі знака
                let text = source.text();
                let mut end = c.range.end.min(text.len()).max(c.range.start + 1) - 1;
                while end > 0 && !text.is_char_boundary(end) {
                    end -= 1;
                }
                (
                    position(source, c.range.start).line,
                    position(source, end).line,
                )
            }),
        fixes,
    }
}

fn edit(source: &Source, loc: Loc, new: String) -> Edit {
    Edit {
        start: position(source, loc.range.start),
        end: position(source, loc.range.end),
        old: source.text()[loc.range.clone()].to_owned(),
        range: loc.range,
        new,
    }
}

/// `// dstu-ignore-next-line: правило` над рядком зауваження.
fn suppress_fix(source: &Source, at: usize, rule: &str) -> Fix {
    let text = source.text();
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let indent: String = text[line_start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let loc = Loc {
        file: 0,
        range: line_start..line_start,
    };
    Fix {
        title: format!("Вимкнути {rule} для цього рядка"),
        edits: vec![edit(
            source,
            loc,
            format!("{indent}// dstu-ignore-next-line: {rule}\n"),
        )],
        is_correction: false,
    }
}

/// Багаторядковий діапазон (виклик `figure(…)` на пів екрана) обрізається до
/// першого рядка.
fn first_line(source: &Source, range: Range<usize>) -> Range<usize> {
    let slice = &source.text()[range.clone()];
    let end = slice.find('\n').unwrap_or(slice.len());
    range.start..range.start + slice[..end].trim_end().len()
}

pub fn position(source: &Source, byte: usize) -> Pos {
    let lines = source.lines();
    let line = lines.byte_to_line(byte).unwrap_or(0);
    let line_start = lines.line_to_byte(line).unwrap_or(0);
    Pos {
        line,
        column: lines.byte_to_column(byte).unwrap_or(0),
        utf16: lines
            .byte_to_utf16(byte)
            .unwrap_or(0)
            .saturating_sub(lines.byte_to_utf16(line_start).unwrap_or(0)),
    }
}

/// Застосовує виправлення-суть до тексту файлу: по одному на зауваження,
/// без перетинів. Повертає новий текст і кількість застосованих.
pub fn apply_fixes(text: &str, diagnostics: &[&Diagnostic]) -> (String, usize) {
    let (edits, count) = select_fixes(text, diagnostics);
    let mut edits = edits;
    edits.sort_by_key(|e| std::cmp::Reverse(e.range.start));
    let mut out = text.to_owned();
    for e in edits {
        out.replace_range(e.range.clone(), &e.new);
    }
    (out, count)
}

/// Наслідки «виправити все»: новий текст змінених файлів і кількість
/// виправлень.
pub struct Fixed {
    pub files: HashMap<PathBuf, String>,
    pub count: usize,
}

/// Виправляє все, що можна, доки є що виправляти: одне виправлення може
/// відкрити наступне (позиція з великої літери й крапкою спершу отримує лише
/// `list-case`, а після нього -- «крапка посеред переліку»). `only` обмежує
/// виправлення одним файлом.
pub fn fix_all(
    root: &Path,
    main: &Path,
    load: &dyn Fn(&Path) -> Option<String>,
    options: &Options,
    only: Option<&Path>,
) -> anyhow::Result<Fixed> {
    let mut files: HashMap<PathBuf, String> = HashMap::new();
    let mut count = 0;
    // Проходів небагато: кожен лише прибирає зауваження, а межа -- від зациклення
    for _ in 0..10 {
        let current = |path: &Path| files.get(path).cloned().or_else(|| load(path));
        let report = check(root, main, &current, options)?;
        let mut by_file: HashMap<&Path, Vec<&Diagnostic>> = HashMap::new();
        for diag in &report.diagnostics {
            if only.is_none_or(|p| p == diag.path) {
                by_file.entry(&diag.path).or_default().push(diag);
            }
        }
        let mut pass = 0;
        let mut updates = Vec::new();
        for (path, diags) in by_file {
            let Some(text) = current(path) else { continue };
            let (new, n) = apply_fixes(&text, &diags);
            if n > 0 {
                updates.push((path.to_path_buf(), new));
                pass += n;
            }
        }
        files.extend(updates);
        count += pass;
        if pass == 0 {
            break;
        }
    }
    Ok(Fixed { files, count })
}

/// Заміни для «виправити все»: перше виправлення-суть кожного зауваження,
/// якщо текст на місці не змінився й заміни не перетинаються. Повертає заміни
/// й кількість виправлених зауважень.
pub fn select_fixes<'a>(text: &str, diagnostics: &[&'a Diagnostic]) -> (Vec<&'a Edit>, usize) {
    let mut edits: Vec<&Edit> = Vec::new();
    let mut count = 0;
    for diag in diagnostics {
        let Some(fix) = diag.fixes.iter().find(|f| f.is_correction) else {
            continue;
        };
        let clashes = fix.edits.iter().any(|e| {
            text.get(e.range.clone()) != Some(e.old.as_str())
                || edits.iter().any(|o| {
                    e.range.start < o.range.end && o.range.start < e.range.end
                        || e.range.start == o.range.start
                })
        });
        if clashes {
            continue;
        }
        edits.extend(&fix.edits);
        count += 1;
    }
    (edits, count)
}

/// Коментарі `dstu-ignore` одного файлу.
#[derive(Default)]
struct Suppressions {
    entries: Vec<Suppression>,
}

/// Один коментар `dstu-ignore`.
struct Suppression {
    /// Рядок, який він вимикає (`None` -- увесь файл).
    line: Option<usize>,
    /// Байти коментаря у файлі.
    comment: std::ops::Range<usize>,
    /// Правила й місце кожної назви; порожньо -- усі.
    rules: Vec<(String, std::ops::Range<usize>)>,
    /// Що справді вимкнуло зауваження: кожне правило (чи «усі» -- нульове).
    used: Vec<bool>,
}

impl Suppressions {
    fn scan(source: &Source) -> Self {
        let mut out = Self::default();
        let mut offset = 0;
        out.walk(source, source.root(), &mut offset);
        out
    }

    fn walk(&mut self, source: &Source, node: &SyntaxNode, offset: &mut usize) {
        if node.children().len() == 0 {
            if matches!(
                node.kind(),
                SyntaxKind::LineComment | SyntaxKind::BlockComment
            ) {
                self.comment(source, node.leaf_text(), *offset);
            }
            *offset += node.len();
            return;
        }
        for child in node.children() {
            self.walk(source, child, offset);
        }
    }

    fn comment(&mut self, source: &Source, text: &str, offset: usize) {
        // `//` чи `/*`
        let opener = 2.min(text.len());
        let inner = text[opener..].trim_end_matches("*/");
        let lead = inner.len() - inner.trim_start().len();
        let body = inner.trim();
        let Some(rest) = body.strip_prefix("dstu-ignore") else {
            return;
        };
        let (kind, rest) = if let Some(r) = rest.strip_prefix("-next-line") {
            (1, r)
        } else if let Some(r) = rest.strip_prefix("-file") {
            (2, r)
        } else {
            (0, rest)
        };
        if rest.starts_with(|c: char| c.is_alphanumeric() || c == '-') {
            return;
        }
        // Назви правил з місцем у файлі
        let rest_at = offset + opener + lead + (body.len() - rest.len());
        let mut rules = Vec::new();
        let mut at = 0;
        for piece in rest.split_inclusive([',', ' ', '\t', ':']) {
            let name = piece.trim_end_matches([',', ' ', '\t', ':']);
            if !name.is_empty() {
                let start = rest_at + at;
                rules.push((name.to_owned(), start..start + name.len()));
            }
            at += piece.len();
        }
        let line = position(source, offset).line;
        let used = vec![false; rules.len().max(1)];
        self.entries.push(Suppression {
            line: match kind {
                2 => None,
                k => Some(line + k),
            },
            comment: offset..offset + text.len(),
            rules,
            used,
        });
    }

    /// Чи вимкнено правило `rule` у рядку `line`; коментар, що вимкнув,
    /// позначається використаним.
    fn covers(&mut self, rule: &str, line: usize) -> bool {
        let mut hit = false;
        for e in &mut self.entries {
            if e.line.is_some_and(|l| l != line) {
                continue;
            }
            if e.rules.is_empty() {
                e.used[0] = true;
                hit = true;
                continue;
            }
            for (k, (name, _)) in e.rules.iter().enumerate() {
                if name == rule || name == "*" {
                    e.used[k] = true;
                    hit = true;
                }
            }
        }
        hit
    }

    /// Невідомі правила й коментарі, що нічого не вимкнули. Невикористаним
    /// вважається лише правило, яке справді перевірялося: увімкнене, а для
    /// правил з повною частиною -- лише з фактами компіляції.
    fn report(
        &self,
        file: usize,
        source: &Source,
        with_facts: bool,
        active: &dyn Fn(&str) -> bool,
    ) -> Vec<Found> {
        let mut out = Vec::new();
        let rule = |code| rules::find(code).expect("службове правило");
        let (unknown_on, unused_on) = (active("ignore-unknown"), active("ignore-unused"));
        for e in &self.entries {
            let removal = |range: &std::ops::Range<usize>| -> FixDraft {
                // Єдине правило (чи «усі») -- увесь коментар, інакше -- назва
                // з комою
                let range = if e.rules.len() <= 1 {
                    comment_span(source.text(), e.comment.clone())
                } else {
                    name_span(source.text(), range.clone())
                };
                FixDraft {
                    title: "Прибрати зайве вимкнення".into(),
                    edits: vec![(Loc { file, range }, String::new())],
                }
            };
            if e.rules.is_empty() {
                if !e.used[0] && with_facts && unused_on {
                    out.push(Found::new(
                        rule("ignore-unused"),
                        Loc {
                            file,
                            range: e.comment.clone(),
                        },
                        "коментар dstu-ignore нічого не вимикає".into(),
                        vec![removal(&e.comment)],
                    ));
                }
                continue;
            }
            for (k, (name, range)) in e.rules.iter().enumerate() {
                let loc = Loc {
                    file,
                    range: range.clone(),
                };
                if name == "*" {
                    continue;
                }
                let Some(known) = rules::find(name) else {
                    if !unknown_on {
                        continue;
                    }
                    let hint = closest_rule(name)
                        .map(|r| format!(": можливо, «{r}»"))
                        .unwrap_or_default();
                    let fixes = closest_rule(name)
                        .map(|r| FixDraft {
                            title: format!("Замінити на «{r}»"),
                            edits: vec![(loc.clone(), r.into())],
                        })
                        .into_iter()
                        .collect();
                    out.push(Found::new(
                        rule("ignore-unknown"),
                        loc,
                        format!("невідоме правило «{name}» у dstu-ignore{hint}"),
                        fixes,
                    ));
                    continue;
                };
                let checked =
                    active(known.code) && (with_facts || (!known.full && known.probes == 0));
                if !e.used[k] && checked && unused_on {
                    out.push(Found::new(
                        rule("ignore-unused"),
                        loc,
                        format!("«{name}» тут не спрацьовує: вимикати нема чого"),
                        vec![removal(range)],
                    ));
                }
            }
        }
        out
    }
}

/// Найближча за написанням назва правила (одна-дві помилки).
fn closest_rule(name: &str) -> Option<&'static str> {
    RULES
        .iter()
        .map(|r| (edit_distance(name, r.code), r.code))
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d)
        .map(|(_, code)| code)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != *cb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Увесь коментар: якщо він єдиний у рядку -- з рядком, інакше -- з
/// пробілами перед ним.
fn comment_span(text: &str, range: std::ops::Range<usize>) -> std::ops::Range<usize> {
    let line_start = text[..range.start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[range.end..]
        .find('\n')
        .map_or(text.len(), |i| range.end + i);
    let before = &text[line_start..range.start];
    let after = &text[range.end..line_end];
    if before.trim().is_empty() && after.trim().is_empty() {
        let end = if line_end < text.len() {
            line_end + 1
        } else {
            line_end
        };
        line_start..end
    } else {
        let start = range.start - (before.len() - before.trim_end().len());
        start..range.end
    }
}

/// Назва правила з комою чи пробілами після неї (або перед нею, якщо
/// вона остання).
fn name_span(text: &str, range: std::ops::Range<usize>) -> std::ops::Range<usize> {
    let after = &text[range.end..];
    let tail = after.len() - after.trim_start_matches([',', ' ', '\t']).len();
    if after[..tail].contains(',') {
        return range.start..range.end + tail;
    }
    let before = &text[..range.start];
    let lead = before.len() - before.trim_end_matches([',', ' ', '\t']).len();
    range.start - lead..range.end
}
