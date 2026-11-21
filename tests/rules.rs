//! Правила на маленьких документах у пам'яті: кожне правило спрацьовує на
//! порушенні й мовчить на правильному коді.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use dsty::lint::{self, Diagnostic, Options, Report};

/// Перевіряє проєкт з файлів у пам'яті; main -- перший файл.
fn run_files(files: &[(&str, &str)], options: &Options) -> Report {
    let root = PathBuf::from("/project");
    let map: HashMap<PathBuf, String> = files
        .iter()
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

const DSTU: &str = "#show: style.dstu\n";

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
    // Без шаблону й з явним вимкненням -- ні
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
    // Перше посилання -- пов'язане місце
    assert_eq!(diags[2].related[0].1.line, 6);
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
    let src = "Умови:\n+ перша;\n+ друга.\n+ третя;\n\nВкладений:\n- зовнішній:\n  + внутрішній;\n  + останній;\n- кінець.\n";
    let diags = run(src);
    let found: Vec<_> = diags.iter().map(|d| (d.rule, pointed(src, d))).collect();
    assert_eq!(found, [("list-punct", "друга."), ("list-punct", "третя;")]);
    assert!(diags[0].msg.contains("посеред переліку"));
    let out = fixed(src);
    assert!(out.contains("+ друга;\n+ третя.\n"), "{out}");
    assert!(out.contains("+ останній;\n"), "{out}");

    // Дві помилки в одній позиції -- два зауваження; кілька речень -- теж «;»
    let src = "Кроки:\n+ Відкрито вікно.\n+ натиснуто кнопку. Далі X.\n+ кінець.\n";
    assert_eq!(rules(src), ["list-case", "list-punct", "list-punct"]);
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
    assert!(fixed(src).contains("де $s$ -- шлях."));
}

#[test]
fn keywords() {
    let src = "#abstract(w, keywords: (\"один\", \"два\"))[Текст.]\n";
    assert_eq!(rules(src), ["keywords-count"]);
    // Рецепт: `abstract: (keywords: …)` у `show`
    let src = "#show: coursework.with(abstract: (keywords: (\"a\",), text: []))\n";
    assert_eq!(rules(src), ["keywords-count"]);
    let src = "#abstract(w, keywords: (\"a\", \"b\", \"c\", \"d\", \"e\"))[Текст.]\n";
    assert!(rules(src).is_empty());
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
    let main = "#show: coursework.with(bib-path: path(\"b.yml\"), appendices: include \"app.typ\")\n= Вступ\nКод у @code[].\n";
    let app = "= Код програми <code>\nТекст.\n";
    let report = run_files(&[("main.typ", main), ("app.typ", app)], &Options::default());
    let found: Vec<_> = report.diagnostics.iter().map(|d| d.rule).collect();
    // `= Вступ` нумерується (шаблон), а додаток -- після переліку джерел
    assert_eq!(found, ["heading-numbered"]);
    assert_eq!(report.files.len(), 2);
}

#[test]
fn dynamic_labels_are_not_guessed() {
    let src = "#show: style.appendices\n#for f in (\"a\", \"b\") [\n  #heading[Файл #f] #label(str(\"app_\" + f))\n]\n#heading[Файл] #label(\"lit\")\n";
    assert_eq!(rules(src), ["appendix-unreferenced"]);
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
    // `--` у коді -- не той самий текст, тож без виправлення
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
    // Велика літера й крапка: спершу лише list-case, після нього -- крапка
    let src = "Визначити:\n1. Швидкість потоку.\n2. Обрати модуляцію.\n3. розробити схему.\n";
    let root = PathBuf::from("/project");
    let main = root.join("main.typ");
    let load = |p: &Path| (p == main).then(|| src.to_owned());
    let fixed = lint::fix_all(&root, &main, &load, &Options::default(), None).unwrap();
    assert_eq!(
        fixed.files[&main],
        "Визначити:\n1. швидкість потоку;\n2. обрати модуляцію;\n3. розробити схему.\n"
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
    assert_eq!(fixed(src), "= Висновки\n== Перелік джерел посилання\n");
    // Великими літерами -- великими й виправлення
    assert_eq!(
        fixed("= СПИСОК ЛІТЕРАТУРИ\n"),
        "= ПЕРЕЛІК ДЖЕРЕЛ ПОСИЛАННЯ\n"
    );
    // Записи переліку джерел під неканонічною назвою -- не перелік за 7.7
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
fn note_numbering() {
    // Одна примітка з номером
    let src = "Текст.\n\nПримітка 1. Пояснення.\n";
    assert_eq!(rules(src), ["note-numbering"]);
    assert_eq!(fixed(src), "Текст.\n\nПримітка. Пояснення.\n");
    // Кілька поспіль: без номера й з неправильним номером
    let src = "Примітка. Перша.\n\nПримітка 3. Друга.\n\nПримітка 3. Третя.\n";
    assert_eq!(rules(src), ["note-numbering", "note-numbering"]);
    assert_eq!(
        fixed(src),
        "Примітка 1. Перша.\n\nПримітка 2. Друга.\n\nПримітка 3. Третя.\n"
    );
    // Окремі примітки в різних місцях -- кожна одна
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
    let refs: Vec<&Diagnostic> = diags.iter().collect();
    assert!(
        lint::apply_fixes(src, &refs)
            .0
            .contains("АЛГОРИТМ, БАЗА ДАНИХ, ЗАТРИМКА, МЕРЕЖА, ЯКІСТЬ.\n")
    );
    // Кількість -- і для набраних вручну
    assert_eq!(
        rules("= Реферат\nАЛГОРИТМ, БАЗА ДАНИХ, МЕРЕЖА.\n"),
        ["keywords-count"]
    );
    // У масиві шаблону порядок не перевіряється: шаблон сортує сам
    let src = "#abstract(w, keywords: (\"Я\", \"Б\", \"В\", \"Г\", \"Д\"))[Текст.]\n";
    assert!(run_with(src, &["keywords-order"]).is_empty());
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
    // Код, формула й латиниця на початку -- назви, а не регістр речення
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
    // `#heading("ІСПИТ")` -- заголовок з рядка, а не порожній
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
    // Власний підпис з малої літери чи власне show-правило ref -- не перевіряється
    let src = "#set figure(supplement: [рис.])\nНа @f.\n#figure(rect(), caption: [Схема]) <f>\n";
    assert!(rules(src).is_empty());
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
    // Інтервал у формулі й індекс у коді -- не посилання
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
    // Позначка -- на закривальному `$`, наступна формула -- пов'язане місце
    assert_eq!(pointed(src, &diags[0]), "$");
    assert_eq!(diags[0].related[0].3, "наступна формула");
    assert_eq!(diags[0].context, Some((1, 2)));
    // Текст між формулами -- окремі речення
    assert!(rules("$ a = b $\nабо\n$ c = d $\n").is_empty());
    // Код, перелік термінів, таблиця між формулами -- теж не «поспіль»
    for between in [
        "```\ncode\n```",
        "/ Термін: опис",
        "#table([a])",
        "#image(\"a.png\")",
    ] {
        let src = format!("$ a = b $\n{between}\n$ c = d $\n");
        assert!(rules(&src).is_empty(), "{between}: {:?}", run(&src));
    }
    // Проміжок чи розрив сторінки -- не текст
    assert_eq!(rules("$ a = b $\n#v(1em)\n$ c = d $\n"), ["formula-comma"]);
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
    // Помилка в назві -- з підказкою й виправленням
    let src = "// dstu-ignore-next-line: list-cse\n- Велика.\n";
    let diags = run(src);
    let found: Vec<_> = diags.iter().map(|d| (d.rule, pointed(src, d))).collect();
    assert_eq!(
        found,
        [("ignore-unknown", "list-cse"), ("list-case", "Велика.")]
    );
    assert!(diags[0].msg.contains("можливо, «list-case»"));
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
    assert!(rules("// dstu-ignore-file: page-size, heading-numbered\nТекст.\n").is_empty());
    // Без переліку (усі правила) -- теж
    assert!(rules("Текст. // dstu-ignore\n").is_empty());
}
