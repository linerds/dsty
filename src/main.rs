use std::collections::BTreeMap;
use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Context as _;
use clap::{Parser, Subcommand, ValueEnum};

use dsty::config::{self, Config};
use dsty::full;
use dsty::lint::{self, Diagnostic, Severity};
use dsty::lsp;
use dsty::model;
use dsty::render;
use dsty::rules::{self, RULES};

/// Лінтер ДСТУ 3008:2015 для Typst. Статична перевірка читає вихідний код без
/// компіляції, тож працює з будь-якою версією шаблону й Typst; повна
/// (`--full`) ще й компілює документ нативним `typst` проєкту.
///
/// Налаштування проєкту лежать у `.dsty.toml` у теці документа чи вище.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Перевірити документи й надрукувати зауваження.
    ///
    /// Код виходу: 0, якщо зауважень немає (або є лише підказки й рекомендації),
    /// 1, якщо є зауваження, 2 у разі помилки.
    Check(CheckArgs),
    /// Запустити LSP-сервер на stdio.
    Lsp,
    /// Перелік правил: код, пункт стандарту, суть.
    Rules {
        /// Формат: текст або JSON.
        #[arg(long, value_enum, default_value_t = RulesFormat::Text)]
        format: RulesFormat,
    },
    /// Пояснити правило: суть, текст пунктів стандарту, як вимкнути.
    Explain {
        /// Код правила, напр. `list-punct`.
        rule: String,
    },
}

#[derive(clap::Args)]
struct CheckArgs {
    /// Main-файли документів (з них обходяться `#include`) або теки з
    /// `src/main.typ` чи `main.typ`.
    #[arg(required = true)]
    paths: Vec<PathBuf>,
    /// Корінь проєкту для шляхів `/…` (як `typst --root`); типово це тека
    /// main-файлу або найближча вище, де знаходяться всі імпорти `/…`.
    #[arg(long)]
    root: Option<PathBuf>,
    /// Файл налаштувань замість знайденого `.dsty.toml`.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Не читати `.dsty.toml`.
    #[arg(long, conflicts_with = "config")]
    no_config: bool,
    /// Правила, які не перевіряти (через кому або кількома прапорцями).
    #[arg(long, value_delimiter = ',')]
    ignore: Vec<String>,
    /// Додаткові правила, типово вимкнені, напр. `unit-space,unit-range`.
    #[arg(long, value_delimiter = ',')]
    enable: Vec<String>,
    /// Виправити те, що можна виправити автоматично, і записати файли.
    #[arg(long)]
    fix: bool,
    /// Повна перевірка: скомпілювати документ нативним `typst` і перевірити
    /// сторінки, стилі й номери. Без typst чи з помилкою компіляції
    /// лишається тільки статична перевірка.
    #[arg(long)]
    full: bool,
    /// `typst` для повної перевірки (типово з PATH).
    #[arg(long)]
    typst: Option<PathBuf>,
    /// Формат виводу (типово `pretty` у терміналі, інакше `text`).
    #[arg(long, value_enum)]
    format: Option<Format>,
    /// Наприкінці надрукувати, скільки зауважень дало кожне правило.
    #[arg(long)]
    statistics: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Format {
    /// `файл:рядок:кол: [правило] повідомлення (п. …)`
    Text,
    /// З рядком коду й позначкою місця, кольором у терміналі.
    Pretty,
    /// Масив JSON: файл, рядки й колонки (з 1), правило, пункт, рівень, текст.
    Json,
    /// Анотації GitHub Actions (`::warning file=…`).
    Github,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum RulesFormat {
    Text,
    Json,
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Check(args) => check(args),
        Command::Lsp => lsp::run().map(|()| ExitCode::SUCCESS),
        Command::Rules { format } => {
            print_rules(format);
            Ok(ExitCode::SUCCESS)
        }
        Command::Explain { rule } => explain(&rule),
    };
    result.unwrap_or_else(|err| {
        eprintln!("dsty: {err:#}");
        ExitCode::from(2)
    })
}

fn print_rules(format: RulesFormat) {
    match format {
        RulesFormat::Text => {
            print!("{}", render::rules_table(RULES, color(), width()));
        }
        RulesFormat::Json => {
            let items: Vec<_> = RULES
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "code": r.code,
                        "clause": r.clause,
                        "description": r.desc,
                        "kind": r.kind(),
                        "full": r.full,
                        "optional": r.optional,
                        "severity": r.severity.name(),
                    })
                })
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&items).unwrap_or_default()
            );
        }
    }
}

/// Чи stdout є терміналом.
fn tty() -> bool {
    std::io::stdout().is_terminal()
}

/// Колір у терміналі, якщо його не вимкнено `NO_COLOR`.
fn color() -> bool {
    tty() && std::env::var_os("NO_COLOR").is_none()
}

/// Ширина терміналу для перенесення рядків.
fn width() -> Option<usize> {
    if tty() { render::term_width() } else { None }
}

fn explain(code: &str) -> anyhow::Result<ExitCode> {
    let Some(rule) = rules::find(code) else {
        anyhow::bail!("невідоме правило «{code}»; перелік: dsty rules");
    };
    print!("{}", render::explain(rule, color(), width()));
    Ok(ExitCode::SUCCESS)
}

/// Main-файл за шляхом: сам файл або `src/main.typ` чи `main.typ` теки.
fn main_file(path: &Path) -> anyhow::Result<PathBuf> {
    if path.is_dir() {
        for candidate in config::MAIN_FILES {
            let file = path.join(candidate);
            if file.is_file() {
                return canonical(&file, "main-файл");
            }
        }
        anyhow::bail!("у теці {} немає src/main.typ чи main.typ", path.display());
    }
    canonical(path, "main-файл")
}

fn check(args: CheckArgs) -> anyhow::Result<ExitCode> {
    let format = args
        .format
        .unwrap_or(if tty() { Format::Pretty } else { Format::Text });
    let colored = format == Format::Pretty && color();
    let mut mains: Vec<PathBuf> = Vec::new();
    for path in &args.paths {
        // Той самий документ двічі (`dsty check . src/main.typ`) перевіряється один раз
        let main = main_file(path)?;
        if !mains.contains(&main) {
            mains.push(main);
        }
    }

    let mut all: Vec<Diagnostic> = Vec::new();
    let mut files = 0;
    for main in &mains {
        let (diagnostics, count) = check_one(&args, main)?;
        files += count;
        all.extend(diagnostics);
    }

    match format {
        Format::Text => {
            for diag in &all {
                println!(
                    "{}:{}:{}: [{}] {}",
                    render::show_path(&diag.path),
                    diag.start.line + 1,
                    diag.start.column + 1,
                    diag.rule,
                    diag.message()
                );
            }
        }
        Format::Pretty => {
            let mut sources: BTreeMap<PathBuf, String> = BTreeMap::new();
            for diag in &all {
                let text = sources
                    .entry(diag.path.clone())
                    .or_insert_with(|| std::fs::read_to_string(&diag.path).unwrap_or_default());
                print!(
                    "{}",
                    render::diagnostic(diag, &render::show_path(&diag.path), text, colored)
                );
            }
        }
        Format::Json => {
            let items: Vec<_> = all
                .iter()
                .map(|d| {
                    serde_json::json!({
                        "file": render::show_path(&d.path),
                        "line": d.start.line + 1,
                        "column": d.start.column + 1,
                        "endLine": d.end.line + 1,
                        "endColumn": d.end.column + 1,
                        "rule": d.rule,
                        "clause": d.clause,
                        "severity": d.severity.name(),
                        "message": d.msg,
                        "help": d.help,
                        "assumed": d.assumed,
                        "fixable": d.auto_fix().is_some(),
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&items)?);
        }
        Format::Github => {
            for d in &all {
                let level = match d.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                    Severity::Info | Severity::Hint => "notice",
                };
                // Значення анотацій GitHub: `%`, `\r`, `\n` (і `,`/`:` у властивостях) кодуються
                let esc = |s: &str| {
                    s.replace('%', "%25")
                        .replace('\r', "%0D")
                        .replace('\n', "%0A")
                };
                let prop = |s: &str| esc(s).replace(':', "%3A").replace(',', "%2C");
                println!(
                    "::{level} file={},line={},col={},endLine={},endColumn={},title={}::{}",
                    prop(&render::show_path(&d.path)),
                    d.start.line + 1,
                    d.start.column + 1,
                    d.end.line + 1,
                    d.end.column + 1,
                    prop(&format!("dsty: {}", d.rule)),
                    esc(&d.message())
                );
            }
        }
    }

    if args.statistics {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for d in &all {
            *counts.entry(d.rule).or_default() += 1;
        }
        let mut counts: Vec<_> = counts.into_iter().collect();
        counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        for (rule, n) in counts {
            let fixable = all
                .iter()
                .filter(|d| d.rule == rule && d.auto_fix().is_some())
                .count();
            let fixable = if fixable > 0 {
                format!("  (виправних: {fixable})")
            } else {
                String::new()
            };
            eprintln!("{n:>5}  {rule}{fixable}");
        }
    }
    let fixable = all.iter().filter(|d| d.auto_fix().is_some()).count();
    let hint = if fixable > 0 && !args.fix {
        format!("; автоматично виправить --fix: {fixable}")
    } else {
        String::new()
    };
    eprintln!("dsty: зауважень: {} (файлів: {files}){hint}", all.len());
    Ok(if all.iter().any(|d| d.severity >= Severity::Warning) {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

/// Перевіряє один документ (з виправленням, якщо треба): зауваження й
/// кількість прочитаних файлів.
fn check_one(args: &CheckArgs, main: &Path) -> anyhow::Result<(Vec<Diagnostic>, usize)> {
    let load = |path: &Path| std::fs::read_to_string(path).ok();
    // Налаштування: файл проєкту, потім прапорці
    let file_config = if args.no_config {
        None
    } else if let Some(path) = &args.config {
        Some(Config::load(path)?)
    } else {
        main.parent()
            .and_then(Config::discover)
            .map(|p| Config::load(&p))
            .transpose()?
    };
    let config = file_config.unwrap_or_default().merged(Config {
        root: args.root.clone(),
        ignore: args.ignore.clone(),
        enable: args.enable.clone(),
        full: args.full.then_some(true),
        typst: args.typst.clone(),
        ..Default::default()
    });
    let root = match &config.root {
        Some(root) => canonical(root, "корінь")?,
        None => model::detect_root(main, &load),
    };
    let options = config.options();
    lint::validate(&options)?;
    let facts = if config.full == Some(true) && full::wanted(&options) {
        let settings = full::Settings {
            typst: config.typst.clone(),
            root: Some(root.clone()),
            timeout: None,
        };
        match full::collect(main, &settings, &options) {
            Ok(facts) => Some(facts),
            Err(err) => {
                eprintln!("dsty: повну перевірку пропущено: {err:#}");
                None
            }
        }
    } else {
        None
    };
    let mut report = lint::check_full(&root, main, &load, &options, facts.as_ref())?;
    for (file, pos, path) in &report.missing {
        eprintln!(
            "dsty: {}:{}:{}: немає файлу {}",
            render::show_path(file),
            pos.line + 1,
            pos.column + 1,
            render::show_path(path)
        );
    }

    if args.fix {
        let fixed = lint::fix_all(&root, main, &load, &options, None)?;
        for (path, text) in &fixed.files {
            write_atomic(path, text).with_context(|| format!("запис {}", path.display()))?;
        }
        eprintln!("dsty: виправлено: {}", fixed.count);
        if fixed.count > 0 {
            report = lint::check_full(&root, main, &load, &options, facts.as_ref())?;
        }
    }
    Ok((report.diagnostics, report.files.len()))
}

/// Записує файл через тимчасовий поруч і перейменування: збій посеред запису
/// (повний диск) не лишає документ обрізаним. Символьне посилання лишається
/// посиланням (пишеться файл, на який воно вказує), права не змінюються.
fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let target = path.canonicalize()?;
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    let tmp = target.with_file_name(format!(".{name}.dsty-{}", std::process::id()));
    let result = std::fs::write(&tmp, text)
        .and_then(|()| {
            let permissions = std::fs::metadata(&target)?.permissions();
            std::fs::set_permissions(&tmp, permissions)
        })
        .and_then(|()| std::fs::rename(&tmp, &target));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn canonical(path: &Path, what: &str) -> anyhow::Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("{what} {}", path.display()))
}
