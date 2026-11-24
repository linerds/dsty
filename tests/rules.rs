//! Правила на маленьких документах у пам'яті: кожне правило спрацьовує на
//! порушенні й мовчить на правильному коді.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use dsty::lint::{self, Diagnostic, Options, Report};

/// Мінішаблон (`tests/fixtures`): лежить у корені кожного проєкту тестів.
const FIXTURES: &[(&str, &str)] = &[
    ("tpl.typ", include_str!("fixtures/tpl.typ")),
    ("style.typ", include_str!("fixtures/style.typ")),
];

/// Перевіряє проєкт з файлів у пам'яті; main є першим файлом.
fn run_files(files: &[(&str, &str)], options: &Options) -> Report {
    let root = PathBuf::from("/project");
    let map: HashMap<PathBuf, String> = FIXTURES
        .iter()
        .chain(files)
        .map(|(p, t)| (root.join(p), (*t).to_owned()))
        .collect();
    let load = |path: &Path| map.get(path).cloned();
    lint::check(&root, &root.join(files[0].0), &load, options).unwrap()
}

fn run(src: &str) -> Vec<Diagnostic> {
    run_files(&[("main.typ", src)], &Options::default()).diagnostics
}

fn rules(src: &str) -> Vec<&'static str> {
    run(src).iter().map(|d| d.rule).collect()
}

/// Правила оформлення: у тестових документах без повного шаблону їх дають
/// типові значення typst.
const LOOKS: &[&str] = &[
    "font-size",
    "font-family",
    "page-number",
    "new-page",
    "appendix-letter",
    "heading-style",
    "heading-indent",
    "heading-hyphen",
];

/// Правила, крім оформлення.
fn content_rules(src: &str) -> Vec<&'static str> {
    rules(src)
        .into_iter()
        .filter(|r| !LOOKS.contains(r))
        .collect()
}

/// Текст, на який вказує зауваження.
fn pointed<'a>(src: &'a str, d: &Diagnostic) -> &'a str {
    &src[d.range.clone()]
}

/// Застосовує перше виправлення-суть і повертає новий текст.
fn fixed(src: &str) -> String {
    let report = run(src);
    let diags: Vec<&Diagnostic> = report.iter().collect();
    lint::apply_fixes(src, &diags).0
}

/// Код після швидких виправлень редактора: перше виправлення суті кожного
/// зауваження, також того, що спирається на припущення (`--fix` таких не
/// застосовує).
fn quick_fixed(src: &str) -> String {
    apply_quick(src, &run(src))
}

/// Перше виправлення суті кожного з `diags`, як його пропонує редактор.
fn apply_quick(src: &str, diags: &[Diagnostic]) -> String {
    let mut edits: Vec<_> = diags
        .iter()
        .filter_map(|d| d.fixes.iter().find(|f| f.is_correction))
        .flat_map(|f| f.edits.clone())
        .collect();
    edits.sort_by_key(|e| std::cmp::Reverse(e.range.start));
    let mut out = src.to_owned();
    for e in edits {
        out.replace_range(e.range.clone(), &e.new);
    }
    out
}

/// Обгортки мінішаблону.
const TPL: &str = "#import \"/tpl.typ\": *\n";
/// Документ за мінішаблоном.
const DSTU: &str = "#import \"/tpl.typ\": *\n#show: style.dstu\n";

#[test]
fn headings() {
    let src = format!("{DSTU}= Розділ.\n== мала літера\n== HTTP запити\n");
    let diags = run(&src);
    let found: Vec<_> = diags.iter().map(|d| (d.rule, pointed(&src, d))).collect();
    assert_eq!(
        found,
        [
            ("heading-period", "Розділ."),
            ("heading-case", "мала літера"),
        ]
    );
    assert_eq!(
        fixed(&src),
        format!("{DSTU}= Розділ\n== Мала літера\n== HTTP запити\n")
    );
}

#[test]
fn structural_numbering_and_order() {
    // Із застосованим шаблоном `= Вступ` нумерується
    assert_eq!(rules(&format!("{DSTU}= Вступ\n")), ["heading-numbered"]);
    assert_eq!(
        fixed(&format!("{DSTU}= Вступ <intro>\nТекст.\n")),
        format!("{DSTU}#heading(numbering: none)[Вступ] <intro>\nТекст.\n")
    );
    // Без шаблону й з явним вимкненням не нумерується
    assert!(rules("= Вступ\n").is_empty());
    assert!(
        rules(&format!(
            "{DSTU}#nheading[Вступ]\n#heading(numbering: none)[Висновки]\n"
        ))
        .is_empty()
    );
    assert!(rules(&format!("{DSTU}#set heading(numbering: none)\n= Вступ\n")).is_empty());

    let src = format!("{DSTU}#nheading[Рекомендації]\n#nheading[Висновки]\n");
    assert_eq!(rules(&src), ["structure-order"]);
    // Реферат від шаблону (`abstract`) стоїть після вступу
    let src = format!(
        "{DSTU}#nheading[Вступ]\n#abstract(w, keywords: (\"a\",\"b\",\"c\",\"d\",\"e\"))[Текст.]\n"
    );
    assert_eq!(rules(&src), ["structure-order"]);
}

#[test]
fn single_subheading() {
    let src = format!("{DSTU}= Розділ\n== Єдиний\nТекст.\n= Інший\n== Перший\n== Другий\n");
    let diags = run(&src);
    assert_eq!(diags.len(), 1);
    assert_eq!(
        (diags[0].rule, pointed(&src, &diags[0])),
        ("heading-single", "Єдиний")
    );
}

#[test]
fn figures() {
    let src = "\
#import \"/tpl.typ\": *
Див. @ok[], @lower[] і @tbl[].

#figure(rect(), caption: [Добрий]) <ok>
#figure(rect(), caption: [мала літера]) <lower>
#figure(rect(), caption: [Без посилання])
#figure(rect(), caption: [Раніше]) <early>
Посилання на @early[].
#figure(rect(), caption: [Без нумерації], numbering: none)
#figure(raw(\"x\"), caption: [Лістинг])
#figure(code(read(\"a.c\")), caption: [Лістинг-обгортка])
#tab(caption: [Таблиця], columns: 2, header: ([Назва], [час.]), label: <tbl>, [a], [1])
#fig(path(\"a.png\"), [Обгортка], label: <none-ref>)
";
    let diags = run(src);
    let found: Vec<_> = diags.iter().map(|d| (d.rule, pointed(src, d))).collect();
    // Перше посилання стає пов'язаним місцем
    assert_eq!(diags[2].related[0].1.line, 7);
    assert_eq!(
        found,
        [
            ("caption-case", "мала літера"),
            ("figure-unreferenced", "Без посилання"),
            ("figure-before-ref", "Раніше"),
            ("table-header-period", "час."),
            ("table-header-case", "час."),
            ("figure-unreferenced", "Обгортка"),
        ]
    );
}

#[test]
fn table_header_in_figure() {
    let src = "\
@t1[] @t2[]
#figure(table(columns: 2, table.header([Назва], [значення]), [a], [1]), caption: [Одна]) <t1>
#figure(table(columns: 1, table.header([Група], [підгрупа]), [a]), caption: [Дві]) <t2>
";
    let diags = run(src);
    assert_eq!(diags.len(), 1, "{diags:#?}");
    assert_eq!(
        (diags[0].rule, pointed(src, &diags[0])),
        ("table-header-case", "значення")
    );
}

#[test]
fn lists() {
    let src = "\
Перелік:
- Велика літера;
- мала літера;
- HTTP -- абревіатура;
- без крапки

Абзац без двокрапки.
- перший;
- другий.

Вкладений:
- перший рівень:
  + другий рівень;
  + ще один.
- кінець.
";
    let diags = run(src);
    let found: Vec<_> = diags.iter().map(|d| (d.rule, pointed(src, d))).collect();
    assert_eq!(
        found,
        [
            ("list-case", "Велика"),
            ("list-punct", "крапки"),
            ("list-colon", "."),
        ]
    );
    let out = fixed(src);
    assert!(out.contains("- велика літера;"), "{out}");
    assert!(out.contains("- без крапки.\n"), "{out}");
    assert!(out.contains("Абзац без двокрапки:\n"), "{out}");
}

#[test]
fn multiline_item_points_at_the_problem() {
    let src = "Перелік:\n- перший рядок\n  другий рядок,\n- Наступний\n  кінець.\n";
    let diags = run(src);
    let found: Vec<_> = diags.iter().map(|d| (d.rule, pointed(src, d))).collect();
    assert_eq!(
        found,
        [("list-punct", "рядок,"), ("list-case", "Наступний")]
    );
    assert_eq!(diags[0].start.line, 2);
    assert_eq!(
        fixed(src),
        "Перелік:\n- перший рядок\n  другий рядок;\n- наступний\n  кінець.\n"
    );
}

#[test]
fn list_period_in_the_middle_and_semicolon_at_the_end() {
    let src = "Умови:\n+ перша;\n+ друга.\n+ третя;\n\nВкладений:\n- зовнішній:\n  + внутрішній;\n  + останній;\n- кінець.\n\nПозицію 3) перевіряють першою.\n";
    let diags = run(src);
    let found: Vec<_> = diags.iter().map(|d| (d.rule, pointed(src, d))).collect();
    assert_eq!(found, [("list-punct", "друга."), ("list-punct", "третя;")]);
    assert!(diags[0].msg.contains("посеред переліку"));
    let out = fixed(src);
    assert!(out.contains("+ друга;\n+ третя.\n"), "{out}");
    assert!(out.contains("+ останній;\n"), "{out}");

    // Дві помилки в одній позиції дають два зауваження; після кількох речень теж «;»
    let src = "Кроки:\n- Відкрито вікно.\n- натиснуто кнопку. Далі X.\n- кінець.\n";
    assert_eq!(rules(src), ["list-case", "list-punct", "list-punct"]);
}

#[test]
fn numbered_list_without_references() {
    // 7.7.2: нумерований перелік без посилань на позиції позначають «–»
    let src = "Завдання:\n+ завдання 1: розробити;\n+ завдання 2: перевірити.\n";
    let diags = run(src);
    let found: Vec<_> = diags.iter().map(|d| (d.rule, pointed(src, d))).collect();
    assert_eq!(found, [("list-unreferenced", "+")], "{diags:?}");
    // Посилання шукаються лише за зразками: виправлення пропонується, але
    // `--fix` його не застосовує
    assert_eq!(fixed(src), src);
    assert_eq!(
        quick_fixed(src),
        "Завдання:\n- завдання 1: розробити;\n- завдання 2: перевірити.\n"
    );
    // Посилання на позицію, маркований і вкладений перелік: мовчить
    for ok in [
        "Кроки:\n+ перший;\n+ другий.\n\nУ пункті 2) описано друге.\n",
        "Кроки:\n- перший;\n- другий.\n",
        "Кроки:\n+ перший:\n  - а;\n  - б.\n+ другий.\n",
    ] {
        assert!(
            !rules(ok).contains(&"list-unreferenced"),
            "{ok}: {:?}",
            run(ok)
        );
    }
}

#[test]
fn nested_list_needs_colon() {
    let src = "Вихідні дані:\n+ параметри джерела;\n  - рівень;\n  - шум.\n+ параметри каналу\n  - смуга;\n  - відстань.\n+ кінець.\n";
    let diags = run(src);
    let found: Vec<_> = diags.iter().map(|d| (d.rule, pointed(src, d))).collect();
    assert_eq!(
        found,
        [("list-colon", "джерела;"), ("list-colon", "каналу")]
    );
    let out = fixed(src);
    assert!(out.contains("+ параметри джерела:\n"), "{out}");
    assert!(out.contains("+ параметри каналу:\n"), "{out}");
}

#[test]
fn list_after_footnote_and_metadata() {
    // Виноска після двокрапки й невидимі виклики не заважають
    let src = "Перелік:#footnote[Пояснення.]\n#expect(\"x\")\n- перший;\n- другий.\n";
    assert!(rules(src).is_empty(), "{:?}", rules(src));
}

#[test]
fn formula_where() {
    let src = "Формула\n$ v = s / t, $\nде: $s$ -- шлях.\n\n$ a = F / m, $\nде $F$ -- сила.\n";
    let diags = run(src);
    assert_eq!(
        diags.iter().map(|d| d.rule).collect::<Vec<_>>(),
        ["formula-where"]
    );
    assert!(quick_fixed(src).contains("де $s$ -- шлях."));
}

#[test]
fn keywords() {
    let src = format!("{TPL}#abstract(w, keywords: (\"один\", \"два\"))[Текст.]\n");
    assert_eq!(rules(&src), ["keywords-count"]);
    // Рецепт: `abstract: (keywords: …)` у `show`
    let src = format!("{TPL}#show: coursework.with(abstract: (keywords: (\"a\",), text: []))\n");
    assert_eq!(content_rules(&src), ["keywords-count"]);
    let src = format!("{TPL}#abstract(w, keywords: (\"a\", \"b\", \"c\", \"d\", \"e\"))[Текст.]\n");
    assert!(rules(&src).is_empty());
}

#[test]
fn appendices() {
    let src = format!(
        "{DSTU}Додатки: @b[], потім @a[] і @c[].\n#show: style.appendices\n= Перший <a>\n= Другий <b>\n= Без посилання\n= з малої <c>\n"
    );
    assert_eq!(
        rules(&src),
        ["appendix-order", "appendix-unreferenced", "appendix-case"]
    );
}

#[test]
fn appendices_from_recipe_and_include() {
    // Рецепт друкує додатки в кінці, хоч вони в аргументах на початку
    let main = format!(
        "{TPL}#show: coursework.with(bib-path: path(\"b.yml\"), appendices: include \"app.typ\")\n= Вступ\nКод у @code[].\n"
    );
    let app = "= Код програми <code>\nТекст.\n";
    let report = run_files(
        &[("main.typ", &main), ("app.typ", app)],
        &Options::default(),
    );
    let found: Vec<_> = report.diagnostics.iter().map(|d| d.rule).collect();
    // `= Вступ` нумерується (шаблон), а додаток іде після переліку джерел
    assert_eq!(found, ["heading-numbered"]);
    assert_eq!(report.files.len(), 2);
}

#[test]
fn dynamic_labels_are_not_guessed() {
    let src = format!(
        "{TPL}#show: style.appendices\n#for f in (\"a\", \"b\") [\n  #heading[Файл #f] #label(str(\"app_\" + f))\n]\n#heading[Файл] #label(\"lit\")\n"
    );
    assert_eq!(content_rules(&src), ["appendix-unreferenced"]);
}

#[test]
fn bibliography() {
    assert_eq!(
        rules("Посилання на джерело [1].\n"),
        ["bibliography-missing"]
    );
    assert!(rules("Посилання [1].\n#bibliography(\"refs.bib\")\n").is_empty());
    // Не посилання: індекс масиву, назва алгоритму
    assert!(rules("Масив `a[8]` і Keccak-f[1600].\n").is_empty());
}

#[test]
fn units_are_optional() {
    let src = "Плата 80 х 25 х 50 мм, відстань 5мм, від 1 до 5 мм і 10--20 МГц.\nПравильно: 5 мм, від 1 мм до 5 мм, 80 мм × 25 мм, 2010--2015 роки.\n";
    assert!(rules(src).is_empty());
    let options = Options {
        enable: vec![
            "unit-space".into(),
            "unit-range".into(),
            "unit-dimensions".into(),
        ],
        ..Default::default()
    };
    let report = run_files(&[("main.typ", src)], &options);
    let found: Vec<_> = report
        .diagnostics
        .iter()
        .map(|d| (d.rule, pointed(src, d)))
        .collect();
    assert_eq!(
        found,
        [
            ("unit-dimensions", "80 х 25 х 50 мм"),
            ("unit-space", "5мм"),
            ("unit-range", "від 1 до 5 мм"),
            ("unit-range", "10--20 МГц"),
        ]
    );
    let diags: Vec<&Diagnostic> = report.diagnostics.iter().collect();
    let out = lint::apply_fixes(src, &diags).0;
    assert!(out.contains("відстань 5~мм"), "{out}");
    assert!(out.contains("Плата 80 мм × 25 мм × 50 мм,"), "{out}");
    assert!(out.contains("від 1 мм до 5 мм і"), "{out}");
    // `--` у коді не збігається з текстом, тож без виправлення
    assert!(out.contains("10--20 МГц"), "{out}");
}

#[test]
fn ignore_comments() {
    let src = "\
// dstu-ignore-next-line: list-case
- Велика;
- Ще одна; // dstu-ignore
- І ще.
";
    let diags = run(src);
    assert_eq!(diags.len(), 1);
    assert_eq!(pointed(src, &diags[0]), "І");
    assert!(rules("// dstu-ignore-file: list-case\n- Велика.\n").is_empty());
    // Вимкнення через виправлення
    let fix = diags[0].fixes.last().unwrap();
    assert!(!fix.is_correction);
    assert_eq!(fix.edits[0].new, "// dstu-ignore-next-line: list-case\n");
}

#[test]
fn options() {
    let options = Options {
        ignore: vec!["list-case".into()],
        ..Default::default()
    };
    let report = run_files(&[("main.typ", "- Велика.\n")], &options);
    assert!(report.diagnostics.is_empty());
    let bad = Options {
        enable: vec!["no-such-rule".into()],
        ..Default::default()
    };
    assert!(lint::validate(&bad).is_err());
}

#[test]
fn broken_source_still_checked() {
    // Незакрита дужка далі в коді не заважає перевірці того, що вище
    let src = "= Вступ.\n#let x = (\n";
    assert_eq!(rules(src), ["heading-period"]);
}

#[test]
fn fix_all_repeats_until_stable() {
    // Велика літера й крапка: спершу лише list-case, після нього крапка
    let src = "Визначити:\n- Швидкість потоку.\n- Обрати модуляцію.\n- розробити схему.\n";
    let root = PathBuf::from("/project");
    let main = root.join("main.typ");
    let load = |p: &Path| (p == main).then(|| src.to_owned());
    let fixed = lint::fix_all(&root, &main, &load, &Options::default(), None).unwrap();
    assert_eq!(
        fixed.files[&main],
        "Визначити:\n- швидкість потоку;\n- обрати модуляцію;\n- розробити схему.\n"
    );
    assert_eq!(fixed.count, 4);
}

fn run_with(src: &str, enable: &[&str]) -> Vec<Diagnostic> {
    let options = Options {
        enable: enable.iter().map(|s| (*s).to_owned()).collect(),
        ..Default::default()
    };
    run_files(&[("main.typ", src)], &options).diagnostics
}

#[test]
fn structure_names() {
    let src = "= Висновок\n== Література\n";
    let diags = run(src);
    let found: Vec<_> = diags.iter().map(|d| (d.rule, pointed(src, d))).collect();
    assert_eq!(
        found,
        [
            ("structure-name", "Висновок"),
            ("structure-name", "Література")
        ]
    );
    assert_eq!(
        quick_fixed(src),
        "= Висновки\n== Перелік джерел посилання\n"
    );
    // Для набраного великими літерами виправлення теж великими
    assert_eq!(
        quick_fixed("= СПИСОК ЛІТЕРАТУРИ\n"),
        "= ПЕРЕЛІК ДЖЕРЕЛ ПОСИЛАННЯ\n"
    );
    // Записи переліку джерел під неканонічною назвою не перевіряються як перелік за 7.7
    assert_eq!(
        rules("= Список літератури\n+ Іванов І. Книга. 2020\n+ Петров П. Стаття. 2021\n"),
        ["structure-name"]
    );
    assert!(rules("= Висновки\n== Огляд літератури\n").is_empty());
}

#[test]
fn heading_abbreviations_are_optional() {
    let src = "= Розділ\n== Мережі, протоколи тощо\n== Рис. і табл. у звіті\n";
    assert!(rules(src).is_empty());
    let diags = run_with(src, &["heading-abbrev"]);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(pointed(src, &diags[0]), "Рис.");
}

#[test]
fn bibliography_arguments() {
    let src = "Джерело [1].\n#bibliography(\"refs.bib\", full: true, style: \"apa\")\n";
    let diags = run(src);
    let found: Vec<_> = diags.iter().map(|d| (d.rule, pointed(src, d))).collect();
    assert_eq!(
        found,
        [
            ("bibliography-full", "true"),
            ("bibliography-style", "\"apa\"")
        ]
    );
    assert!(fixed(src).contains("full: false"));
    assert_eq!(
        rules("#set bibliography(style: \"chicago-author-date\")\n#bibliography(\"r.bib\")\n"),
        ["bibliography-style"]
    );
    assert!(rules("#bibliography(\"r.bib\", style: \"ieee\", full: false)\n").is_empty());
}

#[test]
fn assumptions() {
    // Примітку й пояснення познак стандарт визначає першим словом: це видно
    // з коду напевно, тож `--fix` їх виправляє
    let src = "Формула\n$ v = s / t, $\nде: $s$ -- шлях.\n\nПримітка: пояснення.\n";
    let diags = run(src);
    assert_eq!(
        diags.iter().map(|d| d.rule).collect::<Vec<_>>(),
        ["formula-where", "note-format", "note-format"]
    );
    assert!(diags.iter().all(|d| d.assumed.is_none()), "{diags:?}");
    assert_eq!(
        fixed(src),
        "Формула\n$ v = s / t, $\nде $s$ -- шлях.\n\nПримітка. Пояснення.\n"
    );
    // Слово лише схоже на «Примітка»; «де» не під формулою
    assert!(rules("Примітками називають пояснення.\n\nДе: в тексті.\n").is_empty());
    // Назва структурного елемента вгадана з поширеного варіанта: зауваження
    // так і каже, а `--fix` його не перейменовує
    let src = "= Література\n";
    let diags = run(src);
    assert_eq!(diags.len(), 1);
    assert!(diags[0].assumed.is_some());
    assert!(diags[0].auto_fix().is_none());
    assert!(diags[0].message().contains("припущення: "));
    assert_eq!(fixed(src), src);
}

#[test]
fn template_show_ref() {
    // Шаблон сам друкує посилання: як саме, з коду не видно, тож про слово
    // перед посиланням правило мовчить
    let tpl = "#let doc(body) = {\n  show ref: it => it.element.body\n  body\n}\n";
    let main = "#import \"/own.typ\": doc\n#show: doc\nНаведено у таблиці @t.\n\n#figure(table(columns: 1, [a]), caption: [Дані]) <t>\n";
    let found = run_files(&[("main.typ", main), ("own.typ", tpl)], &Options::default());
    assert!(
        !found.diagnostics.iter().any(|d| d.rule == "ref-supplement"),
        "{:?}",
        found.diagnostics
    );
    // Без шаблону — типовий «Таблиця N» typst
    let plain = "Наведено у таблиці @t.\n\n#figure(table(columns: 1, [a]), caption: [Дані]) <t>\n";
    assert!(rules(plain).contains(&"ref-supplement"));
}

#[test]
fn note_numbering() {
    // Одна примітка з номером
    let src = "Текст.\n\nПримітка 1. Пояснення.\n";
    assert_eq!(rules(src), ["note-numbering"]);
    assert_eq!(quick_fixed(src), "Текст.\n\nПримітка. Пояснення.\n");
    // Кілька поспіль: без номера й з неправильним номером
    let src = "Примітка. Перша.\n\nПримітка 3. Друга.\n\nПримітка 3. Третя.\n";
    assert_eq!(rules(src), ["note-numbering", "note-numbering"]);
    assert_eq!(
        quick_fixed(src),
        "Примітка 1. Перша.\n\nПримітка 2. Друга.\n\nПримітка 3. Третя.\n"
    );
    // Окремі примітки в різних місцях: кожна сама по собі
    assert!(rules("Примітка. Одна.\n\nТекст.\n\nПримітка. Інша.\n").is_empty());
}

#[test]
fn unit_tolerance() {
    let src = "Довжина 80 ± 2 мм, а (65 ± 3) % і 80 мм ± 2 мм -- правильно.\n";
    let diags = run(src);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].rule, "unit-tolerance");
    assert_eq!(pointed(src, &diags[0]), "80 ± 2 мм");
    assert!(fixed(src).starts_with("Довжина (80 ± 2) мм,"));
}

#[test]
fn manual_keywords() {
    let src = "= Реферат\nЗвіт: 10 с., 2 рис.\n\nМЕРЕЖА, АЛГОРИТМ, БАЗА ДАНИХ, ЯКІСТЬ, ЗАТРИМКА.\n\nТекст реферату.\n";
    assert!(rules(src).is_empty());
    let diags = run_with(src, &["keywords-order"]);
    assert_eq!(diags.len(), 1);
    assert!(diags[0].msg.contains("«АЛГОРИТМ» стоїть після «МЕРЕЖА»"));
    assert!(apply_quick(src, &diags).contains("АЛГОРИТМ, БАЗА ДАНИХ, ЗАТРИМКА, МЕРЕЖА, ЯКІСТЬ.\n"));
    // Кількість перевіряється й для набраних вручну
    assert_eq!(
        rules("= Реферат\nАЛГОРИТМ, БАЗА ДАНИХ, МЕРЕЖА.\n"),
        ["keywords-count"]
    );
    // У масиві шаблону порядок не перевіряється: шаблон сортує сам
    let src = format!("{TPL}#abstract(w, keywords: (\"Я\", \"Б\", \"В\", \"Г\", \"Д\"))[Текст.]\n");
    assert!(run_with(&src, &["keywords-order"]).is_empty());
}

#[test]
fn abbreviations_order() {
    let src = "= Скорочення та умовні познаки\n/ ЕОМ: електронна обчислювальна машина\n/ БД: база даних\n/ ПЗ: програмне забезпечення\n";
    let diags = run(src);
    assert_eq!(diags.len(), 1);
    assert_eq!(pointed(src, &diags[0]), "БД");
    // Спершу українські, потім латиницею
    let src = "= Перелік скорочень\n- CPU -- центральний процесор;\n- ОС -- операційна система.\n";
    let diags = run(src);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert!(diags[0].msg.contains("спершу"));
    // Таблиця: перша колонка
    let src = "= Скорочення та умовні познаки\n#table(columns: 2, [ПЗ], [програмне забезпечення], [БД], [база даних])\n";
    assert_eq!(rules(src), ["abbrev-order"]);
    // Кількість колонок з виразу не видно: таблиця не перевіряється, а не
    // береться за одноколонкову
    let src = "= Скорочення та умовні познаки\n#table(columns: (auto,) * 2, [БД], [база даних], [ПЗ], [програмне забезпечення])\n";
    assert!(rules(src).is_empty(), "{:?}", run(src));
    let ok = "= Скорочення та умовні познаки\n/ БД: база даних\n/ ЕОМ: машина\n/ API: інтерфейс\n";
    assert!(rules(ok).is_empty());
}

#[test]
fn case_only_for_plain_text() {
    // Код, формула й латиниця на початку означають назви, а не регістр речення
    let src = "Класи:\n- `Dorm` має кімнати;\n- Windows -- система;\n- $X$ -- змінна;\n- Кінець.\n";
    let diags = run(src);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(pointed(src, &diags[0]), "Кінець.");
    assert!(
        rules("Див. @t[].\n#figure(table(columns: 1)[x], caption: [`main` -- функція]) <t>\n")
            .is_empty()
    );
}

#[test]
fn list_violations_are_grouped() {
    let src = "Кроки:\n- Відкрити вікно;\n- Натиснути кнопку;\n- Закрити вікно;\n- Перевірити.\n";
    let diags = run(src);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].rule, "list-case");
    assert!(
        diags[0]
            .msg
            .starts_with("4 з 4 позицій переліку з великої літери")
    );
    assert_eq!(pointed(src, &diags[0]), "Відкрити");
    assert_eq!(diags[0].fixes[0].title, "Виправити 4 позиції");
    assert_eq!(
        fixed(src),
        "Кроки:\n- відкрити вікно;\n- натиснути кнопку;\n- закрити вікно;\n- перевірити.\n"
    );
    // Вимкнене коментарем у групу не входить
    let src = "Кроки:\n- Відкрити; // dstu-ignore\n- Натиснути;\n- закрити.\n";
    assert_eq!(rules(src), ["list-case"]);
}

#[test]
fn heading_from_string() {
    // `#heading("ІСПИТ")` дає заголовок з рядка, а не порожній
    assert!(rules("#heading(\"ІСПИТ\")\nТекст.\n").is_empty());
    assert_eq!(rules("#heading(\"Розділ.\")\n"), ["heading-period"]);
}

#[test]
fn refs_inside_figures_and_links() {
    // Посилання з клітинки таблиці й `link(<мітка>)` теж рахуються
    let src = "\
Див. табл. @t[].
#figure(table(columns: 1, [див. рис. @f[]]), caption: [Таблиця з посиланням]) <t>
#figure(rect(), caption: [Рисунок]) <f>
#link(<g>)[Схема] ще раз.
#figure(rect(), caption: [Інший рисунок]) <g>
";
    assert!(rules(src).is_empty(), "{:?}", rules(src));
}

#[test]
fn ref_supplement() {
    let src = "\
На рисунку @f показано схему, а дані -- в @t.

@f ілюструє схему. Див. @f[рис.], на рисунку @f[] і (рис. @f[]).

Згідно з формулою @e і (@t).
#figure(rect(), caption: [Схема]) <f>
#figure(table(columns: 1)[x], caption: [Дані]) <t>
$ x = 1 $ <e>
";
    let diags = run(src);
    let found: Vec<_> = diags.iter().map(|d| (d.rule, pointed(src, d))).collect();
    assert_eq!(
        found,
        [
            ("ref-supplement", "@f"),
            ("ref-supplement", "@t"),
            ("ref-supplement", "@e"),
            ("ref-supplement", "@t"),
        ]
    );
    assert!(diags[0].msg.contains("уже стоїть «рисунку»"));
    assert_eq!(diags[0].fixes[0].title, "У посилання: @f[рисунку]");
    let out = fixed(src);
    // Слово переходить у підпис посилання: клікабельне «рисунку 1»
    assert!(out.contains("На @f[рисунку] показано"), "{out}");
    assert!(out.contains("в @t[табл.]."), "{out}");
    assert!(out.contains("(@t[табл.])"), "{out}");
    assert!(out.contains("Згідно з @e[формулою]"), "{out}");
    // Власний підпис з малої літери чи власне show-правило ref не перевіряються
    let src = "#set figure(supplement: [рис.])\nНа @f.\n#figure(rect(), caption: [Схема]) <f>\n";
    assert!(!rules(src).contains(&"ref-supplement"));
    let src = "#show ref: it => it\nНа @f.\n#figure(rect(), caption: [Схема]) <f>\n";
    assert!(rules(src).is_empty());
}

#[test]
fn citation_order() {
    let src = "Відомо [1], а також [2, 3].\n\nДалі [5] і [4].\n#bibliography(\"r.bib\")\n";
    let diags = run(src);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].rule, "citation-order");
    assert_eq!(pointed(src, &diags[0]), "[5]");
    assert!(diags[0].msg.contains("раніше за [4]"));
    // Діапазони й повтори
    let bib = "#bibliography(\"r.bib\")\n";
    assert!(rules(&format!("Огляд [1–3], потім [2] і [4]--[6], [7].\n{bib}")).is_empty());
    assert!(rules(&format!("Роботи [1]--[3] і [4].\n{bib}")).is_empty());
    // Перелік джерел і додатки не враховуються
    let src = "Текст [1].\n= Перелік джерел посилання\n+ [3] Книга.\n";
    assert!(!rules(src).contains(&"citation-order"));
    // Інтервал у формулі й індекс у коді не є посиланнями
    let src = "Інтервал $[3, 6]$, масив `a [2]`, а джерело [1].\n";
    let diags = run(src);
    let found: Vec<_> = diags.iter().map(|d| (d.rule, pointed(src, d))).collect();
    assert_eq!(found, [("bibliography-missing", "[1]")]);
    assert!(rules(&format!("На $[3, 6]$ і [1], далі [2].\n{bib}")).is_empty());
}

#[test]
fn formula_comma() {
    let src = "Маємо\n$ a = b $\n$ c = d. $\n$ e = f. $\nде $a$ -- щось.\n";
    let diags = run(src);
    let found: Vec<_> = diags.iter().map(|d| d.rule).collect();
    assert_eq!(found, ["formula-comma", "formula-comma"], "{diags:?}");
    assert_eq!(
        fixed(src),
        "Маємо\n$ a = b, $\n$ c = d, $\n$ e = f. $\nде $a$ -- щось.\n"
    );
    // Позначка стоїть на закривальному `$`, наступна формула стає пов'язаним місцем
    assert_eq!(pointed(src, &diags[0]), "$");
    assert_eq!(diags[0].related[0].3, "наступна формула");
    assert_eq!(diags[0].context, Some((1, 2)));
    // Текст між формулами робить їх окремими реченнями
    assert!(rules("$ a = b $\nабо\n$ c = d $\n").is_empty());
    // Код, перелік термінів, таблиця між формулами теж розривають «поспіль»
    for between in [
        "```\ncode\n```",
        "/ Термін: опис",
        "#table([a])",
        "#image(\"a.png\")",
    ] {
        let src = format!("$ a = b $\n{between}\n$ c = d $\n");
        assert!(rules(&src).is_empty(), "{between}: {:?}", run(&src));
    }
    // Проміжок чи розрив сторінки не вважаються текстом
    assert_eq!(rules("$ a = b $\n#v(1em)\n$ c = d $\n"), ["formula-comma"]);
    // Формули в позиції переліку: поспіль лише в тій самій позиції
    let src = "Маємо:\n- струм:\n  $ a = b $ $ c = d $\n- напруга:\n  $ e = f $\n";
    let diags = run(src);
    let found: Vec<_> = diags.iter().map(|d| (d.rule, pointed(src, d))).collect();
    assert_eq!(found, [("formula-comma", "$")], "{diags:?}");
    assert_eq!(diags[0].range.start, src.find("b $").unwrap() + 2);
}

#[test]
fn no_fix_after_question_or_ellipsis() {
    // Дописаний знак дав би «?;» чи «...:»: зауваження лишається, виправлення немає
    let src = "Питання до роботи:\n- що таке клас?\n- що таке об'єкт?\n\nРозглянемо питання...\n- перше;\n- друге.\n";
    let diags = run(src);
    let found: Vec<_> = diags.iter().map(|d| d.rule).collect();
    assert_eq!(
        found,
        ["list-punct", "list-punct", "list-colon"],
        "{diags:?}"
    );
    assert!(
        diags
            .iter()
            .all(|d| d.fixes.iter().all(|f| !f.is_correction))
    );
    assert_eq!(fixed(src), src);
    // Кома чи крапка з комою перед переліком замінюються, а не доповнюються
    let src = "Розглянемо таке,\n- перше;\n- друге.\n";
    assert_eq!(fixed(src), "Розглянемо таке:\n- перше;\n- друге.\n");
}

#[test]
fn ignore_comment_problems() {
    // Помилка в назві дає підказку й виправлення
    let src = "// dstu-ignore-next-line: list-cse\n- Велика.\n";
    let diags = run(src);
    let found: Vec<_> = diags.iter().map(|d| (d.rule, pointed(src, d))).collect();
    assert_eq!(
        found,
        [("ignore-unknown", "list-cse"), ("list-case", "Велика.")]
    );
    assert_eq!(diags[0].fixes[0].title, "Замінити на «list-case»");
    assert!(fixed(src).starts_with("// dstu-ignore-next-line: list-case\n"));

    // Нічого не вимикає: прибрати коментар (рядок цілком)
    let src = "Перелік:\n// dstu-ignore-next-line: list-case\n- мала.\n";
    let diags = run(src);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].rule, "ignore-unused");
    assert!(diags[0].unnecessary);
    assert_eq!(diags[0].severity, lint::Severity::Hint);
    assert_eq!(fixed(src), "Перелік:\n- мала.\n");

    // Одна назва з кількох: прибрати лише її
    let src = "- Велика. // dstu-ignore: list-case, list-punct\n";
    let diags = run(src);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(pointed(src, &diags[0]), "list-punct");
    assert_eq!(fixed(src), "- Велика. // dstu-ignore: list-case\n");

    // Правила повної перевірки без компіляції не перевірялися: не зайві
    assert!(rules("// dstu-ignore-file: heading-orphan, heading-numbered\nТекст.\n").is_empty());
    // Без переліку (усі правила) теж
    assert!(rules("Текст. // dstu-ignore\n").is_empty());
}

#[test]
fn text_and_page_settings() {
    let src = "#set page(paper: \"a5\", margin: 10mm)\n#set text(size: 12pt, lang: \"uk\")\n#set par(leading: 0.3em, first-line-indent: 5mm)\n= Огляд\nТекст.\n";
    let options = Options {
        enable: vec!["page-margins".into()],
        ..Default::default()
    };
    let diags = run_files(&[("main.typ", src)], &options).diagnostics;
    let found: Vec<_> = diags.iter().map(|d| (d.rule, pointed(src, d))).collect();
    assert_eq!(
        found,
        [
            ("page-size", "set page(paper: \"a5\", margin: 10mm)"),
            ("page-margins", "set page(paper: \"a5\", margin: 10mm)"),
            ("page-number", "set page(paper: \"a5\", margin: 10mm)"),
            ("font-size", "set text(size: 12pt, lang: \"uk\")"),
            ("font-family", "set text(size: 12pt, lang: \"uk\")"),
            // Заголовок: 1,4 em від 12 pt
            ("font-size", "set text(size: 12pt, lang: \"uk\")"),
            (
                "line-spacing",
                "set par(leading: 0.3em, first-line-indent: 5mm)"
            ),
            (
                "par-indent",
                "set par(leading: 0.3em, first-line-indent: 5mm)"
            ),
            (
                "par-indent",
                "set par(leading: 0.3em, first-line-indent: 5mm)"
            ),
        ],
        "{diags:#?}"
    );
    assert!(diags[0].msg.contains("148 мм × 210 мм"));
    assert!(diags[4].msg.contains("Libertinus Serif") && diags[4].msg.contains("типове"));
    assert!(diags[5].msg.contains("заголовки рівня 1") && diags[5].msg.contains("16,8"));
    assert!(diags[7].msg.contains("5 мм"));
    // Довжина без `all: true`: перший абзац після заголовка без відступу
    assert!(diags[8].msg.contains("перший абзац після заголовка"));
    assert!(fixed(src).contains("#set text(size: 14pt, lang: \"uk\")"));

    // Правильні налаштування, зокрема через змінні й em
    let src = "#let k = 1.25\n#set page(paper: \"a4\", margin: (top: 20mm, bottom: 20mm, left: 25mm, right: 10mm), numbering: \"1\", number-align: right + top)\n#set text(size: 14pt, font: (\"Times New Roman\", \"Liberation Serif\"))\n#set par(leading: 0.95em, first-line-indent: (amount: k * 1cm, all: true))\n#show heading: set text(size: 14pt)\n= Огляд\nТекст.\n";
    assert!(
        run_files(&[("main.typ", src)], &options)
            .diagnostics
            .is_empty()
    );

    // Фрагмент без жодних налаштувань: типові значення typst не перевіряються
    assert!(rules("= Огляд\nТекст.\n").is_empty());
    // Значення, якого з коду не видно, теж
    assert!(rules("#set text(size: base)\n= Огляд\nТекст.\n").contains(&"font-family"));
    assert!(!rules("#set text(size: base)\n= Огляд\nТекст.\n").contains(&"font-size"));
}

#[test]
fn settings_from_template_file() {
    let lib = "#let tpl(doc, size: 14pt) = {\n  set page(numbering: \"1\", number-align: top + right)\n  set text(size: size, font: \"Times New Roman\")\n  set par(leading: 1em, first-line-indent: (amount: 1.25cm, all: true))\n  show heading: set text(size: 14pt)\n  doc\n}\n";
    let check = |main: &str| {
        run_files(&[("main.typ", main), ("lib.typ", lib)], &Options::default()).diagnostics
    };
    assert!(check("#import \"lib.typ\": tpl\n#show: tpl\n= Огляд\nТекст.\n").is_empty());

    let main = "#import \"lib.typ\": tpl\n#show: tpl.with(size: 12pt)\n= Огляд\nТекст.\n";
    let diags = check(main);
    assert_eq!(diags.len(), 1, "{diags:#?}");
    assert_eq!(diags[0].rule, "font-size");
    assert_eq!(pointed(main, &diags[0]), "show: tpl.with(size: 12pt)");
    assert!(diags[0].msg.contains("шаблон, lib.typ"), "{}", diags[0].msg);
    // Налаштування документа після шаблону діє поверх нього
    let main = "#import \"lib.typ\": tpl\n#show: tpl.with(size: 12pt)\n#set text(size: 14pt)\n= Огляд\nТекст.\n";
    assert!(check(main).is_empty());

    // Обгортка з вмістом налаштовує лише його
    let main = "#import \"lib.typ\": tpl\n#show: tpl\n= Огляд\nТекст.\n#text(size: 10pt)[\n  Дрібний текст.\n\n  Ще абзац.\n]\n";
    let diags = check(main);
    assert_eq!(diags.len(), 1, "{diags:#?}");
    assert_eq!(diags[0].rule, "font-size");
    assert!(pointed(main, &diags[0]).starts_with("text(size: 10pt)["));

    // Шаблону з пакунка, якого немає, не простежити: нічого не відомо
    let main = "#import \"@preview/no-such-package:0.1.0\": tpl\n#show: tpl\n= Огляд\nТекст.\n";
    assert!(check(main).is_empty());
}

#[test]
fn appendices_by_what_the_wrapper_does() {
    let def = "#let appendices-style(body) = {\n  set heading(numbering: \"А.1\", supplement: [Додаток])\n  body\n}\n";
    // Назва довільна: додатки видно з налаштувань обгортки
    let src = format!(
        "{def}Додатки: @b[], потім @a[].\n#show: appendices-style\n= Перший <a>\n= Другий <b>\n"
    );
    assert_eq!(content_rules(&src), ["appendix-order"]);
    let src = format!(
        "{def}Додатки: @b[], потім @a[].\n#appendices-style[\n= Перший <a>\n= Другий <b>\n]\n"
    );
    assert_eq!(content_rules(&src), ["appendix-order"]);
    // Обгортка з назвою `appendices`, що додатків не налаштовує, ними не є
    let src = "#let appendices(body) = body\nДодатки: @b[], потім @a[].\n#show: appendices\n= Перший <a>\n= Другий <b>\n";
    assert!(content_rules(src).is_empty());
}

#[test]
fn template_from_package() {
    // SAFETY: змінну задають лише тести з пакунками, і завжди однаково
    unsafe { std::env::set_var("TYPST_PACKAGE_PATH", "/packages") };
    let manifest = "[package]\nname = \"tpl\"\nversion = \"0.1.0\"\nentrypoint = \"src/lib.typ\"\n";
    let lib =
        "#import \"/src/style.typ\"\n#let report(doc, ..args) = {\n  show: style.dstu\n  doc\n}\n";
    let style = "#let dstu(it) = {\n  set page(paper: \"a4\")\n  set text(size: 13pt, font: \"Times New Roman\")\n  set par(leading: 0.95em, first-line-indent: (amount: 1.25cm, all: true))\n  it\n}\n#let appendices(it) = {\n  set heading(supplement: [Додаток])\n  it\n}\n";
    let main = "#import \"@local/tpl:0.1.0\": *\n#show: report.with(..toml(\"doc.toml\"))\n= Огляд\nТекст.\n";
    let report = run_files(
        &[
            ("main.typ", main),
            ("doc.toml", "title = \"Звіт\"\n"),
            ("/packages/local/tpl/0.1.0/typst.toml", manifest),
            ("/packages/local/tpl/0.1.0/src/lib.typ", lib),
            ("/packages/local/tpl/0.1.0/src/style.typ", style),
        ],
        &Options::default(),
    );
    let found: Vec<_> = report
        .diagnostics
        .iter()
        .map(|d| (d.rule, pointed(main, d)))
        .collect();
    // Кегль основного тексту й заголовків (1,4 em від 13 pt), номери сторінок
    let at = "show: report.with(..toml(\"doc.toml\"))";
    assert_eq!(
        found,
        [("font-size", at), ("font-size", at), ("page-number", at)]
    );
    assert!(report.diagnostics[0].msg.contains("style.typ"));
    // Файли пакунка не стають файлами документа
    assert_eq!(report.files.len(), 1);
}

#[test]
fn wrappers_are_recognized_by_what_they_build() {
    // Обгортка з міткою, обчисленою зі шляху: рисунок і його мітка видно
    let lib = r#"#let img(path, caption, ..sink) = {
  let name = path.split(".").first().split("/").last().replace(" ", "_")
  [#figure(image(path, ..sink.named()), caption: caption) #label(name)]
}
#let plot(data, title) = figure(rect(), caption: title)
"#;
    let main = "#import \"lib.typ\": *\nНа @scheme[рисунку] схема.\n#img(\"figures/scheme.png\", \"Схема\")\n#img(\"figures/other.png\", \"інший\")\n#plot((1, 2), [Графік])\n";
    let report = run_files(&[("main.typ", main), ("lib.typ", lib)], &Options::default());
    let found: Vec<_> = report
        .diagnostics
        .iter()
        .map(|d| (d.rule, pointed(main, d)))
        .collect();
    assert_eq!(
        found,
        [
            ("caption-case", "інший"),
            ("figure-unreferenced", "інший"),
            ("figure-unreferenced", "Графік"),
        ],
        "{:#?}",
        report.diagnostics
    );

    // Без визначення обгортки нічого не вгадується: текст перевіряється, але
    // ні рисунка, ні нумерації заголовків не видно
    let src = "#show: style.dstu\n= Вступ\n#fig(path(\"a.png\"), [Обгортка], label: <x>)\n";
    assert!(rules(src).is_empty(), "{:?}", rules(src));
}

#[test]
fn appendices_by_numbering_and_heading_show() {
    // Нумерація функцією, що дає першому заголовку «А»
    let src = "#let letters = \"АБВГ\".clusters()\n#let app(body) = {\n  set heading(numbering: (i, ..n) => letters.at(i - 1))\n  body\n}\nДодатки: @b[], потім @a[].\n#show: app\n= Перший <a>\n= Другий <b>\n";
    assert_eq!(content_rules(src), ["appendix-order"]);
    // `show heading`, що друкує «ДОДАТОК»
    let src = "#let app(body) = {\n  set heading(numbering: \"1\")\n  show heading: it => [ДОДАТОК #it.body]\n  body\n}\nДодатки: @b[], потім @a[].\n#show: app\n= Перший <a>\n= Другий <b>\n";
    assert_eq!(content_rules(src), ["appendix-order"]);
}

/// Зауваження до оформлення елементів (правило, текст, на який вказує).
fn looks_of(src: &str, enable: &[&str]) -> Vec<(&'static str, String)> {
    let options = Options {
        enable: enable.iter().map(|s| (*s).to_owned()).collect(),
        ..Default::default()
    };
    run_files(&[("main.typ", src)], &options)
        .diagnostics
        .iter()
        .map(|d| (d.rule, src[d.range.clone()].to_owned()))
        .collect()
}

#[test]
fn appearance_from_template() {
    // Мінішаблон оформлює все за стандартом
    let body = "#heading(numbering: none)[Вступ]\nТекст вступу.\n\n= Розділ\nТекст розділу:\n- перший;\n- другий.\n\n+ один;\n+ два.\n\nПозиція 2) головна.\n\n== Підрозділ\nТекст#footnote[Виноска.] на @f[рисунку].\n#figure(rect(), caption: [Схема]) <f>\n== Другий\nТекст.\n";
    let ok = format!("{DSTU}{body}");
    assert!(
        looks_of(&ok, &["list-indent", "caption-position"]).is_empty(),
        "{:?}",
        looks_of(&ok, &[])
    );

    // Кожне налаштування документа після шаблону — одне зауваження на ньому
    let cases: &[(&str, &str)] = &[
        ("#set list(marker: [•])", "list-marker"),
        ("#set enum(numbering: \"1.\")", "list-marker"),
        ("#set heading(numbering: \"1.1.\")", "heading-number-format"),
        ("#set heading(numbering: \"I.1\")", "heading-number-format"),
        ("#set figure.caption(separator: [: ])", "caption-separator"),
        ("#set figure(supplement: [Рис.])", "caption-supplement"),
        ("#set figure(numbering: \"1.\")", "object-numbering"),
        ("#set footnote(numbering: \"1\")", "footnote-numbering"),
        (
            "#set footnote.entry(separator: line(length: 60mm))",
            "footnote-separator",
        ),
        ("#show footnote.entry: set text(size: 14pt)", "font-size"),
        ("#set page(number-align: center + bottom)", "page-number"),
        ("#set page(numbering: none)", "page-number"),
        ("#show heading: set text(hyphenate: true)", "heading-hyphen"),
        ("#show heading: set text(font: \"Arial\")", "font-family"),
        (
            "#show heading: set text(weight: \"regular\")",
            "heading-style",
        ),
        ("#set text(style: \"italic\")", "font-style"),
    ];
    for (set, rule) in cases {
        let src = format!("{DSTU}{set}\n{body}");
        let found = looks_of(&src, &["font-style"]);
        assert!(
            found.len() == 1 && found[0].0 == *rule && set.contains(found[0].1.as_str()),
            "{set}: {found:?}"
        );
    }
}

#[test]
fn appearance_from_typst_defaults() {
    // Документ з налаштуваннями тексту, але без оформлення елементів
    let src = "#set text(lang: \"uk\", size: 14pt, font: \"Times New Roman\")\n#set par(leading: 0.95em, first-line-indent: (amount: 1.25cm, all: true), justify: true)\n#set heading(numbering: \"1.1\")\nТитульний аркуш.\n= Розділ\nТекст розділу:\n- перший;\n- другий.\n\n== Підрозділ\nТекст#footnote[Виноска.] на @f[рисунку] і в @t[таблиці].\n#figure(rect(), caption: [Схема]) <f>\n#figure(table[x], caption: [Дані]) <t>\n== Другий\nТекст.\n#heading(numbering: none)[Висновки]\nТекст.\n";
    let mut found: Vec<&str> = looks_of(src, &["caption-position", "list-indent"])
        .into_iter()
        .map(|(r, _)| r)
        .collect();
    found.sort_unstable();
    found.dedup();
    assert_eq!(
        found,
        [
            "caption-position",
            "caption-separator",
            "font-size",
            "footnote-numbering",
            "footnote-separator",
            "heading-hyphen",
            "heading-indent",
            "heading-style",
            "list-indent",
            "list-marker",
            "new-page",
            "page-number",
        ]
    );
    let diags = run(src);
    let msg = |rule: &str, part: &str| diags.iter().any(|d| d.rule == rule && d.msg.contains(part));
    assert!(msg("caption-separator", "двокрапка «:»"));
    // Типові береги А4 — по 25 мм: 30 % від 160 мм
    assert!(msg("footnote-separator", "48 мм"));
    assert!(msg("font-size", "текст виносок набрано кеглем 11,9"));
    assert!(msg("font-size", "заголовки рівня 1 набрано кеглем 19,6"));
    assert!(msg("new-page", "«Висновки»"));
    assert!(msg("page-number", "не пронумеровано"));
}

#[test]
fn appearance_of_headings_from_show_rules() {
    let tpl = |show: &str| {
        format!(
            "#set text(lang: \"uk\", size: 14pt, font: \"Times New Roman\", hyphenate: false)\n#set par(leading: 0.95em, first-line-indent: (amount: 1.25cm, all: true))\n#set page(numbering: \"1\", number-align: top + right)\n#set heading(numbering: \"1.1\")\n{show}\n= Розділ\nТекст.\n== Підрозділ\nТекст.\n== Другий\nТекст.\n"
        )
    };
    // Як у шаблонах: великі літери, відступ підзаголовків з `h(…)` чи `pad`
    for show in [
        "#show heading: it => { set text(size: 14pt); if it.level == 1 { pagebreak(weak: true); upper(it) } else { block[#h(1.25cm)#counter(heading).display(it.numbering) #it.body] } }",
        "#show heading.where(level: 1): it => upper(it)\n#show heading: set text(size: 14pt)\n#show heading.where(level: 2): it => pad(left: 1.25cm, it)",
    ] {
        assert!(
            rules(&tpl(show)).is_empty(),
            "{show}: {:?}",
            rules(&tpl(show))
        );
    }
    // Не великими літерами, посередині
    let src = tpl("#show heading: set text(size: 14pt)\n#show heading: it => align(center, it)");
    let found: Vec<_> = looks_of(&src, &[]).into_iter().map(|(r, _)| r).collect();
    assert_eq!(found, ["heading-style", "heading-indent"]);
    // Show-функція, якої не обчислити: оформлення не видно, тож мовчимо
    let src =
        tpl("#show heading: it => context { if here().page() > 1 { upper(it) } else { it } }");
    assert!(rules(&src).is_empty(), "{:?}", rules(&src));
}

#[test]
fn appearance_of_formulas() {
    let base = format!(
        "{DSTU}= Розділ\nЗа @a[формулою] маємо:\n$ a = b $ <a>\n#par(first-line-indent: 0pt)[де $a$ -- величина.]\n"
    );
    assert!(rules(&base).is_empty(), "{:?}", rules(&base));
    let with = |set: &str| {
        format!(
            "{DSTU}{set}\n= Розділ\nЗа @a[формулою] маємо:\n$ a = b $ <a>\n$ c = d $ <c>\nде $a$ -- величина.\n"
        )
    };
    let found: Vec<_> = looks_of(&with("#set math.equation(numbering: \"1\")"), &[])
        .into_iter()
        .collect();
    assert_eq!(
        found,
        [
            (
                "formula-numbering",
                "set math.equation(numbering: \"1\")".to_owned()
            ),
            ("formula-comma", "$".to_owned()),
            ("formula-numbering", "$ c = d $".to_owned()),
            ("formula-where-indent", "де $a$ -- величина.".to_owned()),
        ]
    );
    let found: Vec<_> = looks_of(
        &with("#set math.equation(numbering: \"(1)\")\n#show math.equation: set align(left)"),
        &[],
    )
    .into_iter()
    .map(|(r, _)| r)
    .collect();
    assert!(found.contains(&"formula-center"), "{found:?}");
    // `#par(first-line-indent: 0pt)[де …]` і `\` перед «де» статика не вважає відступом
    let src = format!(
        "{DSTU}= Розділ\nЗа формулою:\n$ a = b $\n#par(first-line-indent: 0pt)[де $a$ -- величина.]\n"
    );
    assert!(!rules(&src).contains(&"formula-where-indent"));
}

/// Що лише недавно бачила тільки повна перевірка: `context`, лічильники,
/// show-функції, що друкують частини елемента, колонтитули, текст у рядку.
#[test]
fn appearance_beyond_set_rules() {
    let tpl = "#set text(lang: \"uk\", size: 14pt, font: \"Times New Roman\", hyphenate: false)\n#set par(leading: 0.95em, first-line-indent: (amount: 1.25cm, all: true))\n#set page(numbering: \"1\", number-align: top + right)\n#set heading(numbering: \"1.1\")\n#show heading: set text(size: 14pt)\n#show heading.where(level: 1): it => { pagebreak(weak: true); upper(it) }\n#set figure.caption(separator: [ -- ])\n";
    let fig = |n: &str| {
        format!("На @f{n}[рисунку] схема.\n#figure(rect(), caption: [Схема {n}]) <f{n}>\n")
    };
    let chapters = |reset: &str| {
        format!(
            "{tpl}#set figure(numbering: i => context numbering(\"1.1\", counter(heading).get().at(0), i))\n{reset}= Перший\nТекст.\n{}{}= Другий\nТекст.\n{}",
            fig("1"),
            fig("2"),
            fig("3")
        )
    };
    // Номер у межах розділу без скидання лічильника: «2.3» замість «2.1»
    let found = looks_of(&chapters(""), &[]);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, "object-numbering");
    assert!(
        run(&chapters(""))
            .iter()
            .any(|d| d.msg.contains("«2.3» не за порядком")
                && d.help.as_deref() == Some("За порядком має бути «2.1»"))
    );
    let reset = "#show heading.where(level: 1): it => { counter(figure.where(kind: image)).update(0); it }\n";
    assert!(
        looks_of(&chapters(reset), &[]).is_empty(),
        "{:?}",
        looks_of(&chapters(reset), &[])
    );

    // Show-правило прибирає номер у формул без посилань
    let eq = format!(
        "{tpl}#set math.equation(numbering: \"(1)\")\n#show math.equation.where(block: true): it => {{ if it.numbering == none {{ return it }}; context {{ if it.has(\"label\") and query(ref.where(target: it.label)).len() > 0 {{ return it }}; math.equation(it.body, block: true, numbering: none) }} }}\n= Розділ\nЗа @a[формулою] маємо:\n$ a = b, $ <a>\n$ c = d $\n#par(first-line-indent: 0pt)[де $a$ -- величина.]\n"
    );
    assert!(looks_of(&eq, &[]).is_empty(), "{:?}", looks_of(&eq, &[]));

    // Фігуру друкує show-функція: назва таблиці над нею, з абзацного відступу
    let table = |order: &str| {
        format!(
            "{tpl}#show figure.where(kind: table): it => block({order})\n= Розділ\nУ @t[таблиці] дані.\n#figure(table[x], caption: [Дані]) <t>\n"
        )
    };
    let above = table("[#h(1.25cm)#it.caption\\ #it.body]");
    assert!(
        looks_of(&above, &["caption-position"]).is_empty(),
        "{:?}",
        looks_of(&above, &["caption-position"])
    );
    let below = looks_of(&table("[#it.body #it.caption]"), &["caption-position"]);
    assert!(
        below.iter().any(|(r, _)| *r == "caption-position"),
        "{below:?}"
    );

    // Вкладений перелік: `set` у show-функції зовнішнього
    let lists = format!(
        "{tpl}#set enum(numbering: \"а)\")\n#show enum: it => {{ set enum(numbering: \"1.\"); it }}\n= Розділ\nПерелік:\n+ перший:\n  + вкладений;\n  + другий;\n+ останній.\n"
    );
    let found = looks_of(&lists, &[]);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        run(&lists)
            .iter()
            .any(|d| d.rule == "list-marker" && d.msg.contains("«1.»"))
    );

    // Номер сторінки у власному колонтитулі
    let header = |align: &str| {
        format!(
            "{tpl}#set page(header: align({align}, context counter(page).display()))\n= Розділ\nТекст.\n"
        )
    };
    assert!(
        looks_of(&header("right"), &[]).is_empty(),
        "{:?}",
        looks_of(&header("right"), &[])
    );
    let found = looks_of(&header("center"), &[]);
    assert!(
        found
            .iter()
            .any(|(r, t)| *r == "page-number" && t.contains("header")),
        "{found:?}"
    );

    // Лічильник виносок без скидання на кожній сторінці
    let notes = format!(
        "{tpl}#set footnote(numbering: \"1)\")\n#set footnote.entry(separator: line(length: 35mm))\n#show footnote.entry: set text(size: 12pt)\n= Розділ\nТекст#footnote[Перша.] і ще#footnote[Друга.].\n"
    );
    let found: Vec<_> = looks_of(&notes, &[]).into_iter().map(|(r, _)| r).collect();
    assert_eq!(found, ["footnote-numbering"]);
}

#[test]
fn inline_text_and_notes() {
    let src = format!(
        "{DSTU}= Розділ\nЗвичайний абзац, де #text(size: 10pt)[дрібний] текст і #text(font: \"Arial\")[інший] шрифт.\n\nПримітка. Примітка кеглем 14.\n"
    );
    let found = looks_of(&src, &[]);
    assert_eq!(
        found,
        [
            ("font-size", "text(size: 10pt)[дрібний]".to_owned()),
            ("font-family", "text(font: \"Arial\")[інший]".to_owned()),
            ("note-size", "Примітка. Примітка кеглем 14.".to_owned()),
        ]
    );
    let src = format!(
        "{DSTU}= Розділ\nТекст.\n#par(leading: 0.5em)[#text(size: 12pt)[Примітка. Правильна примітка.]]\n"
    );
    assert!(looks_of(&src, &[]).is_empty(), "{:?}", looks_of(&src, &[]));
    // Курсив — лише з `enable: font-style`
    let src = format!("{DSTU}= Розділ\nТекст з _курсивом_ у реченні.\n");
    assert!(looks_of(&src, &[]).is_empty());
    assert_eq!(
        looks_of(&src, &["font-style"]),
        [("font-style", "_курсивом_".to_owned())]
    );
    // `\` перед «де»: порожній рядок
    let src = format!("{DSTU}= Розділ\nЗа формулою:\n$ a = b $\\\nде $a$ -- величина.\n");
    let diags = run(&src);
    assert!(
        diags
            .iter()
            .any(|d| d.rule == "formula-where-indent" && d.msg.contains("порожній рядок")),
        "{diags:?}"
    );
}

#[test]
fn loop_over_known_array() {
    // Цикл за відомим масивом друкує тіло для кожного значення
    let src = "#for f in (\"перший\", \"другий\") [\n  == Пункт #f.\n  Текст.\n]\n";
    let diags = run(src);
    let periods: Vec<_> = diags
        .iter()
        .filter(|d| d.rule == "heading-period")
        .map(|d| d.msg.clone())
        .collect();
    assert_eq!(periods.len(), 2, "{diags:?}");
    assert!(periods[0].contains("«Пункт перший.»"), "{periods:?}");
    assert!(periods[1].contains("«Пункт другий.»"), "{periods:?}");
    // Масив, якого не видно: тіло один раз, змінної немає
    let src = "#for f in sys.inputs.values() [\n  == Пункт #f.\n]\n";
    let n = rules(src)
        .iter()
        .filter(|r| **r == "heading-period")
        .count();
    assert_eq!(n, 1);
}

#[test]
fn known_variable_in_text() {
    let src = "#let name = \"Вступ\"\n= #name.\n";
    let diags = run(src);
    let d = diags
        .iter()
        .find(|d| d.rule == "heading-period")
        .expect("heading-period");
    assert!(d.msg.contains("«Вступ.»"), "{}", d.msg);
}

#[test]
fn content_in_let_is_document_text() {
    // Вміст з `#let`, який документ ставить сам, перевіряється там, де написаний
    let body = "#let body = [\n= Розділ.\nПерелік:\n- Перший.\n]\n";
    let same = format!("{body}#body\n");
    let found = |files: &[(&str, &str)]| -> Vec<(String, &'static str)> {
        let mut out: Vec<_> = run_files(files, &Options::default())
            .diagnostics
            .into_iter()
            .filter(|d| matches!(d.rule, "heading-period" | "list-case"))
            .map(|d| {
                (
                    d.path.file_name().unwrap().to_string_lossy().into_owned(),
                    d.rule,
                )
            })
            .collect();
        out.sort();
        out
    };
    let expect = |file: &str| {
        vec![
            (file.to_owned(), "heading-period"),
            (file.to_owned(), "list-case"),
        ]
    };
    assert_eq!(found(&[("main.typ", &same)]), expect("main.typ"));
    // Імпорт назви й доступ через модуль: зауваження у файлі, де вміст набрано
    let main = "#import \"parts.typ\": body\n#body\n";
    assert_eq!(
        found(&[("main.typ", main), ("parts.typ", body)]),
        expect("parts.typ")
    );
    let main = "#import \"parts.typ\"\n#parts.body\n";
    assert_eq!(
        found(&[("main.typ", main), ("parts.typ", body)]),
        expect("parts.typ")
    );
    // Вміст, переданий шаблону, який його друкує
    let tpl = "#let tpl(body: none, doc) = { doc; body }\n";
    let main = "#import \"t.typ\": tpl\n#import \"parts.typ\": body\n#show: tpl.with(body: body)\nТекст.\n";
    assert_eq!(
        found(&[("main.typ", main), ("t.typ", tpl), ("parts.typ", body)]),
        expect("parts.typ")
    );
    // Невикористаний вміст не друкується
    assert!(found(&[("main.typ", body)]).is_empty());
}

#[test]
fn loops_over_range_enumerate_and_dict() {
    let captions = |src: &str| -> Vec<String> {
        run(src)
            .into_iter()
            .filter(|d| d.rule == "figure-unreferenced")
            .map(|d| d.msg)
            .collect()
    };
    // `range`, число в тексті й обчислений рядок підпису
    let src = "= Розділ\n#for i in range(1, 3) {\n  figure(rect(), caption: [Слайд #i])\n  figure(rect(), caption: \"Аркуш \" + str(i))\n}\n";
    let c = captions(src);
    for want in ["«Слайд 1»", "«Слайд 2»", "«Аркуш 1»", "«Аркуш 2»"] {
        assert!(c.iter().any(|m| m.contains(want)), "{want}: {c:?}");
    }
    // `enumerate` з деструктуризацією
    let src = "= Розділ\n#for (n, name) in (\"А\", \"Б\").enumerate(start: 1) [\n  #figure(rect(), caption: [#n. #name])\n]\n";
    let c = captions(src);
    assert!(c.iter().any(|m| m.contains("«1. А»")), "{c:?}");
    assert!(c.iter().any(|m| m.contains("«2. Б»")), "{c:?}");
    // Словник: пари (ключ, значення)
    let src = "= Розділ\n#let parts = (вхід: 1, вихід: 2)\n#for (k, v) in parts [\n  #figure(rect(), caption: [#k #v])\n]\n";
    let c = captions(src);
    assert!(c.iter().any(|m| m.contains("«вхід 1»")), "{c:?}");
    assert!(c.iter().any(|m| m.contains("«вихід 2»")), "{c:?}");
}

#[test]
fn let_destructuring_and_calc() {
    let src = "#let (a, .., z) = (\"Перший\", \"x\", \"Останній\")\n#let (name: n) = (name: \"Ім'я\")\n= #a.\n= #z.\n= #n.\n= Пункт #calc.max(2, 7).\n";
    let periods: Vec<String> = run(src)
        .into_iter()
        .filter(|d| d.rule == "heading-period")
        .map(|d| d.msg)
        .collect();
    for want in ["«Перший.»", "«Останній.»", "«Ім'я.»", "«Пункт 7.»"]
    {
        assert!(
            periods.iter().any(|m| m.contains(want)),
            "{want}: {periods:?}"
        );
    }
}

#[test]
fn appendix_status() {
    // Додаток упізнано за нумерацією літерою: лише припущення
    let diags = run("#set heading(numbering: \"А.1\")\n= додаток з малої\n");
    let case: Vec<_> = diags.iter().filter(|d| d.rule == "appendix-case").collect();
    assert_eq!(case.len(), 1, "{diags:?}");
    assert!(case[0].assumed.is_some());
    // Нумерацію видно, ознак додатка немає: що це не додаток, певно
    let diags = run("#set heading(numbering: \"I.1\")\n= Огляд\n== Частина\n");
    let digits: Vec<_> = diags
        .iter()
        .filter(|d| d.rule == "heading-number-format")
        .collect();
    assert!(!digits.is_empty(), "{diags:?}");
    assert!(digits.iter().all(|d| d.assumed.is_none()), "{digits:?}");
}

#[test]
fn structural_variant_everywhere() {
    // «Література» лише вгадано як перелік джерел: і нова сторінка, і стиль
    // заголовка спираються на це припущення
    let src = "#show heading: set text(weight: 400)\n= Вступ\nТекст.\n= Література\nТекст.\n";
    let diags = run(src);
    let new_page: Vec<_> = diags.iter().filter(|d| d.rule == "new-page").collect();
    assert_eq!(new_page.len(), 1, "{diags:?}");
    assert!(new_page[0].assumed.is_some());
    // Стиль заголовків зведено з «Вступ» (певного) і «Література»: зауваження
    // певне, бо стосується й певного заголовка
    let style: Vec<_> = diags.iter().filter(|d| d.rule == "heading-style").collect();
    assert!(!style.is_empty(), "{diags:?}");
    assert!(style.iter().all(|d| d.assumed.is_none()), "{style:?}");
}

#[test]
fn builtin_show_set_under_show_function() {
    // Show-функція друкує `it.body` сама, але вбудоване оформлення typst
    // (напівжирне накреслення заголовка) однаково діє
    let src = "#set text(size: 14pt, lang: \"uk\")\n#show heading: it => block(it.body)\n= Вступ\nТекст.\n";
    let diags = run(src);
    assert!(
        !diags.iter().any(|d| d.msg.contains("не напівжирним")),
        "{diags:?}"
    );
}
