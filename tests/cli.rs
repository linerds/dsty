//! `dsty check` і `dsty rules`.

use std::path::PathBuf;
use std::process::Command;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dsty-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("chapters")).unwrap();
    dir.canonicalize().unwrap()
}

fn dsty() -> Command {
    Command::new(env!("CARGO_BIN_EXE_dsty"))
}

#[test]
fn check_and_fix() {
    let dir = temp_dir("cli");
    std::fs::write(
        dir.join("main.typ"),
        "#show: style.dstu\n#nheading[Вступ]\n#include \"chapters/one.typ\"\n#include \"chapters/missing.typ\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("chapters/one.typ"),
        "== Підрозділ.\nПерелік:\n- Перший\n- другий.\n",
    )
    .unwrap();

    let output = dsty()
        .current_dir(&dir)
        .args(["check", "main.typ"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        [
            "chapters/one.typ:1:4: [heading-period] крапка в кінці заголовка «Підрозділ.» (п. 7.1.20, 7.1.21)",
            "chapters/one.typ:3:3: [list-case] позиція переліку з великої літери: «Перший» (п. 7.7.4)",
            "chapters/one.typ:3:3: [list-punct] позиція переліку без «;» або «.» в кінці: «Перший» (п. 7.7.2)",
        ]
    );
    assert!(
        stderr.contains("немає файлу chapters/missing.typ"),
        "{stderr}"
    );

    let output = dsty()
        .current_dir(&dir)
        .args(["check", "main.typ", "--fix", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let items: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(items, serde_json::json!([]));
    assert_eq!(
        std::fs::read_to_string(dir.join("chapters/one.typ")).unwrap(),
        "== Підрозділ\nПерелік:\n- перший;\n- другий.\n"
    );
}

/// `--fix` через символьне посилання править файл, на який воно вказує, і не
/// лишає тимчасових файлів.
#[cfg(unix)]
#[test]
fn fix_keeps_symlinks() {
    let dir = temp_dir("cli-link");
    std::fs::write(
        dir.join("chapters/real.typ"),
        "Перелік:\n- Перший;\n- другий.\n",
    )
    .unwrap();
    std::os::unix::fs::symlink("chapters/real.typ", dir.join("main.typ")).unwrap();
    let output = dsty()
        .current_dir(&dir)
        .args(["check", "main.typ", "--fix"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(dir.join("main.typ").is_symlink());
    assert_eq!(
        std::fs::read_to_string(dir.join("chapters/real.typ")).unwrap(),
        "Перелік:\n- перший;\n- другий.\n"
    );
    let leftovers: Vec<_> = std::fs::read_dir(dir.join("chapters"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(leftovers, ["real.typ"]);
}

#[test]
fn unknown_rule_is_an_error() {
    let dir = temp_dir("cli-bad");
    std::fs::write(dir.join("main.typ"), "Текст.\n").unwrap();
    let output = dsty()
        .current_dir(&dir)
        .args(["check", "main.typ", "--ignore", "nope"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("невідоме правило «nope»"));
}

#[test]
fn lists_rules() {
    let output = dsty().arg("rules").output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.lines().any(|l| l.starts_with("list-punct")));
    assert!(
        stdout
            .lines()
            .any(|l| l.starts_with("unit-space ") && l.contains(" static enable "))
    );
}

#[test]
fn root_from_absolute_imports() {
    // `/lib.typ` лежить у теці вище, ніж main: корінем стає вона
    let dir = temp_dir("root");
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::create_dir_all(dir.join("work")).unwrap();
    std::fs::write(dir.join("lib.typ"), "#let x = 1\n").unwrap();
    std::fs::write(dir.join("part.typ"), "= Розділ.\n").unwrap();
    std::fs::write(
        dir.join("work/main.typ"),
        "#import \"/lib.typ\": x\n#include \"/part.typ\"\n",
    )
    .unwrap();
    let output = dsty()
        .current_dir(dir.join("work"))
        .args(["check", "main.typ"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("[heading-period]"), "{stdout}");
}

#[test]
fn explains_rules() {
    let output = dsty().args(["explain", "list-case"]).output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("7.7.4 Текст кожної позиції переліку"),
        "{stdout}"
    );
    let output = dsty().args(["explain", "nope"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn project_config_and_formats() {
    // Налаштування над текою роботи діють на всі роботи нижче
    let dir = temp_dir("config");
    std::fs::create_dir_all(dir.join("work/src")).unwrap();
    std::fs::write(
        dir.join(".dsty.toml"),
        "ignore = [\"heading-period\"]\n[severity]\nlist-case = \"info\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("work/src/main.typ"),
        "= Розділ.\nПерелік:\n- Перший.\n",
    )
    .unwrap();
    // Тека замість main-файлу, анотації GitHub
    let output = dsty()
        .current_dir(&dir)
        .args(["check", "work", "--format", "github"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout.trim(),
        "::notice file=work/src/main.typ,line=3,col=3,endLine=3,endColumn=10,title=dsty%3A list-case::позиція переліку з великої літери: «Перший.» (п. 7.7.4)"
    );
    // Лише інформація: код виходу 0
    assert_eq!(output.status.code(), Some(0));
    // Без файлу налаштувань спрацьовують обидва правила
    let output = dsty()
        .current_dir(&dir)
        .args(["check", "work", "--no-config", "--format", "json"])
        .output()
        .unwrap();
    let items: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(items.as_array().unwrap().len(), 2);
    assert_eq!(items[1]["severity"], "warning");
    assert_eq!(output.status.code(), Some(1));
}

/// README описує кожне правило.
#[test]
fn readme_lists_every_rule() {
    let readme =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md")).unwrap();
    for rule in dsty::rules::RULES {
        assert!(
            readme.contains(&format!("| `{}` |", rule.code)),
            "README не описує {}",
            rule.code
        );
    }
}
