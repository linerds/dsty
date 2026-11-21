//! Запуск нативного `typst` користувача: один виклик `typst eval --in -` з
//! обгорткою через stdin, межі часу й пам'яті. Середовище (PATH,
//! `TYPST_PACKAGE_PATH`) -- як у процесу dsty.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context as _, bail};

use super::facts::{Entry, Facts, Warning};

/// Налаштування запуску.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    /// Шлях до `typst`; без нього -- з `PATH`.
    pub typst: Option<PathBuf>,
    /// Корінь проєкту для typst (`--root`); типово -- тека main-файлу, як у
    /// `typst compile`.
    pub root: Option<PathBuf>,
    /// Межа часу компіляції (типово 60 с).
    pub timeout: Option<Duration>,
}

/// Зупинити всі компіляції, що йдуть (LSP-сервер завершує роботу): typst
/// не має пережити dsty.
static CANCEL: AtomicBool = AtomicBool::new(false);

/// Перериває компіляції, що йдуть, і не дає почати нові.
pub fn cancel_all() {
    CANCEL.store(true, Ordering::SeqCst);
}

const PROBE: &str = include_str!("probe.typ");
const QUERY: &str = include_str!("query.typ");

/// Секції обгортки й чи потрібна кожна.
#[derive(Debug, Clone, Copy)]
pub struct Sections {
    pub text: bool,
    pub par: bool,
    pub table: bool,
    pub equation: bool,
    pub runs: bool,
}

/// Обгортка з потрібними секціями проб.
pub fn wrapper(main_rel: &str, sections: Sections) -> String {
    let keep = |name: &str| match name {
        "marks" => sections.text || sections.par || sections.runs,
        "text" => sections.text,
        "par" => sections.par,
        "table" => sections.table,
        "equation" => sections.equation,
        "runs" => sections.runs,
        _ => true,
    };
    let mut out = String::with_capacity(PROBE.len());
    let mut skipping = false;
    for line in PROBE.lines() {
        if let Some(name) = line.strip_prefix("// @") {
            let name = name.trim();
            if name == "end" {
                skipping = false;
            } else {
                skipping = !keep(name);
            }
            continue;
        }
        if !skipping {
            out.push_str(line);
            out.push('\n');
        }
    }
    // Шлях у рядку Typst: зворотні скісні й лапки екрануються
    let escaped = main_rel.replace('\\', "\\\\").replace('"', "\\\"");
    out.replace("/__MAIN__", &format!("/{escaped}"))
}

/// Компілює документ з пробами й повертає факти.
pub fn collect(main: &Path, settings: &Settings, sections: Sections) -> anyhow::Result<Facts> {
    let main_dir = main.parent().context("main-файл без теки")?;
    let root = settings
        .root
        .clone()
        .unwrap_or_else(|| main_dir.to_path_buf());
    let rel = main
        .strip_prefix(&root)
        .with_context(|| format!("{} поза коренем {}", main.display(), root.display()))?;
    let rel = rel.to_string_lossy().replace('\\', "/");

    let typst = find_typst(settings.typst.as_deref()).with_context(|| match &settings.typst {
        Some(path) => format!("typst не знайдено: {}", path.display()),
        None => "typst не знайдено в PATH".into(),
    })?;
    if CANCEL.load(Ordering::SeqCst) {
        bail!("перевірку скасовано");
    }

    // `typst eval` є з 0.15; раніші версії отримують той самий запит усередині
    // документа (один `context` наприкінці) і віддають його через `typst query`
    let eval = version(&typst).is_none_or(|v| v >= (0, 15));
    let mut input = wrapper(&rel, sections);
    let mut cmd = Command::new(&typst);
    if eval {
        cmd.args(["eval", "--in", "-", "--root"])
            .arg(&root)
            .arg(QUERY);
    } else {
        input.push_str(&format!("\n#context [#metadata({QUERY})<dsty-result>]\n"));
        cmd.args([
            "query",
            "-",
            "<dsty-result>",
            "--field",
            "value",
            "--one",
            "--root",
        ])
        .arg(&root);
    }
    cmd.current_dir(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    limit_memory(&mut cmd);
    let mut child = cmd
        .spawn()
        .with_context(|| format!("запуск {}", typst.display()))?;

    // stdout і stderr читаються в окремих потоках ще до запису stdin, щоб
    // typst не став на повному каналі
    let mut stdout = child.stdout.take().context("stdout typst")?;
    let mut stderr = child.stderr.take().context("stderr typst")?;
    let out_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        stdout.read_to_end(&mut buf).map(|_| buf)
    });
    let err_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        stderr
            .read_to_end(&mut buf)
            .map(|_| String::from_utf8_lossy(&buf).into_owned())
    });
    // typst, що завершився одразу (невідомий аргумент), закриває stdin раніше:
    // тоді причину скаже його stderr, а не «broken pipe»
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input.as_bytes());
    }

    let timeout = settings.timeout.unwrap_or(Duration::from_secs(60));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        let cancelled = CANCEL.load(Ordering::SeqCst);
        if cancelled || started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            if cancelled {
                bail!("перевірку скасовано");
            }
            bail!("typst не вклався в {} с", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let stdout = out_thread
        .join()
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    let stderr = err_thread
        .join()
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    if !status.success() {
        let first = stderr
            .lines()
            .skip_while(|l| !l.starts_with("error"))
            .take(4)
            .collect::<Vec<_>>()
            .join(" ");
        bail!(
            "typst завершився з помилкою ({status}): {}",
            if first.is_empty() {
                stderr.trim()
            } else {
                first.trim()
            }
        );
    }
    // Для налагодження: сирий потік фактів у файл
    if let Some(path) = std::env::var_os("DSTY_FULL_DUMP") {
        let _ = std::fs::write(path, &stdout);
    }
    let entries: Vec<Entry> = serde_json::from_slice(&stdout).context("розбір відповіді typst")?;
    Ok(Facts {
        entries,
        warnings: warnings(&stderr),
    })
}

/// Версія typst: `typst 0.14.2 (b33de9de)` -> (0, 14). Запам'ятовується за
/// шляхом і часом зміни бінарника: окремий процес на кожну перевірку зайвий.
fn version(typst: &Path) -> Option<(u32, u32)> {
    type Cache = HashMap<(PathBuf, Option<SystemTime>), Option<(u32, u32)>>;
    static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Mutex::default);
    let modified = std::fs::metadata(typst).and_then(|m| m.modified()).ok();
    let key = (typst.to_path_buf(), modified);
    if let Some(&cached) = CACHE.lock().unwrap().get(&key) {
        return cached;
    }
    let found = read_version(typst);
    CACHE.lock().unwrap().insert(key, found);
    found
}

fn read_version(typst: &Path) -> Option<(u32, u32)> {
    let output = Command::new(typst).arg("--version").output().ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    let mut parts = text.split_whitespace().nth(1)?.split('.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// Попередження typst: `warning: текст` і далі `┌─ файл:рядок:колонка`.
fn warnings(stderr: &str) -> Vec<Warning> {
    let mut out: Vec<Warning> = Vec::new();
    for line in stderr.lines() {
        if let Some(msg) = line.strip_prefix("warning: ") {
            out.push(Warning {
                message: msg.trim().to_owned(),
                file: None,
                line: None,
            });
        } else if let Some(at) = line.trim_start().strip_prefix("┌─ ")
            && let Some(last) = out.last_mut()
            && last.file.is_none()
        {
            let mut parts = at.rsplitn(3, ':');
            let _col = parts.next();
            let line = parts.next().and_then(|l| l.parse().ok());
            last.file = parts.next().map(str::to_owned);
            last.line = line;
        }
    }
    out
}

/// `typst`: заданий шлях, задана назва (`typst-0.15`) чи `typst` з `PATH`.
/// Середовище проєкту (typst і `TYPST_PACKAGE_PATH` від mise, direnv тощо)
/// успадковується від процесу. Шлях абсолютний (typst запускається з теки
/// кореня, де відносний шлях означав би інше), але символьні посилання не
/// розкриваються: шим mise за назвою посилання знає, що запускати.
fn find_typst(configured: Option<&Path>) -> Option<PathBuf> {
    let name = match configured {
        Some(path) if path.components().count() > 1 || path.is_absolute() => {
            return std::path::absolute(path).ok().filter(|p| p.is_file());
        }
        Some(name) => name.as_os_str(),
        None => "typst".as_ref(),
    };
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
        .and_then(|p| std::path::absolute(p).ok())
}

/// Межа пам'яті typst (4 ГБ адресного простору): помилка в обгортці чи
/// документі не має з'їсти пам'ять редактора.
#[cfg(unix)]
fn limit_memory(cmd: &mut Command) {
    use std::os::unix::process::CommandExt as _;
    // SAFETY: між fork і exec викликається лише setrlimit, безпечний для async-signal.
    unsafe {
        cmd.pre_exec(|| {
            let limit = libc::rlimit {
                rlim_cur: 4 << 30,
                rlim_max: 4 << 30,
            };
            libc::setrlimit(libc::RLIMIT_AS, &limit);
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn limit_memory(_: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_are_cut() {
        let all = wrapper(
            "src/main.typ",
            Sections {
                text: true,
                par: true,
                table: true,
                equation: true,
                runs: true,
            },
        );
        assert!(all.contains("#show text.where(fill: rgb(\"#010001\"))"));
        assert!(all.contains("#include \"/src/main.typ\""));
        let lean = wrapper(
            "main.typ",
            Sections {
                text: false,
                par: false,
                table: false,
                equation: false,
                runs: false,
            },
        );
        assert!(!lean.contains("#show text.where"));
        assert!(!lean.contains("#show heading: set text"));
        assert!(lean.contains("#set page(foreground: dsty-page, background: dsty-page)"));
    }

    #[test]
    fn parses_warnings() {
        let stderr =
            "warning: unknown font family: times new roman\n    ┌─ src/main.typ:3:20\n    │\n";
        let w = warnings(stderr);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].file.as_deref(), Some("src/main.typ"));
        assert_eq!(w[0].line, Some(3));
    }
}
