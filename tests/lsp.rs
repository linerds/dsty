//! LSP-сервер через stdio: діагностики відкритих буферів і швидкі виправлення.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

struct Client {
    child: Child,
    stdin: ChildStdin,
    messages: Receiver<Value>,
    next_id: i64,
}

impl Client {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_dsty"))
            .arg("lsp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let (tx, messages) = channel();
        std::thread::spawn(move || {
            loop {
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    if stdout.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let line = line.trim();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(n) = line.strip_prefix("Content-Length: ") {
                        length = n.parse().unwrap();
                    }
                }
                let mut body = vec![0; length];
                stdout.read_exact(&mut body).unwrap();
                if tx.send(serde_json::from_slice(&body).unwrap()).is_err() {
                    return;
                }
            }
        });
        Self {
            child,
            stdin,
            messages,
            next_id: 1,
        }
    }

    fn send(&mut self, msg: Value) {
        let body = msg.to_string();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        self.stdin.flush().unwrap();
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        self.wait(method, |m| m["id"] == id)["result"].clone()
    }

    /// Наступне повідомлення, для якого `f` повертає `true`.
    fn wait(&self, what: &str, f: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let msg = self
                .messages
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("не дочекався: {what}"));
            if f(&msg) {
                return msg;
            }
        }
    }

    fn wait_diagnostics(&self, uri: &str) -> Vec<Value> {
        self.wait_versioned(uri).0
    }

    /// Зауваження й версія документа, до якої вони належать.
    fn wait_versioned(&self, uri: &str) -> (Vec<Value>, Value) {
        let msg = self.wait("publishDiagnostics", |m| {
            m["method"] == "textDocument/publishDiagnostics" && m["params"]["uri"] == uri
        });
        (
            msg["params"]["diagnostics"].as_array().unwrap().clone(),
            msg["params"]["version"].clone(),
        )
    }
}

#[test]
fn diagnostics_and_quick_fixes() {
    let dir = std::env::temp_dir().join(format!("dsty-lsp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir: PathBuf = dir.canonicalize().unwrap();
    let main = dir.join("main.typ");
    // На диску -- без зауважень; відкритий буфер має перевагу
    std::fs::write(&main, "Текст.\n").unwrap();
    let uri = format!("file://{}", main.display());

    let mut client = Client::spawn();
    client.request(
        "initialize",
        json!({
            "capabilities": {},
            "rootUri": format!("file://{}", dir.display()),
            "initializationOptions": { "main": "main.typ", "debounce": 50 },
        }),
    );
    client.notify("initialized", json!({}));

    let text = "Перелік:\n- Перший;\n- другий.\n- Третій.\n";
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "typst", "version": 1, "text": text } }),
    );
    let (diags, version) = client.wait_versioned(&uri);
    assert_eq!(version, json!(1));
    assert_eq!(diags.len(), 3, "{diags:#?}");
    assert_eq!(diags[0]["code"], "list-case");
    assert_eq!(diags[0]["severity"], 2);

    // Підказка: суть правила й текст пункту стандарту
    let hover = client.request(
        "textDocument/hover",
        json!({ "textDocument": { "uri": uri }, "position": diags[0]["range"]["start"] }),
    );
    let value = hover["contents"]["value"].as_str().unwrap();
    assert!(value.contains("**list-case**"), "{value}");
    assert!(
        value.contains("> **7.7.4** Текст кожної позиції"),
        "{value}"
    );
    assert_eq!(
        diags[0]["range"]["start"],
        json!({ "line": 1, "character": 2 })
    );
    assert!(
        diags[0]["codeDescription"]["href"]
            .as_str()
            .unwrap()
            .contains("lint.md#")
    );

    let actions = client.request(
        "textDocument/codeAction",
        json!({
            "textDocument": { "uri": uri },
            "range": diags[0]["range"],
            "context": { "diagnostics": [diags[0]] },
        }),
    );
    let actions = actions.as_array().unwrap();
    assert_eq!(actions.len(), 4, "{actions:#?}");
    assert_eq!(actions[0]["isPreferred"], true);
    assert_eq!(actions[3]["command"]["command"], "dsty.fullCheck");
    let edit = &actions[0]["edit"]["changes"][&uri][0];
    assert_eq!(edit["newText"], "п");
    assert_eq!(edit["range"]["start"], json!({ "line": 1, "character": 2 }));
    assert!(
        actions[1]["title"]
            .as_str()
            .unwrap()
            .contains("Вимкнути list-case")
    );

    // Буфер змінився: старі виправлення вже не пропонуються, нові -- після перевірки
    client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": uri, "version": 2 },
            "contentChanges": [{ "text": "Перелік:\n- перший;\n- другий.\n" }],
        }),
    );
    assert!(client.wait_diagnostics(&uri).is_empty());

    // Файл, якого main не підключає, перевіряється окремо
    let other = dir.join("old.typ");
    let other_uri = format!("file://{}", other.display());
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": other_uri, "languageId": "typst", "version": 1, "text": "= Розділ.\n" } }),
    );
    let diags = client.wait_diagnostics(&other_uri);
    assert_eq!(diags.len(), 1, "{diags:#?}");
    assert_eq!(diags[0]["code"], "heading-period");

    client.request("shutdown", json!(null));
    client.notify("exit", json!(null));
    client.child.wait().unwrap();
}

/// Повна перевірка (`full = true`): після відкриття документа приходять і її
/// зауваження. Без typst у PATH тест пропускається.
#[test]
fn full_check_on_open() {
    let has_typst = std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|d| d.join("typst").is_file()));
    if !has_typst {
        eprintln!("пропущено: typst немає в PATH");
        return;
    }
    let dir = std::env::temp_dir().join(format!("dsty-lsp-full-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir: PathBuf = dir.canonicalize().unwrap();
    let main = dir.join("main.typ");
    let text = "#set page(paper: \"a5\")\nТекст документа.\n";
    std::fs::write(&main, text).unwrap();
    let uri = format!("file://{}", main.display());

    let mut client = Client::spawn();
    client.request(
        "initialize",
        json!({
            "capabilities": {},
            "rootUri": format!("file://{}", dir.display()),
            "initializationOptions": { "main": "main.typ", "debounce": 50, "full": true },
        }),
    );
    client.notify("initialized", json!({}));
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "typst", "version": 1, "text": text } }),
    );
    // Спершу статичні зауваження, потім -- з фактами компіляції
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let diags = client.wait_diagnostics(&uri);
        if let Some(d) = diags.iter().find(|d| d["code"] == "page-size") {
            // Зауваження стоїть на `set page`, а не на 1:1
            assert_eq!(d["range"]["start"], json!({ "line": 0, "character": 1 }));
            break;
        }
        assert!(Instant::now() < deadline, "немає page-size: {diags:#?}");
    }

    client.request("shutdown", json!(null));
    client.notify("exit", json!(null));
    client.child.wait().unwrap();
}

/// «Виправити все» з `codeAction/resolve`: заміна рахується лише при виборі
/// дії; з `only: source.fixAll` (при збереженні) -- лише ця дія.
#[test]
fn fix_all_is_resolved_lazily() {
    let dir = std::env::temp_dir().join(format!("dsty-lsp-resolve-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir: PathBuf = dir.canonicalize().unwrap();
    let main = dir.join("main.typ");
    std::fs::write(&main, "").unwrap();
    let uri = format!("file://{}", main.display());

    let mut client = Client::spawn();
    client.request(
        "initialize",
        json!({
            "capabilities": { "textDocument": { "codeAction": {
                "resolveSupport": { "properties": ["edit"] }
            } } },
            "rootUri": format!("file://{}", dir.display()),
            "initializationOptions": { "main": "main.typ", "debounce": 50 },
        }),
    );
    client.notify("initialized", json!({}));
    let text = "Перелік:\n- Перший;\n- другий.\n- Третій.\n";
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "typst", "version": 1, "text": text } }),
    );
    let diags = client.wait_diagnostics(&uri);
    assert_eq!(diags.len(), 3, "{diags:#?}");

    let actions = client.request(
        "textDocument/codeAction",
        json!({
            "textDocument": { "uri": uri },
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } },
            "context": { "diagnostics": [], "only": ["source.fixAll"] },
        }),
    );
    let actions = actions.as_array().unwrap();
    assert_eq!(actions.len(), 1, "{actions:#?}");
    assert_eq!(actions[0]["kind"], "source.fixAll");
    assert!(actions[0].get("edit").is_none());

    let resolved = client.request("codeAction/resolve", actions[0].clone());
    let edit = &resolved["edit"]["changes"][&uri][0];
    assert_eq!(
        edit["newText"],
        "Перелік:\n- перший;\n- другий;\n- третій.\n"
    );

    client.request("shutdown", json!(null));
    client.notify("exit", json!(null));
    client.child.wait().unwrap();
}

/// `source.fixAll` виправляє й єдине зауваження файлу, а «Виправити все в
/// документі» -- як `check --fix`: main і підключені файли однією дією.
#[test]
fn fix_all_single_and_whole_document() {
    let dir = std::env::temp_dir().join(format!("dsty-lsp-document-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir: PathBuf = dir.canonicalize().unwrap();
    let main = dir.join("main.typ");
    let chapter = dir.join("ch.typ");
    let main_text = "= Розділ.\n#include \"ch.typ\"\n";
    std::fs::write(&main, main_text).unwrap();
    std::fs::write(&chapter, "Перелік:\n- Перший;\n- другий.\n").unwrap();
    let uri = format!("file://{}", main.display());
    let chapter_uri = format!("file://{}", chapter.display());

    let mut client = Client::spawn();
    client.request(
        "initialize",
        json!({
            "capabilities": { "textDocument": { "codeAction": {
                "resolveSupport": { "properties": ["edit"] }
            } } },
            "rootUri": format!("file://{}", dir.display()),
            "initializationOptions": { "main": "main.typ", "debounce": 50 },
        }),
    );
    client.notify("initialized", json!({}));
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "typst", "version": 1, "text": main_text } }),
    );
    // Зауваження обох файлів -- в одній публікації перевірки
    let mut seen = std::collections::HashMap::new();
    while seen.len() < 2 {
        let msg = client.wait("publishDiagnostics", |m| {
            m["method"] == "textDocument/publishDiagnostics"
                && (m["params"]["uri"] == uri || m["params"]["uri"] == chapter_uri)
        });
        let params = &msg["params"];
        seen.insert(
            params["uri"].as_str().unwrap().to_owned(),
            params["diagnostics"].as_array().unwrap().len(),
        );
    }
    assert_eq!(seen[&uri], 1, "{seen:?}");
    assert_eq!(seen[&chapter_uri], 1, "{seen:?}");

    let start =
        json!({ "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } });
    // Одне зауваження у файлі: при збереженні його теж виправити
    let on_save = client.request(
        "textDocument/codeAction",
        json!({
            "textDocument": { "uri": uri },
            "range": start,
            "context": { "diagnostics": [], "only": ["source.fixAll"] },
        }),
    );
    let on_save = on_save.as_array().unwrap();
    assert_eq!(on_save.len(), 1, "{on_save:#?}");
    let resolved = client.request("codeAction/resolve", on_save[0].clone());
    let changes = resolved["edit"]["changes"].as_object().unwrap();
    assert_eq!(changes.len(), 1, "{changes:#?}");
    assert_eq!(
        changes[&uri][0]["newText"],
        "= Розділ\n#include \"ch.typ\"\n"
    );

    // Без `only`: швидке виправлення й увесь документ, а «у файлі» -- ні,
    // бо воно таке саме, як швидке
    let actions = client.request(
        "textDocument/codeAction",
        json!({
            "textDocument": { "uri": uri },
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 9 } },
            "context": { "diagnostics": [] },
        }),
    );
    let actions = actions.as_array().unwrap();
    let kinds: Vec<&str> = actions
        .iter()
        .map(|a| a["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        [
            "quickfix",
            "quickfix",
            "source.dsty.fixDocument",
            "source.dsty.fullCheck"
        ],
        "{actions:#?}"
    );
    let resolved = client.request("codeAction/resolve", actions[2].clone());
    assert_eq!(resolved["title"], "Виправити все в документі (2)");
    let changes = resolved["edit"]["changes"].as_object().unwrap();
    assert_eq!(changes.len(), 2, "{changes:#?}");
    assert_eq!(
        changes[&chapter_uri][0]["newText"],
        "Перелік:\n- перший;\n- другий.\n"
    );

    // `source.fixAll` при збереженні не чіпає інших файлів
    let fix_all = client.request(
        "textDocument/codeAction",
        json!({
            "textDocument": { "uri": uri },
            "range": start,
            "context": { "diagnostics": [], "only": ["source.fixAll"] },
        }),
    );
    assert!(
        fix_all
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["kind"] == "source.fixAll"),
        "{fix_all:#?}"
    );

    client.request("shutdown", json!(null));
    client.notify("exit", json!(null));
    client.child.wait().unwrap();
}

/// Доповнення назв правил у `dstu-ignore`, підказка до назви в коментарі,
/// `.dsty.toml` проєкту й `didChangeConfiguration`.
#[test]
fn comments_config_and_settings() {
    let dir = std::env::temp_dir().join(format!("dsty-lsp-config-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir: PathBuf = dir.canonicalize().unwrap();
    let main = dir.join("main.typ");
    std::fs::write(&main, "").unwrap();
    // Файл проєкту: list-case -- підказка
    std::fs::write(dir.join(".dsty.toml"), "[severity]\nlist-case = \"hint\"\n").unwrap();
    let uri = format!("file://{}", main.display());

    let mut client = Client::spawn();
    client.request(
        "initialize",
        json!({
            "capabilities": {},
            "rootUri": format!("file://{}", dir.display()),
            "initializationOptions": { "debounce": 50 },
        }),
    );
    client.notify("initialized", json!({}));
    let text = "// dstu-ignore-next-line: list-c\n- Велика.\n";
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "typst", "version": 1, "text": text } }),
    );
    let diags = client.wait_diagnostics(&uri);
    let codes: Vec<_> = diags.iter().map(|d| d["code"].clone()).collect();
    assert_eq!(
        codes,
        [json!("ignore-unknown"), json!("list-case")],
        "{diags:#?}"
    );
    assert_eq!(diags[1]["severity"], 4);

    // Доповнення після двокрапки -- назви правил
    let items = client.request(
        "textDocument/completion",
        json!({ "textDocument": { "uri": uri }, "position": { "line": 0, "character": 32 } }),
    );
    let labels: Vec<&str> = items
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["label"].as_str().unwrap())
        .collect();
    assert!(labels.contains(&"list-case"), "{labels:?}");
    // Підказка до назви правила в коментарі
    let text = "// dstu-ignore-next-line: list-case\n- велика.\n";
    client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": uri, "version": 2 },
            "contentChanges": [{ "text": text }],
        }),
    );
    let (_, version) = client.wait_versioned(&uri);
    assert_eq!(version, json!(2));
    let hover = client.request(
        "textDocument/hover",
        json!({ "textDocument": { "uri": uri }, "position": { "line": 0, "character": 28 } }),
    );
    assert!(
        hover["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("**list-case**")
    );

    // Налаштування редактора: правило вимкнено
    client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": uri, "version": 3 },
            "contentChanges": [{ "text": "- Велика.\n" }],
        }),
    );
    assert_eq!(client.wait_diagnostics(&uri).len(), 1);
    client.notify(
        "workspace/didChangeConfiguration",
        json!({ "settings": { "dsty": { "ignore": ["list-case"] } } }),
    );
    assert!(client.wait_diagnostics(&uri).is_empty());

    client.request("shutdown", json!(null));
    client.notify("exit", json!(null));
    client.child.wait().unwrap();
}

/// Команда `dsty.fullCheck` запускає повну перевірку й без `full = true`, а
/// клієнт, що вміє `$/progress`, бачить її хід.
#[test]
fn full_check_command_with_progress() {
    let has_typst = std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|d| d.join("typst").is_file()));
    if !has_typst {
        eprintln!("пропущено: typst немає в PATH");
        return;
    }
    let dir = std::env::temp_dir().join(format!("dsty-lsp-cmd-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir: PathBuf = dir.canonicalize().unwrap();
    let main = dir.join("main.typ");
    let text = "#set page(paper: \"a5\")\nТекст документа.\n";
    std::fs::write(&main, text).unwrap();
    let uri = format!("file://{}", main.display());

    let mut client = Client::spawn();
    client.request(
        "initialize",
        json!({
            "capabilities": { "window": { "workDoneProgress": true } },
            "rootUri": format!("file://{}", dir.display()),
            "initializationOptions": { "debounce": 50 },
        }),
    );
    client.notify("initialized", json!({}));
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "typst", "version": 1, "text": text } }),
    );
    // Чистий документ: порожній перелік зауважень з версією
    let (diags, version) = client.wait_versioned(&uri);
    assert!(diags.is_empty());
    assert_eq!(version, json!(1));
    client.request(
        "workspace/executeCommand",
        json!({ "command": "dsty.fullCheck", "arguments": [] }),
    );
    client.wait("progress create", |m| {
        m["method"] == "window/workDoneProgress/create"
    });
    client.wait("progress end", |m| {
        m["method"] == "$/progress" && m["params"]["value"]["kind"] == "end"
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let diags = client.wait_diagnostics(&uri);
        if diags.iter().any(|d| d["code"] == "page-size") {
            break;
        }
        assert!(Instant::now() < deadline, "немає page-size: {diags:#?}");
    }

    client.request("shutdown", json!(null));
    client.notify("exit", json!(null));
    client.child.wait().unwrap();
}
