//! Повна перевірка з реальним `typst` з PATH (без нього тести пропускаються):
//! правила спрацьовують на порушеннях, а проби не змінюють розкладки.

use std::path::{Path, PathBuf};
use std::process::Command;

use dsty::full::{self, Sections, Settings};
use dsty::lint::{self, Options};

fn typst() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let found = std::env::split_paths(&path)
        .map(|d| d.join("typst"))
        .find(|p| p.is_file());
    if found.is_none() {
        eprintln!("пропущено: typst немає в PATH");
    }
    found
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dsty-full-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

/// Повна перевірка документа: (правило, повідомлення) повних правил.
fn check(dir: &Path, src: &str, enable: &[&str]) -> Vec<(String, String)> {
    let main = dir.join("main.typ");
    std::fs::write(&main, src).unwrap();
    let options = Options {
        enable: enable.iter().map(|s| (*s).to_owned()).collect(),
        ..Default::default()
    };
    let facts = full::collect(&main, &Settings::default(), &options).expect("компіляція");
    let load = |p: &Path| std::fs::read_to_string(p).ok();
    let report = lint::check_full(dir, &main, &load, &options, Some(&facts)).unwrap();
    report
        .diagnostics
        .iter()
        .map(|d| (d.rule.to_owned(), d.message()))
        .collect()
}

fn has(found: &[(String, String)], rule: &str, part: &str) -> bool {
    found.iter().any(|(r, m)| r == rule && m.contains(part))
}

#[track_caller]
fn expect(found: &[(String, String)], wanted: &[(&str, &str)]) {
    for (rule, part) in wanted {
        assert!(
            has(found, rule, part),
            "немає [{rule}] «{part}» серед:\n{}",
            found
                .iter()
                .map(|(r, m)| format!("[{r}] {m}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

#[test]
fn page_text_and_headings() {
    if typst().is_none() {
        return;
    }
    let dir = temp_dir("page");
    let found = check(
        &dir,
        r#"#set page(paper: "a5", margin: 10mm, numbering: "1.", number-align: center + bottom)
#set text(size: 12pt, lang: "uk")
#set par(first-line-indent: 5mm, leading: 0.3em)
#set heading(numbering: "1.1.")
#align(center)[Титульний аркуш]
#pagebreak()
= Вступ
Перший абзац вступу, досить довгий, щоб займати кілька рядків і мати ключ прив'язки.

= Огляд
Перший абзац розділу, досить довгий, щоб займати кілька рядків і мати ключ прив'язки.

Другий абзац розділу, теж досить довгий, щоб займати кілька рядків і мати ключ.
== Підрозділ огляду
Текст підрозділу, досить довгий, щоб займати кілька рядків і мати ключ прив'язки.
#pagebreak()
Текст на другій сторінці, досить довгий, щоб займати кілька рядків і мати ключ.
= Висновки
Висновки роботи, досить довгі, щоб займати кілька рядків і мати ключ прив'язки.
"#,
        &["page-margins"],
    );
    expect(
        &found,
        &[
            ("page-size", "148 мм × 210 мм"),
            ("page-margins", "лівий 10 мм"),
            ("page-number", "на титульному аркуші"),
            ("page-number", "«2.»"),
            ("page-number", "center + bottom"),
            ("font-family", "основний текст"),
            ("font-size", "основний текст"),
            ("font-size", "заголовки рівня 1"),
            ("line-spacing", "основний текст"),
            ("par-indent", "5 мм"),
            ("par-indent", "перший абзац після заголовка"),
            ("heading-style", "великими літерами"),
            ("heading-number-format", "Прибрати крапку після номера"),
            ("heading-indent", "з абзацного відступу"),
            ("new-page", "«Висновки»"),
        ],
    );
    // Статичне зауваження не дублюється повною перевіркою
    assert_eq!(
        found
            .iter()
            .filter(|(r, _)| r == "heading-numbered")
            .count(),
        2
    );
}

#[test]
fn title_page_from_layout() {
    if typst().is_none() {
        return;
    }
    // Заголовок на першій сторінці: текст перед ним не титульний аркуш, а
    // основний текст, і номер на першій сторінці стандарт дозволяє
    let dir = temp_dir("title");
    let found = check(
        &dir,
        r#"#set page(numbering: "1", number-align: top + right)
#set text(size: 14pt, lang: "uk")
#text(size: 10pt)[Вступний рядок перед першим заголовком.]
= Вступ
Текст роботи.
"#,
        &[],
    );
    expect(&found, &[("font-size", "Вступний рядок")]);
    assert!(
        !has(&found, "page-number", "на титульному аркуші"),
        "{found:?}"
    );
}

#[test]
fn figures_tables_formulas_footnotes() {
    if typst().is_none() {
        return;
    }
    let dir = temp_dir("objects");
    let found = check(
        &dir,
        r#"#set text(lang: "en")
#set heading(numbering: "1.1")
#set figure(numbering: "1.")
#set math.equation(numbering: "1")
#show figure: set block(breakable: true)
= Розділ
Посилання на рисунок @far і таблицю @big. Формула @used.
#footnote[Перша.] #footnote[Друга.] #footnote[Третя.] #footnote[Четверта.] #footnote[П'ята.]

$ a = b $ <used>

$ c = d $ <unused>

Перелік:
- перший;
- другий.

Нумерований:
+ перший;
+ другий.

#pagebreak()
Сторінка два.
#pagebreak()
Сторінка три.
#figure(rect(width: 2cm, height: 1cm), caption: [Далекий рисунок]) <far>

#figure(
  table(columns: 2, ..range(80).map(i => (str(i), str(i * i))).flatten()),
  caption: [Велика таблиця],
) <big>
"#,
        &["caption-position"],
    );
    expect(
        &found,
        &[
            ("footnote-count", "виносок: 5"),
            ("footnote-numbering", "«1»"),
            ("footnote-separator", "48 мм"),
            ("formula-numbering", "без посилання"),
            ("formula-numbering", "без круглих дужок"),
            ("list-marker", "«•»"),
            ("list-marker", "«1.»"),
            ("caption-supplement", "«Figure»"),
            ("caption-supplement", "«Table»"),
            ("object-numbering", "«1.»"),
            ("figure-far-from-ref", "«Далекий рисунок» на с. 3"),
            ("table-no-header", "без головки"),
            ("table-continuation", "«Велика таблиця»"),
            ("caption-position", "під таблицями"),
        ],
    );
}

#[test]
fn structure_notes_and_appendices() {
    if typst().is_none() {
        return;
    }
    let dir = temp_dir("structure");
    let found = check(
        &dir,
        r#"#set text(lang: "uk", size: 14pt, font: "Times New Roman")
#set par(justify: true, first-line-indent: (amount: 12.5mm, all: true), leading: 0.95em)
#set heading(numbering: "1.1")
#show heading: set text(hyphenate: true)
#show math.equation: set align(left)
#set math.equation(numbering: "(1)")
#heading(numbering: none)[Реферат]
Звіт містить 9 с., 4 рис., 2 табл., 1 дод., 3 джерела.

Текст реферату, досить довгий, щоб займати кілька рядків і мати ключ прив'язки до коду.
#pagebreak()
#lorem(30)
#pagebreak()
= Розділ з дуже довгою назвою, що займає кілька рядків і має переноси слів у заголовку надзвичайно
Абзац з #text(fill: red)[червоним] текстом, досить довгий для ключа прив'язки до коду.

$ x = 1 $ <eq>

де x -- змінна, що описує щось, досить довга для ключа прив'язки до коду.

Посилання на @eq.

#par(first-line-indent: 0pt)[Примітка. Це примітка до тексту кеглем чотирнадцять.]
#set heading(supplement: [Додаток], numbering: "А.1")
#show heading.where(level: 1): it => block[ДОДАТОК З #linebreak() #it.body]
= Код програми
Текст додатка, досить довгий, щоб займати кілька рядків і мати ключ прив'язки до коду.
"#,
        &["abstract-length", "font-style"],
    );
    expect(
        &found,
        &[
            ("abstract-counts", "«9 с.»"),
            ("abstract-counts", "«4 рис»"),
            ("abstract-length", "2 с."),
            ("heading-hyphen", "переноси"),
            ("font-style", "«червоним»"),
            ("formula-center", "left"),
            ("formula-where-indent", "12,5 мм"),
            ("note-style", "кеглем 14"),
            ("appendix-letter", "«З»"),
            ("new-page", "додаток «Код програми»"),
        ],
    );
    // Кількість додатків у рефераті правильна
    assert!(!has(&found, "abstract-counts", "дод"));
}

#[test]
fn heading_on_last_line() {
    if typst().is_none() {
        return;
    }
    let dir = temp_dir("orphan");
    let found = check(
        &dir,
        r#"#set text(lang: "uk")
#show heading: set block(sticky: false)
Початок тексту документа, досить довгий для ключа прив'язки до коду.
#v(650pt)
= Заголовок унизу
Текст після заголовка, досить довгий, щоб займати кілька рядків і мати ключ прив'язки до коду, і ще трохи слів, щоб рядків було три.
"#,
        &[],
    );
    expect(
        &found,
        &[("heading-orphan", "«Заголовок унизу» стоїть унизу с. 1")],
    );
}

/// Документ з усіма видами блоків, які розмічають проби.
const LAYOUT_DOCUMENT: &str = r#"#set page(paper: "a4", margin: (top: 20mm, bottom: 20mm, left: 25mm, right: 10mm), numbering: "1", number-align: top + right)
#set text(lang: "uk", size: 14pt)
#set par(justify: true, leading: 0.95em, first-line-indent: (amount: 1.25cm, all: true))
#set heading(numbering: "1.1")
#set math.equation(numbering: "(1)")
#align(center)[Титульний аркуш]
#pagebreak()
#heading(numbering: none)[Вступ]
Вступ до роботи, досить довгий, щоб займати кілька рядків і мати ключ прив'язки до коду.

= Огляд предметної області
Перший абзац розділу, досить довгий, щоб займати кілька рядків і мати ключ прив'язки.
Схему наведено на рисунку @fig, дані -- у таблиці @tab, розрахунок -- за формулою @eq.

#figure(rect(width: 4cm, height: 2cm), caption: [Схема устатковання]) <fig>

#figure(
  table(columns: 2, table.header[Параметр][Значення], [Маса], [5 кг], [Довжина], [2 м]),
  caption: [Параметри устатковання],
) <tab>

$ m = F / a $ <eq>

Перелік вимог:
- перша вимога;
- друга вимога.

Текст з виноскою#footnote[Пояснення до тексту.], досить довгий, щоб мати ключ прив'язки.
== Перший підрозділ
Текст підрозділу, досить довгий, щоб займати кілька рядків і мати ключ прив'язки.
#heading(numbering: none)[Висновки]
Висновки роботи, досить довгі, щоб займати кілька рядків і мати ключ прив'язки до коду.
"#;

/// Позиції всіх блокових елементів і кількість сторінок без проб і з ними.
#[test]
fn probes_do_not_change_layout() {
    let Some(typst) = typst() else {
        return;
    };
    // Позиції читає `typst eval` (є з typst 0.15)
    let has_eval = Command::new(&typst)
        .args(["eval", "--help"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !has_eval {
        return;
    }
    let dir = temp_dir("layout");
    std::fs::write(dir.join("main.typ"), LAYOUT_DOCUMENT).unwrap();
    let query = "query(selector(heading).or(figure).or(par).or(footnote).or(math.equation).or(list).or(table)).map(e => { let p = e.location().position(); (repr(e.func()), p.page, calc.round(p.x.pt(), digits: 3), calc.round(p.y.pt(), digits: 3)) })";
    let positions = |input: String| {
        let mut child = Command::new(&typst)
            .args(["eval", "--in", "-", "--root"])
            .arg(&dir)
            .arg(query)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        use std::io::Write as _;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap()
    };
    let plain = positions("#include \"/main.typ\"\n".into());
    let probed = positions(full::wrapper(
        "main.typ",
        Sections {
            text: true,
            par: true,
            table: true,
        },
    ));
    assert!(plain.len() > 100, "{plain}");
    assert_eq!(plain, probed);
}

#[test]
fn skipped_without_typst() {
    let dir = temp_dir("no-typst");
    std::fs::write(dir.join("main.typ"), "= Розділ.\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dsty"))
        .current_dir(&dir)
        .env("PATH", "")
        .args(["check", "--full", "main.typ"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("повну перевірку пропущено"), "{stderr}");
    // Статичні зауваження лишаються
    assert!(String::from_utf8_lossy(&output.stdout).contains("[heading-period]"));
    assert_eq!(output.status.code(), Some(1));
}

/// Номер формули: праворуч і за порядком, у додатку з літерою. Номер
/// читається з прогону з дужками, а тіло формули не записується.
#[test]
fn formula_numbers() {
    if typst().is_none() {
        return;
    }
    let dir = temp_dir("formulas");
    let found = check(
        &dir,
        r#"#set text(lang: "uk")
#set heading(numbering: "1.1")
#set math.equation(numbering: "(1)", number-align: left)
= Розділ
Формула @a, досить довгий абзац, щоб мати ключ прив'язки до коду документа.

$ x = 2 + 1.25 "см" $ <a>

#set heading(supplement: [Додаток], numbering: "А.1")
#show heading.where(level: 1): it => block[ДОДАТОК А #linebreak() #it.body]
= Додаткові розрахунки
Формула @b, досить довгий абзац, щоб мати ключ прив'язки до коду документа.

$ y = 3 $ <b>
"#,
        &[],
    );
    expect(
        &found,
        &[
            ("formula-numbering", "крайньому правому"),
            ("formula-numbering", "з літерою додатка: «(А.1)»"),
        ],
    );
    assert!(!has(&found, "formula-numbering", "без круглих дужок"));
}

/// 7.10.6: відступ рядка «де …» береться таким, як його надруковано, а не як задано
/// в `set par`: `\` переносить «де» на другий рядок, `all: false` не
/// відступає абзац після формули, `#par(first-line-indent: 0pt)` і
/// `#h(-12.5mm)` знімають відступ.
#[test]
fn formula_where_indent() {
    if typst().is_none() {
        return;
    }
    let body = r#"#set math.equation(numbering: "(1)")
Межа має вигляд за формулами @a, @b, @c і @d, досить довгий абзац для ключа прив'язки.

$ beta = gamma / (2^gamma - 1) $ <a>\
де $gamma$ -- частотна ефективність, досить довгий рядок для ключа прив'язки до коду.

$ beta = gamma / (2^gamma - 1) $ <b>
де $beta$ -- енергетична ефективність, досить довгий рядок для ключа прив'язки до коду.

$ beta = gamma / (2^gamma - 1) $ <c>
#par(first-line-indent: 0pt)[де $x$ -- змінна, досить довгий рядок для ключа прив'язки до коду.]

$ y = x $ <d>
#h(-12.5mm) де $y$ -- функція, досить довгий рядок для ключа прив'язки до коду документа.
"#;
    let dir = temp_dir("where");
    let all = format!(
        "#set text(lang: \"uk\")\n#set par(first-line-indent: (amount: 12.5mm, all: true))\n{body}"
    );
    let found: Vec<_> = check(&dir, &all, &[])
        .into_iter()
        .filter(|(r, _)| r == "formula-where-indent")
        .collect();
    expect(
        &found,
        &[
            ("formula-where-indent", "порожній рядок"),
            ("formula-where-indent", "відступом 12,5 мм"),
        ],
    );
    assert_eq!(found.len(), 2, "{found:?}");

    // За `all: false` typst не відступає абзац після формули
    let dir = temp_dir("where-first");
    let first = format!("#set text(lang: \"uk\")\n#set par(first-line-indent: 12.5mm)\n{body}");
    let found = check(&dir, &first, &[]);
    assert!(
        !has(&found, "formula-where-indent", "відступом"),
        "{found:?}"
    );
    // `#h(-12.5mm)` без відступу абзацу: рядок виступає за берег
    expect(
        &found,
        &[("formula-where-indent", "виступає за лівий берег на 12,5 мм")],
    );
    assert!(
        !found
            .iter()
            .any(|(r, m)| r == "par-indent" && m.contains("перший абзац після")),
        "«де …» -- не перший абзац після блоку: {found:?}"
    );
}

/// Typst без шаблону: роздільник «:» у підписі, заголовок «Бібліографія»,
/// стиль джерел за абеткою, заголовок додатка великими літерами.
#[test]
fn plain_typst_defaults() {
    if typst().is_none() {
        return;
    }
    let dir = temp_dir("plain");
    std::fs::write(
        dir.join("refs.bib"),
        "@book{zz, title={Перша}, author={Яковенко, Я.}, year={2020}, publisher={П}}\n\
         @book{aa, title={Друга}, author={Антоненко, А.}, year={2021}, publisher={П}}\n",
    )
    .unwrap();
    let found = check(
        &dir,
        r#"#set text(lang: "uk")
#set heading(numbering: "1.1")
= Розділ
Спершу джерело @zz, потім @aa, рисунок -- на рис. @f[].

#figure(rect(), caption: [Схема]) <f>

#bibliography("refs.bib", style: "association-for-computing-machinery")

#set heading(supplement: [Додаток], numbering: "А.1")
#show heading.where(level: 1): it => block[ДОДАТОК А #linebreak() #upper(it.body)]
= Код програми <app>
Текст додатка, досить довгий, щоб мати ключ прив'язки до коду документа.
"#,
        &[],
    );
    expect(
        &found,
        &[
            ("caption-separator", "двокрапка «:»"),
            ("structure-name", "«Бібліографія»"),
            ("citation-order", "[2] надруковано раніше за [1]"),
            ("heading-style", "заголовки додатків надруковано великими"),
        ],
    );
}

/// Номер рисунка, складений нумерацією з частин (`context`, розмітка,
/// `strong`), друкується кількома прогонами й читається цілим, а окрема
/// крапка після номера лишається роздільником.
#[test]
fn split_figure_numbers() {
    if typst().is_none() {
        return;
    }
    let dir = temp_dir("split-numbers");
    let found = check(
        &dir,
        r#"#set text(lang: "uk")
#set heading(numbering: "1.1")
#set figure.caption(separator: [ -- ])
= Розділ
Текст розділу, досить довгий, щоб мати ключ прив'язки до коду документа.
#[
  #set figure.caption(separator: [. ])
  #figure(rect(), caption: [Крапка])
]

#set heading(supplement: [Додаток], numbering: "А.1")
#counter(heading).update(0)
= Слайди
#counter(figure.where(kind: image)).update(0)
Текст додатка, досить довгий, щоб мати ключ прив'язки до коду документа.
#let letter() = upper("абв".clusters().at(counter(heading).get().first() - 1))
#let app-letter = context letter()
#set figure(numbering: i => app-letter + "." + str(i))
#figure(rect(), caption: [Перший])
#set figure(numbering: i => [#context letter().#i])
#figure(rect(), caption: [Другий])
#set figure(numbering: i => context strong(letter()) + "." + str(i))
#figure(rect(), caption: [Третій])
#set figure(numbering: i => context [#letter().#i])
#figure(rect(), caption: [Четвертий])
#set figure(numbering: i => context [А.#i])
#figure(rect(), caption: [П'ятий])
"#,
        &[],
    );
    expect(&found, &[("caption-separator", "крапка «.»")]);
    let wrong: Vec<_> = found
        .iter()
        .filter(|(r, _)| r == "object-numbering")
        .collect();
    assert!(wrong.is_empty(), "{wrong:?}");
}

/// Документ з власним `foreground` (водяний знак) не вимикає перевірки
/// сторінок, а кількість сторінок у рефераті рахується правильно.
#[test]
fn own_page_foreground() {
    if typst().is_none() {
        return;
    }
    let dir = temp_dir("foreground");
    let found = check(
        &dir,
        r#"#set page(paper: "us-letter", foreground: place(center + horizon)[ЧЕРНЕТКА])
#set text(lang: "uk")
= Реферат
Звіт: 1 с., 0 рис., 0 табл.

= Вступ
Текст вступу.
"#,
        &[],
    );
    expect(&found, &[("page-size", "Задати А4")]);
    assert!(
        !found.iter().any(|(r, _)| r == "abstract-counts"),
        "{found:?}"
    );
}

/// Таблиця типових значень (`src/defaults.rs`) збігається з тим, що дає
/// typst з PATH: інакше статичні правила оформлення помиляються мовчки.
#[test]
fn defaults_match_typst() {
    use dsty::defaults as d;
    let Some(typst) = typst() else {
        return;
    };
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let out = Command::new(&typst)
        .args(["query", "defaults.typ", "<d>", "--field", "value"])
        .current_dir(&fixtures)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let entries: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap();
    let num = |v: &str, unit: &str| v.strip_suffix(unit).and_then(|n| n.parse::<f64>().ok());
    let near =
        |v: &str, unit: &str, want: f64| num(v, unit).is_some_and(|n| (n - want).abs() < 0.01);
    let quoted = |s: &str| format!("\"{s}\"");
    let weight = |w: i64| match w {
        400 => "\"regular\"",
        700 => "\"bold\"",
        _ => "?",
    };
    let mut wrong = Vec::new();
    let mut seen = 0;
    for e in &entries {
        let (k, v) = (e["k"].as_str().unwrap(), e["v"].as_str().unwrap());
        let parts: Vec<&str> = k.split('.').collect();
        let ok = match k {
            "text.size" => near(v, "pt", d::TEXT_SIZE),
            "text.font" => v == quoted(&d::FONT.to_lowercase()),
            "text.weight" => v == weight(d::WEIGHT),
            "text.lang" => v == quoted(d::LANG),
            "par.leading" => near(v, "em", d::LEADING_EM),
            "page.width" => d::PAPER == "a4" && near(v, "pt", 595.28),
            "page.height" => d::PAPER == "a4" && near(v, "pt", 841.89),
            "page.number-align" => v.replace(' ', "") == d::PAGE_NUMBER_ALIGN,
            "list.marker" => {
                let [a, b, c] = d::LIST_MARKERS;
                v == format!("([{a}], [{b}], [{c}])")
            }
            "enum.numbering" => v == quoted(d::ENUM_NUMBERING),
            "footnote.numbering" => v == quoted(d::FOOTNOTE_NUMBERING),
            "footnote.entry.separator" => {
                v.starts_with(&format!("line(length: {}% + 0pt", d::FOOTNOTE_LINE * 100.0))
            }
            "figure.caption.position" => v == if d::CAPTION_TOP { "top" } else { "bottom" },
            "math.equation.number-align" => v.replace(' ', "") == d::EQUATION_NUMBER_ALIGN,
            "footnote.size" => near(v, "pt", d::TEXT_SIZE * d::FOOTNOTE_SIZE_EM),
            "footnote.leading" => near(v, "em", d::FOOTNOTE_LEADING_EM),
            _ if k.starts_with("heading") && k.ends_with(".size") => {
                let level = k["heading".len()..k.len() - ".size".len()].parse().unwrap();
                near(v, "pt", d::TEXT_SIZE * d::heading_em(level))
            }
            _ if k.starts_with("heading") && k.ends_with(".weight") => {
                v == weight(d::HEADING_WEIGHT)
            }
            _ if parts[0] == "supplement" => {
                d::supplement(parts[1], parts[2]).is_some_and(|s| v == format!("[{s}]"))
            }
            _ if parts[0] == "separator" => {
                d::caption_separator(parts[1]).is_some_and(|s| v == format!("[{s}]"))
            }
            _ => panic!("невідоме значення {k}"),
        };
        seen += 1;
        if !ok {
            wrong.push(format!("{k}: typst {v}"));
        }
    }
    let version = Command::new(&typst).arg("--version").output().unwrap();
    assert!(seen >= 40, "лише {seen} значень");
    assert!(
        wrong.is_empty(),
        "{} має інші типові значення, ніж src/defaults.rs:\n{}",
        String::from_utf8_lossy(&version.stdout).trim(),
        wrong.join("\n")
    );
}

/// `--fix --full` перезбирає факти розкладки після запису виправлень, а не
/// показує зауваження зі старої компіляції.
#[test]
fn fix_full_recollects_facts() {
    if typst().is_none() {
        return;
    }
    let dir = temp_dir("fix-full");
    let main = dir.join("main.typ");
    std::fs::write(
        &main,
        r#"#set text(lang: "uk", size: 12pt)
#set heading(numbering: "1.1")
= Вступ
Текст вступу.
"#,
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dsty"))
        .current_dir(&dir)
        .args(["check", "--fix", "--full", "--format", "text", "main.typ"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let fixed = std::fs::read_to_string(&main).unwrap();
    assert!(
        fixed.contains("#heading(numbering: none)[Вступ]"),
        "{fixed}"
    );
    assert!(fixed.contains("size: 14pt"), "{fixed}");
    // Після виправлення `heading-numbered` зникає і з повної перевірки (факти перезібрано)
    assert!(!stdout.contains("[heading-numbered]"), "{stdout}");
    assert!(
        !stdout.contains("основний текст набрано кеглем"),
        "{stdout}"
    );
}

/// Шаблон із `set text(fill: black)` / `luma(0)` на заголовках і таблицях не
/// стирає позначки проб (`dtyk`).
#[test]
fn template_text_fill_does_not_clobber_probes() {
    if typst().is_none() {
        return;
    }
    let dir = temp_dir("fill-clobber");
    let src = r##"#set text(lang: "uk", size: 14pt)
#set heading(numbering: "1.1")
#show heading: set text(fill: black)
#show table: set text(fill: luma(0))
#show figure: set text(fill: rgb("#000000"))
#show outline: set heading(numbering: "1")
#outline()
= Розділ
Текст розділу, досить довгий для ключа прив'язки.
#figure(
  table(columns: 2, [Комірка таблиці з чорним текстом], [Значення]),
  caption: [Таблиця з чорним текстом],
) <t>
"##;
    let found = check(&dir, src, &[]);
    expect(&found, &[("heading-numbered", "надруковано з номером")]);
    // Перевіряємо й самі факти: прогони заголовка лишились `H`, а комірка
    // таблиці не стала абзацом основного тексту (`kind == ""`)
    let main = dir.join("main.typ");
    let facts = full::collect(&main, &Settings::default(), &Options::default()).unwrap();
    assert!(
        facts
            .entries
            .iter()
            .any(|e| matches!(&e.item, full::facts::Item::Run(r) if r.kind == "H")),
        "{:?}",
        facts.entries
    );
    assert!(
        !facts.entries.iter().any(|e| matches!(
            &e.item,
            full::facts::Item::Par(p) if p.kind.is_empty() && p.head.contains("Комірка")
        )),
        "{:?}",
        facts.entries
    );
}

/// Розділ із самим блоком коду не вважається «заголовком без тексту» (`heading-orphan`).
#[test]
fn heading_before_raw_block_is_not_orphan() {
    if typst().is_none() {
        return;
    }
    let dir = temp_dir("raw-content");
    let found = check(
        &dir,
        r#"#set text(lang: "uk")
= Лістинг коду
```rust
fn main() { println!("ok"); }
```
#pagebreak()
= Наступний розділ
Текст наступного розділу.
"#,
        &[],
    );
    assert!(
        !found.iter().any(|(r, _)| r == "heading-orphan"),
        "{found:?}"
    );
}

/// «Продовження…» у звичайному тексті нижче за перенесену таблицю не
/// приховує відсутності напису над перенесеною частиною таблиці; а кількість у
/// переліку реферату перевіряється `abstract-counts`.
#[test]
fn table_continuation_position_and_abstract_list_counts() {
    if typst().is_none() {
        return;
    }
    let dir = temp_dir("cont-and-abstract");
    let found = check(
        &dir,
        r#"#set text(lang: "uk")
#show figure: set block(breakable: true)
#heading(numbering: none)[Реферат]
Відомості про обсяг звіту:
- 99 с.;
- 0 рис.
#pagebreak()
= Розділ
Посилання на @big[таблицю].
#figure(
  table(columns: 2, table.header[А][Б], ..range(55).map(i => (str(i), str(i))).flatten()),
  caption: [Велика таблиця],
) <big>

Продовження досліджень планується в наступних розділах роботи.
"#,
        &[],
    );
    expect(
        &found,
        &[
            ("abstract-counts", "«99 с.»"),
            ("table-continuation", "«Велика таблиця»"),
        ],
    );
}

/// `--full` підставляє `Basis::Layout` для `page-size`, коли статичний обхід
/// не може простежити непрозорий шаблон (`Fact::Unknown`), і перевіряє
/// фактичний формат надрукованих посилань на джерела (`bibliography-style`)
/// навіть для нестандартного файлу `.csl`.
#[test]
fn full_upgrades_opaque_page_and_checks_custom_csl_style() {
    if typst().is_none() {
        return;
    }
    let dir = temp_dir("opaque-and-csl");
    std::fs::write(
        dir.join("refs.bib"),
        "@book{knuth, author = {Knuth, Donald}, title = {TAOCP}, year = {1997}}\n",
    )
    .unwrap();
    // Мінімальний CSL-стиль, що друкує `(Knuth, 1997)`, але назва файлу
    // `custom.csl` не потрапляє до статичного списку `is_author_date`
    std::fs::write(
        dir.join("custom.csl"),
        r#"<?xml version="1.0" encoding="utf-8"?>
<style xmlns="http://purl.org/net/xbiblio/csl" class="in-text" version="1.0">
  <info><title>Custom</title><id>custom</id></info>
  <citation>
    <layout prefix="(" suffix=")" delimiter="; ">
      <names variable="author"><name form="short"/></names>
      <date variable="issued" prefix=", "><date-part name="year"/></date>
    </layout>
  </citation>
  <bibliography>
    <layout><text variable="title"/></layout>
  </bibliography>
</style>
"#,
    )
    .unwrap();
    let src = r#"#let mk(body) = {
  // Динамічний вираз, непрозорий для статики: `set page(..d)` з невідомою умовою
  let p = if str(1) == "1" { (paper: "a5") } else { (paper: "a4") }
  set page(..p)
  body
}
#show: mk
#set text(lang: "uk")
= Вступ
У роботі використано джерело @knuth.
#bibliography("refs.bib", style: "custom.csl", title: [Перелік джерел посилання])
"#;
    let found = check(&dir, src, &[]);
    expect(
        &found,
        &[
            ("page-size", "148 мм × 210 мм"),
            ("bibliography-style", "Knuth"),
        ],
    );
}

/// Якщо документ уже задає `set page(paper: …)` у коді, `--full` не дублює
/// `page-size` через непрозорий підблок усередині документа.
#[test]
fn full_does_not_duplicate_known_page_size_on_opaque_subscope() {
    if typst().is_none() {
        return;
    }
    let dir = temp_dir("opaque-subscope");
    let src = r#"#set page(paper: "presentation-16-9")
#set text(lang: "uk")
= Вступ
Перший абзац.
#{
  let d = if str(1) == "1" { (fill: white) } else { (fill: black) }
  set page(..d)
  [Другий абзац у непрозорому підблоці.]
}
"#;
    let found = check(&dir, src, &[]);
    let page_sizes: Vec<_> = found.iter().filter(|(r, _)| r == "page-size").collect();
    assert_eq!(page_sizes.len(), 1, "{page_sizes:?}");
    assert!(
        page_sizes[0].1.contains("presentation-16-9"),
        "{page_sizes:?}"
    );
}

/// Статична модель (`Work`) збігається з фактичними стилями, берегами,
/// підписами й позначками переліків від `typst eval`.
#[test]
fn static_model_conforms_to_typst() {
    let Some(typst) = typst() else {
        return;
    };
    let has_eval = Command::new(&typst)
        .args(["eval", "--help"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !has_eval {
        return;
    }
    let dir = temp_dir("conform");
    let main = dir.join("main.typ");
    let src = r#"#set page(paper: "a4", margin: (top: 20mm, bottom: 20mm, left: 25mm, right: 10mm), numbering: "1", number-align: top + right)
#set text(lang: "uk", size: 14pt)
#set par(justify: true, leading: 0.95em, first-line-indent: (amount: 12.5mm, all: true))
#set heading(numbering: "1.1")
#set figure.caption(separator: [ -- ])
#show figure.where(kind: table): set figure.caption(position: top)
#set list(marker: [--])
#set enum(numbering: "1)")
#set math.equation(numbering: "(1)")
= Розділ
Текст розділу.
#figure(rect(width: 4cm, height: 2cm), caption: [Схема]) <f>
#figure(table(columns: 2, [А], [Б]), caption: [Таблиця]) <t>
$ x = y $ <eq>
- перший;
- другий.
+ крок один;
+ крок два.
"#;
    std::fs::write(&main, src).unwrap();
    let load = |p: &Path| std::fs::read_to_string(p).ok();
    let (work, _) = dsty::model::build(&dir, &main, &load, true).unwrap();

    let query = r#"{
      let plain(c) = if type(c) == str { c } else if type(c) != content { "" } else if c.has("text") { c.text } else if c.has("children") { c.children.fold("", (a, x) => a + plain(x)) } else if c.has("body") { plain(c.body) } else if c.func() == [ ].func() { " " } else { "" }
      let caps = query(figure.caption).map(c => (sep: plain(c.separator), top: c.position == top))
      let ls = query(list).map(l => plain(if type(l.marker) == array { l.marker.at(0) } else { l.marker }))
      let es = query(enum).map(e => plain(numbering(e.numbering, 1)))
      let st = query(<cf>).last().value
      (caps: caps, ls: ls, es: es, st: st)
    }"#;
    let probe = r#"#show par: it => {
      context [#metadata((size: text.size.to-absolute().pt(), w: page.width.to-absolute().pt(), h: page.height.to-absolute().pt()))<cf>]
      it
    }
    #include "/main.typ"
    "#;
    let mut child = Command::new(&typst)
        .args(["eval", "--in", "-", "--root"])
        .arg(&dir)
        .arg(query)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write as _;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(probe.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();

    let par_look = work
        .elements
        .iter()
        .find_map(|e| match e {
            dsty::work::Element::Par(p) => Some(&work.looks[p.look]),
            _ => None,
        })
        .unwrap();
    assert!(
        (par_look.text.size.known().copied().unwrap() - v["st"]["size"].as_f64().unwrap()).abs()
            < 0.05
    );
    let paper = par_look.paper.known().unwrap();
    assert!((paper.w.unwrap() - v["st"]["w"].as_f64().unwrap()).abs() < 0.1);
    assert!((paper.h.unwrap() - v["st"]["h"].as_f64().unwrap()).abs() < 0.1);

    let figs: Vec<_> = work
        .elements
        .iter()
        .filter_map(|e| match e {
            dsty::work::Element::Figure(f) => Some(f),
            _ => None,
        })
        .collect();
    assert_eq!(figs.len(), 2);
    for (i, f) in figs.iter().enumerate() {
        assert_eq!(
            f.look.separator.known().map(|s| s.trim()),
            Some(v["caps"][i]["sep"].as_str().unwrap().trim())
        );
        assert_eq!(
            f.look.top.known().copied(),
            Some(v["caps"][i]["top"].as_bool().unwrap())
        );
    }

    let lists: Vec<_> = work
        .elements
        .iter()
        .filter_map(|e| match e {
            dsty::work::Element::List(l) => Some(l),
            _ => None,
        })
        .collect();
    assert_eq!(
        lists[0].marker.known().map(String::as_str),
        v["ls"][0].as_str()
    );
    assert_eq!(
        lists[1].marker.known().map(String::as_str),
        v["es"][0].as_str()
    );
}
