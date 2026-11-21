//! Звірка з typst-версією лінтера: `tests/lint.typ` репозиторію пакунка nure
//! позначає кожне порушення `#expect("правило")`. Статичний лінтер має знайти
//! ті самі порушення, крім правил, яким потрібна розкладка на сторінки.

use std::path::PathBuf;

use dsty::lint::{self, Options};

const PAGE_RULES: &[&str] = &[
    "abstract-length",
    "figure-far-from-ref",
    "table-no-header",
    "footnote-count",
];

#[test]
fn matches_typst_linter_expectations() {
    let repo = std::env::var_os("NURE_TEMPLATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../typst_nure_template")
        });
    let main = repo.join("tests/lint.typ");
    let Ok(source) = std::fs::read_to_string(&main) else {
        eprintln!("пропущено: немає {}", main.display());
        return;
    };
    let mut want: Vec<String> = source
        .lines()
        .filter_map(|line| line.trim().strip_prefix("#expect(\""))
        .map(|rest| rest[..rest.find('"').unwrap()].to_owned())
        .filter(|r| !PAGE_RULES.contains(&r.as_str()))
        .collect();
    want.sort();

    let options = Options {
        enable: vec![
            "unit-space".into(),
            "unit-range".into(),
            "unit-dimensions".into(),
        ],
        ..Default::default()
    };
    let load = |p: &std::path::Path| std::fs::read_to_string(p).ok();
    let report = lint::check(&repo, &main, &load, &options).unwrap();
    // Правила, яких typst-версія не має, тут не звіряються
    let known = typst_rules(&repo);
    let mut got: Vec<String> = report
        .diagnostics
        .iter()
        .map(|d| d.rule.to_owned())
        .filter(|r| known.is_empty() || known.contains(r))
        .collect();
    got.sort();
    assert_eq!(got, want);
}

/// Коди правил зі словника `rules` у `src/lint.typ` пакунка.
fn typst_rules(repo: &std::path::Path) -> Vec<String> {
    let Ok(src) = std::fs::read_to_string(repo.join("src/lint.typ")) else {
        return Vec::new();
    };
    src.lines()
        .skip_while(|l| !l.starts_with("#let rules = ("))
        .take_while(|l| *l != ")")
        .filter_map(|l| {
            let rest = l.strip_prefix("  \"")?;
            let end = rest.find("\": (")?;
            Some(rest[..end].to_owned())
        })
        .collect()
}
