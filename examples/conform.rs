//! Звірка статичної моделі ([`dsty::work`]) з компіляцією: що рушій
//! передбачив з коду і що надрукував typst. Не правило, а інструмент
//! розробки: показує, де відтворення поведінки typst розходиться з нею.
//!
//! ```text
//! cargo run --release --example conform -- [--typst ШЛЯХ] [-v] [-u] [-a] ФАЙЛ…
//! ```
//!
//! `-v`: елементи без пари й помилки компіляції; `-u`: де статика не знає
//! значення; `-a`: кожне порівняння.
//!
//! ```text
//! ```
//!
//! Потрібен typst 0.15+ (`typst eval`). Пари елементів шукаються за
//! текстом (найдовша спільна підпослідовність), тож пропущений чи зайвий
//! елемент не зсуває решту.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use dsty::full::{Sections, wrapper};
use dsty::model;
use dsty::work::{Element, Fact, FigureKind, Work};

/// Запит після компіляції: надруковані номери з тією самою нумерацією, що в
/// документі. Окремий від запиту повної перевірки: помилка тут її не зачепить.
const QUERY: &str = r#"{
  let plain(c) = {
    if type(c) == str { return c }
    if type(c) != content { return "" }
    if c.has("text") { return if type(c.text) == str { c.text } else { "" } }
    if c.has("children") { return c.children.fold("", (acc, x) => acc + plain(x)) }
    if c.has("body") { return plain(c.body) }
    if c.has("child") { return plain(c.child) }
    if c.func() == [ ].func() { return " " }
    ""
  }
  let printed(n, nums) = if n == none { none } else { plain(numbering(n, ..nums)) }
  let lab(e) = if e.has("label") { str(e.label) } else { none }
  query(selector(heading).or(figure).or(math.equation).or(footnote)).map(e => {
    let l = e.location()
    let f = e.func()
    if f == heading {
      (k: "heading", key: plain(e.body), num: printed(e.numbering, counter(heading).at(l)))
    } else if f == figure {
      (
        k: "figure",
        kind: repr(e.kind),
        key: if e.caption == none { "" } else { plain(e.caption.body) },
        supplement: plain(e.supplement),
        num: printed(e.numbering, e.counter.at(l)),
      )
    } else if f == math.equation {
      if e.block { (k: "equation", key: "", num: printed(e.numbering, counter(math.equation).at(l))) }
    } else if f == footnote {
      (k: "footnote", key: "", num: printed(e.numbering, counter(footnote).at(l)))
    }
  }).filter(x => x != none)
}"#;

#[derive(serde::Deserialize, Debug)]
struct Compiled {
    k: String,
    #[serde(default)]
    kind: String,
    key: String,
    #[serde(default)]
    supplement: Option<String>,
    num: Option<String>,
}

/// Підсумок одного виду фактів.
#[derive(Default)]
struct Tally {
    agree: usize,
    differ: usize,
    /// Статика мовчить (невідоме).
    unknown: usize,
    /// typst не дав тексту (нумерація через `context`).
    opaque: usize,
    /// Елемент лише в статиці чи лише в компіляції.
    unpaired: usize,
}

struct Ctx {
    verbose: bool,
    /// Показувати й невідоме статиці.
    unknowns: bool,
    /// Показувати кожне порівняння.
    all: bool,
    tally: BTreeMap<&'static str, Tally>,
}

impl Ctx {
    fn compare(&mut self, what: &'static str, at: &str, s: &Fact<Option<String>>, c: Option<&str>) {
        let t = self.tally.entry(what).or_default();
        let Some(s) = s.known() else {
            t.unknown += 1;
            if self.unknowns {
                println!("  {at}: {what}: статика не знає, typst {c:?}");
            }
            return;
        };
        if c == Some("") {
            t.opaque += 1;
            return;
        }
        let norm = |v: Option<&str>| v.map(|v| v.trim().to_owned());
        if norm(s.as_deref()) == norm(c) {
            t.agree += 1;
            if self.all {
                println!("  {at}: {what}: {s:?}");
            }
        } else {
            t.differ += 1;
            println!("  {at}: {what}: статика {s:?}, typst {c:?}");
        }
    }

    fn unpaired(&mut self, what: &'static str, at: &str, side: &str, key: &str) {
        self.tally.entry(what).or_default().unpaired += 1;
        if self.verbose {
            println!("  {at}: {what}: лише {side}: «{key}»");
        }
    }
}

/// Ключ для пари: літери й цифри малими.
fn key(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Пари індексів за найдовшою спільною підпослідовністю ключів.
fn align(a: &[String], b: &[String]) -> Vec<(Option<usize>, Option<usize>)> {
    let (n, m) = (a.len(), b.len());
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a[i] == b[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut raw) = (0, 0, Vec::new());
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            raw.push((Some(i), Some(j)));
            i += 1;
            j += 1;
        } else if j == m || i < n && dp[i + 1][j] >= dp[i][j + 1] {
            raw.push((Some(i), None));
            i += 1;
        } else {
            raw.push((None, Some(j)));
            j += 1;
        }
    }
    // Між парами стільки ж елементів з кожного боку: ті самі елементи з
    // різним текстом (підпис через функцію, формула в заголовку)
    let mut out = Vec::new();
    let mut gap: (Vec<usize>, Vec<usize>) = (Vec::new(), Vec::new());
    let flush = |gap: &mut (Vec<usize>, Vec<usize>), out: &mut Vec<_>| {
        if gap.0.len() == gap.1.len() {
            out.extend(
                gap.0
                    .drain(..)
                    .zip(gap.1.drain(..))
                    .map(|(i, j)| (Some(i), Some(j))),
            );
        } else {
            out.extend(gap.0.drain(..).map(|i| (Some(i), None)));
            out.extend(gap.1.drain(..).map(|j| (None, Some(j))));
        }
    };
    for pair in raw {
        match pair {
            (Some(i), None) => gap.0.push(i),
            (None, Some(j)) => gap.1.push(j),
            p => {
                flush(&mut gap, &mut out);
                out.push(p);
            }
        }
    }
    flush(&mut gap, &mut out);
    out
}

/// Пакунки проєкту: `vendor/typst-packages` у теці документа чи вище (так їх
/// підключає mise у робочих проєктах). Задає `TYPST_PACKAGE_PATH` і для
/// статики, і для typst; без такої теки лишається те, що було на початку.
fn use_packages(main: &Path, initial: &Option<std::ffi::OsString>) {
    let found = main
        .ancestors()
        .map(|d| d.join("vendor/typst-packages"))
        .find(|d| d.is_dir());
    // SAFETY: інструмент однопотоковий, середовище читає лише він і typst.
    unsafe {
        match found
            .map(|p| p.into_os_string())
            .or_else(|| initial.clone())
        {
            Some(p) => std::env::set_var("TYPST_PACKAGE_PATH", p),
            None => std::env::remove_var("TYPST_PACKAGE_PATH"),
        }
    }
}

fn compile(typst: &Path, root: &Path, main: &Path) -> anyhow::Result<Vec<Compiled>> {
    let rel = main
        .strip_prefix(root)?
        .to_string_lossy()
        .replace('\\', "/");
    let sections = Sections {
        text: false,
        par: false,
        table: false,
    };
    let mut child = Command::new(typst)
        .args(["eval", "--in", "-", "--root"])
        .arg(root)
        .arg(QUERY)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(wrapper(&rel, sections).as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        anyhow::bail!(
            "typst: {}",
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .next()
                .unwrap_or("")
        );
    }
    Ok(serde_json::from_slice(&out.stdout)?)
}

/// Номери, які `typst eval` не надрукує (нумерація через `context`), з
/// прогонів тексту повної перевірки: слово й номер у підписі, номер формули
/// в дужках. Фігури й формули обох запитів ідуть в одному порядку.
fn fill_printed(compiled: &mut [Compiled], typst: &Path, root: &Path, main: &Path) {
    use dsty::full::facts::Item;
    let settings = dsty::full::Settings {
        typst: Some(typst.to_owned()),
        root: Some(root.to_owned()),
        timeout: None,
    };
    let Ok(facts) = dsty::full::collect(main, &settings, &dsty::lint::Options::default()) else {
        return;
    };
    let number = regex::Regex::new(r"^\S+\s+([\dА-ЯҐЄІЇA-Z]{1,3}(?:\.\d+)*)(?:\s|$)").unwrap();
    let e = &facts.entries;
    // Прогони виду `kind` від запису `i` до наступного такого самого елемента
    let runs = |i: usize, kind: &str, stop: &dyn Fn(&Item) -> bool| -> Vec<String> {
        e[i + 1..]
            .iter()
            .take_while(|x| !stop(&x.item))
            .filter_map(|x| match &x.item {
                Item::Run(r) if r.kind == kind && !r.t.trim().is_empty() => {
                    Some(r.t.trim().to_owned())
                }
                _ => None,
            })
            .collect()
    };
    let mut figures = Vec::new();
    let mut equations = Vec::new();
    for (i, x) in e.iter().enumerate() {
        match &x.item {
            Item::Figure { num, .. } => {
                let joined = runs(i, "c", &|it| matches!(it, Item::Figure { .. })).join(" ");
                let printed = number.captures(&joined).map(|c| c[1].to_owned());
                figures.push(num.as_ref().and(printed));
            }
            Item::Equation { num, .. } => {
                let printed = runs(i, "e", &|it| matches!(it, Item::Equation { .. }))
                    .into_iter()
                    .find(|t| t.starts_with('(') && t.ends_with(')'));
                equations.push(num.as_ref().and(printed));
            }
            _ => {}
        }
    }
    for (k, found) in [("figure", figures), ("equation", equations)] {
        let mine: Vec<&mut Compiled> = compiled.iter_mut().filter(|c| c.k == k).collect();
        if mine.len() != found.len() {
            continue;
        }
        for (c, printed) in mine.into_iter().zip(found) {
            if c.num.as_deref() == Some("")
                && let Some(p) = printed
            {
                c.num = Some(p);
            }
        }
    }
}

fn check(ctx: &mut Ctx, typst: &Path, main: &Path) -> anyhow::Result<()> {
    let load = |p: &Path| std::fs::read_to_string(p).ok();
    let root = model::detect_root(main, &load);
    let (w, code): (Work, _) =
        model::build(&root, main, &load, true).ok_or_else(|| anyhow::anyhow!("не прочитано"))?;
    let mut compiled = compile(typst, &root, main)?;
    if compiled.iter().any(|c| c.num.as_deref() == Some("")) {
        fill_printed(&mut compiled, typst, &root, main);
    }
    println!("{}", main.display());
    let place = |loc: &dsty::text::Loc| {
        let f = &code.files[loc.file];
        let line = dsty::lint::position(&f.source, loc.range.start).line + 1;
        format!(
            "{}:{line}",
            f.path.file_name().unwrap_or_default().to_string_lossy()
        )
    };
    let of = |k: &str| compiled.iter().filter(|c| c.k == k).collect::<Vec<_>>();

    // Заголовки: надрукований номер
    let heads: Vec<_> = w
        .elements
        .iter()
        .filter_map(|e| match e {
            Element::Heading(h) => Some(h),
            _ => None,
        })
        .collect();
    let ch = of("heading");
    let a: Vec<_> = heads.iter().map(|h| key(h.title.trimmed())).collect();
    let b: Vec<_> = ch.iter().map(|c| key(&c.key)).collect();
    for pair in align(&a, &b) {
        match pair {
            (Some(i), Some(j)) => {
                let h = heads[i];
                if let Some(look) = &h.look {
                    ctx.compare(
                        "номер заголовка",
                        &place(&h.loc),
                        &look.number,
                        ch[j].num.as_deref(),
                    );
                }
            }
            (Some(i), None) => ctx.unpaired(
                "заголовок",
                &place(&heads[i].loc),
                "у статиці",
                heads[i].title.trimmed(),
            ),
            (None, Some(j)) => ctx.unpaired("заголовок", "", "у typst", &ch[j].key),
            (None, None) => {}
        }
    }

    // Рисунки й таблиці: номер і слово підпису
    for (kind, name) in [(FigureKind::Image, "image"), (FigureKind::Table, "table")] {
        let figs: Vec<_> = w
            .elements
            .iter()
            .filter_map(|e| match e {
                Element::Figure(f) if f.kind == kind => Some(f),
                _ => None,
            })
            .collect();
        let cf: Vec<_> = of("figure")
            .into_iter()
            .filter(|c| c.kind == name)
            .collect();
        let a: Vec<_> = figs
            .iter()
            .map(|f| {
                f.caption
                    .as_ref()
                    .map_or(String::new(), |c| key(c.trimmed()))
            })
            .collect();
        let b: Vec<_> = cf.iter().map(|c| key(&c.key)).collect();
        for pair in align(&a, &b) {
            match pair {
                (Some(i), Some(j)) => {
                    let f = figs[i];
                    let at = place(&f.loc);
                    let number = match (&f.number, f.look.numbering.known()) {
                        (_, Some(None)) => f.look.numbering.clone(),
                        (Fact::Known(n, why), _) => Fact::Known(Some(n.clone()), why.clone()),
                        _ => Fact::Unknown,
                    };
                    ctx.compare("номер фігури", &at, &number, cf[j].num.as_deref());
                    if f.caption.is_some() {
                        let s = match &f.look.supplement {
                            Fact::Known(s, why) => Fact::Known(Some(s.clone()), why.clone()),
                            Fact::Unknown => Fact::Unknown,
                        };
                        ctx.compare("слово підпису", &at, &s, cf[j].supplement.as_deref());
                    }
                }
                (Some(i), None) => ctx.unpaired("фігура", &place(&figs[i].loc), "у статиці", &a[i]),
                (None, Some(j)) => ctx.unpaired("фігура", "", "у typst", &cf[j].key),
                (None, None) => {}
            }
        }
    }

    // Формули: за порядком
    let eqs: Vec<_> = w
        .elements
        .iter()
        .filter_map(|e| match e {
            Element::Equation(q) => Some(q),
            _ => None,
        })
        .collect();
    let ce = of("equation");
    let a = vec![String::new(); eqs.len()];
    let b = vec![String::new(); ce.len()];
    for pair in align(&a, &b) {
        match pair {
            (Some(i), Some(j)) => {
                let q = eqs[i];
                let number = match (&q.number, q.look.numbering.known()) {
                    (_, Some(None)) => q.look.numbering.clone(),
                    (Fact::Known(n, why), _) => Fact::Known(Some(n.clone()), why.clone()),
                    _ => Fact::Unknown,
                };
                ctx.compare(
                    "номер формули",
                    &place(&q.loc),
                    &number,
                    ce[j].num.as_deref(),
                );
            }
            (Some(i), None) => ctx.unpaired("формула", &place(&eqs[i].loc), "у статиці", ""),
            (None, Some(_)) => ctx.unpaired("формула", "", "у typst", ""),
            (None, None) => {}
        }
    }

    // Виноски: позначка першої
    if let (Some(look), Some(first)) = (&w.footnotes.look, of("footnote").first())
        && !w.footnotes.at.is_empty()
    {
        let m = match &look.marker {
            Fact::Known(m, why) => Fact::Known(Some(m.clone()), why.clone()),
            Fact::Unknown => Fact::Unknown,
        };
        ctx.compare("позначка виноски", "", &m, first.num.as_deref());
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let mut typst = PathBuf::from("typst");
    let mut files = Vec::new();
    let mut verbose = false;
    let mut unknowns = false;
    let mut all = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--typst" => typst = args.next().map(PathBuf::from).unwrap_or(typst),
            "-v" => verbose = true,
            "-u" => unknowns = true,
            "-a" => all = true,
            _ => files.push(PathBuf::from(a)),
        }
    }
    let mut ctx = Ctx {
        verbose,
        unknowns,
        all,
        tally: BTreeMap::new(),
    };
    let (mut ok, mut failed) = (0, 0);
    let initial = std::env::var_os("TYPST_PACKAGE_PATH");
    for f in &files {
        let main = match std::fs::canonicalize(f) {
            Ok(m) => m,
            Err(e) => {
                failed += 1;
                eprintln!("{}: {e}", f.display());
                continue;
            }
        };
        use_packages(&main, &initial);
        match check(&mut ctx, &typst, &main) {
            Ok(()) => ok += 1,
            Err(e) => {
                failed += 1;
                if verbose {
                    eprintln!("{}: {e:#}", f.display());
                }
            }
        }
    }
    println!("\nдокументів: {ok}, не скомпільовано: {failed}");
    println!(
        "{:<18} {:>6} {:>6} {:>9} {:>9} {:>8}",
        "факт", "збіг", "різне", "невідоме", "непрозоре", "без пари"
    );
    for (what, t) in &ctx.tally {
        println!(
            "{what:<18} {:>6} {:>6} {:>9} {:>9} {:>8}",
            t.agree, t.differ, t.unknown, t.opaque, t.unpaired
        );
    }
    Ok(())
}
