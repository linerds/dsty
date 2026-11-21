//! Повна перевірка з реальним `typst` з PATH (без нього тести пропускаються):
//! правила спрацьовують на порушеннях, мовчать на документі за шаблоном nure,
//! а проби не змінюють розкладки.

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
        .map(|d| (d.rule.to_owned(), d.msg.clone()))
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
            ("font-size", "заголовок «Вступ»"),
            ("line-spacing", "основний текст"),
            ("par-indent", "5 мм"),
            ("par-indent", "перший абзац після заголовка"),
            ("heading-style", "«Огляд» надруковано не великими"),
            ("heading-number-format", "«2.1.»"),
            ("heading-indent", "«Підрозділ огляду»"),
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
            ("caption-position", "під таблицею"),
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
            ("note-size", "кеглем 14"),
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

/// Документ за шаблоном nure (поруч у `../typst_nure_template`): повна
/// перевірка не має зауважень, крім свідомих рішень шаблону.
fn nure_document(dir: &Path) -> Option<String> {
    let repo = std::env::var_os("NURE_TEMPLATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../typst_nure_template")
        });
    let src = repo.join("src");
    if !src.join("lib.typ").is_file() {
        eprintln!("пропущено: немає {}", src.display());
        return None;
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&src, dir.join("lib")).unwrap();
    Some(
        r#"#import "/lib/lib.typ": *
#show: style.dstu.with(skip: 1)
#align(center)[Титульний аркуш]
#pagebreak()
#utils.nheading[Вступ]
Вступ до роботи, досить довгий, щоб займати кілька рядків і мати ключ прив'язки до коду.
Його мета -- показати, що повна перевірка не має зауважень до шаблону.

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
== Другий підрозділ
Текст підрозділу, досить довгий, щоб займати кілька рядків і мати ключ прив'язки.
#utils.nheading[Висновки]
Висновки роботи, досить довгі, щоб займати кілька рядків і мати ключ прив'язки до коду.
"#
        .to_owned(),
    )
}

#[test]
fn nure_template_is_clean() {
    if typst().is_none() {
        return;
    }
    let dir = temp_dir("nure");
    let Some(src) = nure_document(&dir) else {
        return;
    };
    let found = check(&dir, &src, &[]);
    let full: Vec<&(String, String)> = found
        .iter()
        .filter(|(r, _)| dsty::rules::find(r).is_some_and(|r| r.full))
        .collect();
    assert!(
        full.is_empty(),
        "зауваження до документа за шаблоном: {full:?}"
    );
}

/// Позиції всіх блокових елементів і кількість сторінок без проб і з ними.
#[test]
fn probes_do_not_change_layout() {
    let Some(typst) = typst() else {
        return;
    };
    let dir = temp_dir("layout");
    let Some(src) = nure_document(&dir) else {
        return;
    };
    std::fs::write(dir.join("main.typ"), src).unwrap();
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
            equation: true,
            runs: true,
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

/// Номер формули: праворуч і за порядком, у додатку -- з літерою. Номер
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
            ("formula-numbering", "за порядком має бути «(А.1)»"),
        ],
    );
    assert!(!has(&found, "formula-numbering", "без круглих дужок"));
}

/// 7.10.6: відступ рядка «де …» -- як його надруковано, а не як задано
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
    // `#h(-12.5mm)` без відступу абзацу -- рядок виступає за берег
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
            (
                "heading-style",
                "заголовок додатка «Код програми» надруковано великими",
            ),
        ],
    );
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
    expect(&found, &[("page-size", "має бути А4")]);
    assert!(
        !found.iter().any(|(r, _)| r == "abstract-counts"),
        "{found:?}"
    );
}
