//! LSP-сервер на stdio: `publishDiagnostics`, швидкі виправлення
//! (`codeAction`), підказки з текстом стандарту (`hover`), доповнення назв
//! правил у коментарях `dstu-ignore` і команда повної перевірки.
//!
//! Головний цикл приймає повідомлення й тримає вміст відкритих буферів.
//! Перевірка йде в окремому потоці: запити на перевірку чекають `debounce`,
//! новіший запит заміняє старіший, а результат застарілої перевірки не
//! публікується. Статична перевірка лише розбирає код, тож займає мілісекунди.
//!
//! Повна перевірка (`full = true`) компілює документ нативним `typst` лише
//! при відкритті й збереженні (і на команду `dsty.fullCheck`), у третьому
//! потоці. Її факти кешуються й заново прив'язуються до коду на кожній
//! статичній перевірці, тож під час набору зауваження не з'їжджають.
//!
//! Налаштування: `.dsty.toml` проєкту, а поверх нього `initializationOptions`
//! і `workspace/didChangeConfiguration` редактора.

use std::collections::{HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeConfiguration, DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument,
    DidSaveTextDocument, LogMessage, Notification as _, Progress, PublishDiagnostics,
};
use lsp_types::request::{
    CodeActionRequest, CodeActionResolveRequest, Completion, ExecuteCommand, HoverRequest,
    Request as _, WorkDoneProgressCreate,
};
use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOptions, CodeActionOrCommand, CodeActionParams,
    CodeActionProviderCapability, Command, CompletionItem, CompletionItemKind, CompletionOptions,
    CompletionParams, CompletionResponse, Diagnostic, DiagnosticRelatedInformation,
    DiagnosticSeverity, DiagnosticTag, Documentation, ExecuteCommandOptions, ExecuteCommandParams,
    Hover, HoverContents, HoverParams, HoverProviderCapability, InitializeParams, Location,
    LogMessageParams, MarkupContent, MarkupKind, MessageType, NumberOrString, Position,
    ProgressParams, ProgressParamsValue, PublishDiagnosticsParams, Range, SaveOptions,
    ServerCapabilities, ServerInfo, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncOptions, TextDocumentSyncSaveOptions, TextEdit, Url, WorkDoneProgress,
    WorkDoneProgressBegin, WorkDoneProgressCreateParams, WorkDoneProgressEnd, WorkspaceEdit,
};

use crate::config::{self, Config};
use crate::full::{self, Facts};
use crate::lint::{self, Options, Severity};
use crate::{model, rules};

/// Команда повної перевірки (і дія з нею).
const FULL_CHECK: &str = "dsty.fullCheck";
/// Дія «Виправити все в документі».
const FIX_DOCUMENT: &str = "source.dsty.fixDocument";

/// Що охоплює «Виправити все».
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum FixScope {
    /// Поточний файл.
    File,
    /// Документ від main з усіма підключеними файлами, як `check --fix`.
    Document,
}

impl FixScope {
    fn title(self) -> &'static str {
        match self {
            FixScope::File => "Виправити все у файлі",
            FixScope::Document => "Виправити все в документі",
        }
    }
}

/// Що й де перевіряти.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    root: PathBuf,
    main: PathBuf,
    /// Main-файл не задано й не знайдено, тож узято відкритий файл: тоді
    /// помилки лише пишуться в журнал.
    guessed: bool,
}

/// Відкритий буфер: текст і версія від редактора.
#[derive(Debug, Clone)]
struct Buffer {
    text: Arc<str>,
    version: Option<i32>,
}

struct Job {
    generation: u64,
    delay: Duration,
    target: Target,
    overlays: Overlays,
    options: Arc<Options>,
}

/// Повна перевірка: що компілювати й з якими правилами.
struct FullJob {
    target: Target,
    options: Arc<Options>,
    typst: Option<PathBuf>,
}

/// Вміст відкритих буферів за канонічним шляхом: копіювання лише збільшує лічильник.
type Overlays = HashMap<PathBuf, Buffer>;

/// Наслідки останньої перевірки: з них будуються виправлення й підказки.
type Published = Arc<Mutex<Checked>>;

#[derive(Default)]
struct Checked {
    /// Зауваження за файлом.
    by_path: HashMap<PathBuf, Vec<lint::Diagnostic>>,
    /// Файли документа від main (без відкритих файлів, які main не
    /// підключає): для «Виправити все в документі».
    document: HashSet<PathBuf>,
}
/// Факти останньої повної перевірки за main-файлом.
type FullFacts = Arc<Mutex<HashMap<PathBuf, Arc<Facts>>>>;
type Docs = Arc<Mutex<Overlays>>;

/// `lock()`, що переживає паніку іншого потоку.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn run() -> anyhow::Result<()> {
    let (connection, io_threads) = Connection::stdio();

    let capabilities = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::FULL),
                save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                    include_text: Some(false),
                })),
                ..Default::default()
            },
        )),
        code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
            code_action_kinds: Some(vec![
                CodeActionKind::QUICKFIX,
                CodeActionKind::SOURCE_FIX_ALL,
                CodeActionKind::SOURCE,
            ]),
            resolve_provider: Some(true),
            ..Default::default()
        })),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![":".into(), ",".into()]),
            ..Default::default()
        }),
        execute_command_provider: Some(ExecuteCommandOptions {
            commands: vec![FULL_CHECK.into()],
            ..Default::default()
        }),
        ..Default::default()
    };
    let (id, params) = connection.initialize_start()?;
    let params: InitializeParams = serde_json::from_value(params)?;
    let result = serde_json::json!({
        "capabilities": capabilities,
        "serverInfo": ServerInfo {
            name: "dsty".into(),
            version: Some(env!("CARGO_PKG_VERSION").into()),
        },
    });
    connection.initialize_finish(id, result)?;

    let workspace = workspace_root(&params);
    let base = workspace
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();
    let init_config = params
        .initialization_options
        .clone()
        .map(|v| parse_settings(v, &base))
        .transpose()
        .unwrap_or_else(|err| {
            log(
                &connection.sender,
                MessageType::ERROR,
                format!("initializationOptions: {err}"),
            );
            None
        })
        .unwrap_or_default();
    // «Виправити все» рахується лише при виборі дії, якщо клієнт уміє
    // `codeAction/resolve`
    let resolve = params
        .capabilities
        .text_document
        .as_ref()
        .and_then(|t| t.code_action.as_ref())
        .and_then(|c| c.resolve_support.as_ref())
        .is_some_and(|r| r.properties.iter().any(|p| p == "edit"));
    let progress = params
        .capabilities
        .window
        .as_ref()
        .and_then(|w| w.work_done_progress)
        .unwrap_or(false);

    let generation = Arc::new(AtomicU64::new(0));
    let published: Published = Arc::default();
    let facts: FullFacts = Arc::default();
    let docs: Docs = Arc::default();
    let (jobs, rx) = crossbeam_channel::unbounded();
    let worker = {
        let sender = connection.sender.clone();
        let generation = generation.clone();
        let published = published.clone();
        let facts = facts.clone();
        std::thread::spawn(move || worker(rx, sender, generation, published, facts))
    };
    let (full_jobs, full_rx) = crossbeam_channel::unbounded::<FullJob>();
    let full_worker = {
        let ctx = FullWorker {
            sender: connection.sender.clone(),
            jobs: jobs.clone(),
            generation: generation.clone(),
            facts: facts.clone(),
            docs: docs.clone(),
            progress,
            next_token: 0,
        };
        std::thread::spawn(move || ctx.run(full_rx))
    };

    let mut server = Server {
        sender: connection.sender.clone(),
        jobs,
        full_jobs,
        generation,
        docs,
        base,
        init_config,
        client_config: Config::default(),
        file_config: None,
        settings: Config::default(),
        options: Arc::new(Options::default()),
        configured: None,
        last_opened: None,
        resolve,
        published,
        reported: None,
    };
    server.reload();

    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    break;
                }
                let id = req.id.clone();
                let method = req.method.clone();
                // Паніка в обробнику не має зупинити сервер
                let resp = std::panic::catch_unwind(AssertUnwindSafe(|| server.request(req)))
                    .unwrap_or_else(|panic| {
                        let msg = panic_message(&panic);
                        log(
                            &server.sender,
                            MessageType::ERROR,
                            format!("{method}: внутрішня помилка: {msg}"),
                        );
                        Response::new_err(id, lsp_server::ErrorCode::InternalError as i32, msg)
                    });
                connection.sender.send(resp.into())?;
            }
            Message::Notification(note) => {
                let method = note.method.clone();
                if let Err(panic) =
                    std::panic::catch_unwind(AssertUnwindSafe(|| server.notification(note)))
                {
                    log(
                        &server.sender,
                        MessageType::ERROR,
                        format!("{method}: внутрішня помилка: {}", panic_message(&panic)),
                    );
                }
            }
            // Відповіді на `window/workDoneProgress/create`
            Message::Response(_) => {}
        }
    }

    // Компіляція, що йде, не має пережити сервер; потік запису stdout
    // завершується, коли закрито всі відправники
    full::cancel_all();
    drop(server);
    full_worker.join().ok();
    worker.join().ok();
    drop(connection);
    io_threads.join()?;
    Ok(())
}

fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "паніка".into())
}

/// Налаштування редактора: сам об'єкт або вкладений у `dsty`;
/// відносні шляхи відраховуються від робочої теки.
fn parse_settings(value: serde_json::Value, base: &Path) -> anyhow::Result<Config> {
    // Клієнт може надіслати налаштування всіх серверів (`{"dsty": …,
    // "tinymist": …}`): беремо свої
    let value = match value {
        serde_json::Value::Object(mut map)
            if map.get("dsty").is_some_and(serde_json::Value::is_object) =>
        {
            map.remove("dsty").unwrap_or_default()
        }
        serde_json::Value::Null => return Ok(Config::default()),
        v => v,
    };
    let mut config: Config = serde_json::from_value(value)?;
    config.resolve_paths(base);
    Ok(config)
}

struct Server {
    sender: Sender<Message>,
    jobs: Sender<Job>,
    full_jobs: Sender<FullJob>,
    generation: Arc<AtomicU64>,
    docs: Docs,
    /// Робоча тека (чи поточна): від неї відраховуються відносні шляхи налаштувань редактора.
    base: PathBuf,
    init_config: Config,
    client_config: Config,
    /// Файл налаштувань проєкту й час його зміни.
    file_config: Option<(PathBuf, Option<SystemTime>, Config)>,
    /// Чинні налаштування: файл, потім редактор.
    settings: Config,
    options: Arc<Options>,
    /// Main-файл з налаштувань або знайдений у робочій теці.
    configured: Option<Target>,
    /// Останній відкритий файл Typst і його корінь: main, якщо main не задано.
    last_opened: Option<(PathBuf, PathBuf)>,
    /// Клієнт уміє `codeAction/resolve`.
    resolve: bool,
    published: Published,
    /// Що вже написано в журнал про налаштування (щоб не повторювати).
    reported: Option<String>,
}

impl Server {
    /// Перечитує налаштування: файл проєкту (якщо змінився) і редактор.
    fn reload(&mut self) {
        // Файл налаштувань шукається від теки main-документа (з налаштувань
        // редактора чи знайденого в робочій теці), інакше від відкритого
        // файлу, інакше від робочої теки
        let editor = self.init_config.clone().merged(self.client_config.clone());
        let start = configured_target(&editor, &self.base)
            .and_then(|t| t.main.parent().map(Path::to_path_buf))
            .or_else(|| {
                self.last_opened
                    .as_ref()
                    .and_then(|(p, _)| p.parent().map(Path::to_path_buf))
            })
            .unwrap_or_else(|| self.base.clone());
        let path = Config::discover(&start);
        let modified = path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
        let fresh = match (&self.file_config, &path) {
            (Some((old, time, _)), Some(new)) => old != new || *time != modified,
            (None, None) => false,
            _ => true,
        };
        if fresh {
            self.file_config = match &path {
                Some(path) => match Config::load(path) {
                    Ok(config) => Some((path.clone(), modified, config)),
                    Err(err) => {
                        log(&self.sender, MessageType::ERROR, format!("{err:#}"));
                        Some((path.clone(), modified, Config::default()))
                    }
                },
                None => None,
            };
        }
        let settings = self
            .file_config
            .as_ref()
            .map(|(_, _, c)| c.clone())
            .unwrap_or_default()
            .merged(self.init_config.clone())
            .merged(self.client_config.clone());
        if settings == self.settings && !fresh && self.reported.is_some() {
            return;
        }
        let options = settings.options();
        let mut report = Vec::new();
        if let Err(err) = lint::validate(&options) {
            log(&self.sender, MessageType::ERROR, format!("{err:#}"));
        }
        self.configured = configured_target(&settings, &self.base);
        match &self.configured {
            Some(t) => report.push(format!(
                "main: {}, root: {}",
                t.main.display(),
                t.root.display()
            )),
            None => report.push(
                "main-файл не задано й не знайдено в робочій теці (src/main.typ, main.typ): беру main відкритого файлу"
                    .into(),
            ),
        }
        if let Some((path, _, _)) = &self.file_config {
            report.push(format!("налаштування: {}", path.display()));
        }
        report.push(format!(
            "повна перевірка: {}",
            if settings.full == Some(true) {
                "так"
            } else {
                "ні"
            }
        ));
        let report = report.join("; ");
        if self.reported.as_ref() != Some(&report) {
            log(&self.sender, MessageType::INFO, report.clone());
            self.reported = Some(report);
        }
        self.options = Arc::new(options);
        self.settings = settings;
    }

    fn request(&mut self, req: Request) -> Response {
        fn parse<T: serde::de::DeserializeOwned>(req: &Request) -> Result<T, Response> {
            serde_json::from_value(req.params.clone()).map_err(|err| {
                Response::new_err(
                    req.id.clone(),
                    lsp_server::ErrorCode::InvalidParams as i32,
                    err.to_string(),
                )
            })
        }
        let result = match req.method.as_str() {
            CodeActionRequest::METHOD => {
                parse::<CodeActionParams>(&req).map(|p| serde_json::json!(self.code_actions(&p)))
            }
            CodeActionResolveRequest::METHOD => {
                parse::<CodeAction>(&req).map(|a| serde_json::json!(self.resolve_action(a)))
            }
            HoverRequest::METHOD => {
                parse::<HoverParams>(&req).map(|p| serde_json::json!(self.hover(&p)))
            }
            Completion::METHOD => {
                parse::<CompletionParams>(&req).map(|p| serde_json::json!(self.completion(&p)))
            }
            ExecuteCommand::METHOD => parse::<ExecuteCommandParams>(&req).map(|p| {
                if p.command == FULL_CHECK {
                    self.schedule_full(true);
                }
                serde_json::Value::Null
            }),
            _ => {
                return Response::new_err(
                    req.id,
                    lsp_server::ErrorCode::MethodNotFound as i32,
                    format!("unsupported request {}", req.method),
                );
            }
        };
        match result {
            Ok(value) => Response::new_ok(req.id, value),
            Err(resp) => resp,
        }
    }

    fn code_actions(&self, params: &CodeActionParams) -> Vec<CodeActionOrCommand> {
        let uri = &params.text_document.uri;
        let Some(path) = uri_path(uri) else {
            return Vec::new();
        };
        // `only` клієнта: `source.fixAll` при збереженні, `quickfix` тощо
        let only = params.context.only.as_deref();
        let wants = |kind: &CodeActionKind| {
            only.is_none_or(|only| {
                only.iter().any(|k| {
                    kind.as_str() == k.as_str()
                        || kind.as_str().starts_with(&format!("{}.", k.as_str()))
                })
            })
        };
        let text = self.text(&path);
        let mut actions = Vec::new();
        let published = lock(&self.published);
        let diagnostics = published.by_path.get(&path).cloned().unwrap_or_default();
        let is_fixable = |d: &&lint::Diagnostic| d.fixes.iter().any(|f| f.is_correction);
        // Інші файли документа, які є що виправити
        let elsewhere = published.document.contains(&path)
            && published.by_path.iter().any(|(p, diags)| {
                *p != path && published.document.contains(p) && diags.iter().any(|d| is_fixable(&d))
            });
        drop(published);
        let want = params.range;
        for diag in diagnostics
            .iter()
            .filter(|_| wants(&CodeActionKind::QUICKFIX))
        {
            let lsp_diag = to_lsp(diag);
            if !overlaps(lsp_diag.range, want) {
                continue;
            }
            for (n, fix) in diag.fixes.iter().enumerate() {
                // Файл змінився з часу перевірки: виправлення влучили б не туди
                if fix
                    .edits
                    .iter()
                    .any(|e| text.get(e.range.clone()) != Some(e.old.as_str()))
                {
                    continue;
                }
                let edits = fix
                    .edits
                    .iter()
                    .map(|e| TextEdit {
                        range: Range::new(pos(e.start), pos(e.end)),
                        new_text: e.new.clone(),
                    })
                    .collect();
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: fix.title.clone(),
                    kind: Some(CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![lsp_diag.clone()]),
                    edit: Some(WorkspaceEdit {
                        changes: Some([(uri.clone(), edits)].into()),
                        ..Default::default()
                    }),
                    is_preferred: Some(n == 0 && fix.is_correction),
                    ..Default::default()
                }));
            }
        }
        // Усі виправлення файлу одним кроком, разом із тими, що відкриваються
        // після попередніх (як `check --fix`). Дія з'являється, лише якщо є що
        // виправляти, а рахується при виборі дії (`codeAction/resolve`). Серед
        // швидких виправлень вона є, коли зауважень хоч два (одне вже має окрему
        // дію), а `source.fixAll` при збереженні виправляє й одне
        let fixable = diagnostics.iter().filter(is_fixable).count();
        let (kind, least) = if wants(&CodeActionKind::QUICKFIX) {
            (CodeActionKind::QUICKFIX, 2)
        } else {
            (CodeActionKind::SOURCE_FIX_ALL, 1)
        };
        if fixable >= least && wants(&kind) {
            actions.extend(self.fix_all_action(uri, FixScope::File, kind));
        }
        // Увесь документ від main з підключеними файлами, як
        // `check --fix`. Не `source.fixAll`: при збереженні не правити інших
        // файлів
        let document = CodeActionKind::new(FIX_DOCUMENT);
        if elsewhere && wants(&document) {
            actions.extend(self.fix_all_action(uri, FixScope::Document, document));
        }
        // Повна перевірка на вимогу (працює й без `full = true`)
        let source = CodeActionKind::new("source.dsty.fullCheck");
        if is_typst(&path) && wants(&source) {
            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: "dsty: повна перевірка документа зараз".into(),
                kind: Some(source),
                command: Some(Command {
                    title: "Повна перевірка".into(),
                    command: FULL_CHECK.into(),
                    arguments: None,
                }),
                ..Default::default()
            }));
        }
        actions
    }

    /// Текст файлу: відкритий буфер або диск.
    fn text(&self, path: &Path) -> String {
        match lock(&self.docs).get(path) {
            Some(buffer) => buffer.text.to_string(),
            None => std::fs::read_to_string(path).unwrap_or_default(),
        }
    }

    /// «Виправити все» у файлі чи документі: заміна рахується зараз або,
    /// якщо клієнт уміє, лише при виборі дії.
    fn fix_all_action(
        &self,
        uri: &Url,
        scope: FixScope,
        kind: CodeActionKind,
    ) -> Option<CodeActionOrCommand> {
        let action = CodeAction {
            title: scope.title().into(),
            kind: Some(kind),
            data: Some(serde_json::json!({ "uri": uri, "scope": scope })),
            ..Default::default()
        };
        let action = if self.resolve {
            action
        } else {
            Some(self.resolve_action(action)).filter(|a| a.edit.is_some())?
        };
        Some(CodeActionOrCommand::CodeAction(action))
    }

    /// Рахує заміну для «виправити все».
    fn resolve_action(&self, mut action: CodeAction) -> CodeAction {
        let data = action.data.as_ref();
        let uri = data
            .and_then(|d| d.get("uri"))
            .and_then(|u| serde_json::from_value::<Url>(u.clone()).ok());
        let scope = data
            .and_then(|d| d.get("scope"))
            .and_then(|s| serde_json::from_value::<FixScope>(s.clone()).ok())
            .unwrap_or(FixScope::File);
        if let Some(uri) = uri
            && let Some(path) = uri_path(&uri)
            && let Some((count, edit)) = self.fix_all_edit(&path, scope)
        {
            action.title = format!("{} ({count})", scope.title());
            action.edit = Some(edit);
        }
        action
    }

    fn fix_all_edit(&self, path: &Path, scope: FixScope) -> Option<(usize, WorkspaceEdit)> {
        let target = self.target()?;
        let docs = lock(&self.docs).clone();
        let load = |p: &Path| {
            docs.get(p)
                .map(|b| b.text.to_string())
                .or_else(|| std::fs::read_to_string(p).ok())
        };
        let in_document = lock(&self.published).document.contains(path);
        let fixed = match scope {
            FixScope::File => {
                let only = Some(path);
                let fixed =
                    lint::fix_all(&target.root, &target.main, &load, &self.options, only).ok()?;
                if fixed.count > 0 || in_document {
                    fixed
                } else {
                    // Файл, якого main не підключає, є окремим документом, як у
                    // перевірці
                    let root = orphan_root(path, &target.root);
                    lint::fix_all(&root, path, &load, &self.options, only).ok()?
                }
            }
            FixScope::Document => {
                lint::fix_all(&target.root, &target.main, &load, &self.options, None).ok()?
            }
        };
        if fixed.count == 0 {
            return None;
        }
        // Кожен файл змінюється однією заміною всього тексту: виправлення кількох
        // проходів у позиціях початкового тексту не виразити
        let mut changes = HashMap::new();
        for (file, new) in &fixed.files {
            let text = load(file).unwrap_or_default();
            let Ok(uri) = Url::from_file_path(file) else {
                continue;
            };
            let lines: Vec<&str> = text.split('\n').collect();
            let end = Position::new(
                (lines.len() - 1) as u32,
                lines.last().map_or(0, |l| l.encode_utf16().count()) as u32,
            );
            let edit = TextEdit {
                range: Range::new(Position::new(0, 0), end),
                new_text: new.clone(),
            };
            changes.insert(uri, vec![edit]);
        }
        Some((
            fixed.count,
            WorkspaceEdit {
                changes: Some(changes),
                ..Default::default()
            },
        ))
    }

    /// Підказка: суть правила й текст пунктів стандарту для зауважень під
    /// курсором або для назви правила в коментарі `dstu-ignore`.
    fn hover(&self, params: &HoverParams) -> Option<Hover> {
        let at = &params.text_document_position_params;
        let path = uri_path(&at.text_document.uri)?;
        let position = at.position;
        let published = lock(&self.published);
        let here: Vec<lint::Diagnostic> = published
            .by_path
            .get(&path)
            .into_iter()
            .flatten()
            .filter(|d| {
                let r = Range::new(pos(d.start), pos(d.end));
                overlaps(r, Range::new(position, position))
            })
            .cloned()
            .collect();
        drop(published);
        let mut parts = Vec::new();
        let mut range = None;
        let mut seen = HashSet::new();
        for d in &here {
            range.get_or_insert(Range::new(pos(d.start), pos(d.end)));
            if let Some(rule) = rules::find(d.rule)
                && seen.insert(d.rule)
            {
                parts.push(rule.explain());
            }
        }
        // Назва правила в коментарі `dstu-ignore`
        let text = self.text(&path);
        if let Some(line) = text.lines().nth(position.line as usize) {
            let (start, end) = rule_word(line, utf16_to_byte(line, position.character as usize));
            if line[..start].contains("dstu-ignore")
                && let Some(rule) = rules::find(&line[start..end])
                && seen.insert(rule.code)
            {
                parts.insert(0, rule.explain());
                range = Some(Range::new(
                    Position::new(position.line, line[..start].encode_utf16().count() as u32),
                    Position::new(position.line, line[..end].encode_utf16().count() as u32),
                ));
            }
        }
        if parts.is_empty() {
            return None;
        }
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: parts.join("\n\n---\n\n"),
            }),
            range,
        })
    }

    /// Доповнення в коментарях: `// dstu-` -> вид коментаря, після `:` ->
    /// назви правил.
    fn completion(&self, params: &CompletionParams) -> Option<CompletionResponse> {
        let at = &params.text_document_position;
        let path = uri_path(&at.text_document.uri)?;
        let text = self.text(&path);
        let line = text.lines().nth(at.position.line as usize).unwrap_or("");
        let prefix = &line[..utf16_to_byte(line, at.position.character as usize)];
        let comment = &prefix[prefix.rfind("//")? + 2..];
        let body = comment.trim_start();
        let items = if let Some(rest) = body.strip_prefix("dstu-ignore") {
            // Після виду коментаря й двокрапки пропонуються назви правил, яких ще немає
            let (_, list) = rest.split_once(':')?;
            let listed: Vec<&str> = list
                .split([',', ' '])
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();
            rules::RULES
                .iter()
                .filter(|r| !r.code.starts_with("ignore-") && !listed.contains(&r.code))
                .map(|r| CompletionItem {
                    label: r.code.into(),
                    kind: Some(CompletionItemKind::VALUE),
                    detail: Some(r.desc.into()),
                    documentation: Some(Documentation::MarkupContent(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: r.explain(),
                    })),
                    ..Default::default()
                })
                .collect()
        } else if body.len() >= 2 && "dstu-ignore".starts_with(body)
            || body.starts_with("dstu") && !body.contains(':')
        {
            [
                (
                    "dstu-ignore-next-line",
                    "вимкнути правила для наступного рядка",
                ),
                ("dstu-ignore", "вимкнути правила для цього рядка"),
                ("dstu-ignore-file", "вимкнути правила для всього файлу"),
            ]
            .into_iter()
            .map(|(label, detail)| CompletionItem {
                label: label.into(),
                kind: Some(CompletionItemKind::KEYWORD),
                detail: Some(detail.into()),
                insert_text: Some(format!("{label}: ")),
                ..Default::default()
            })
            .collect()
        } else {
            return None;
        };
        Some(CompletionResponse::Array(items))
    }

    fn notification(&mut self, note: Notification) {
        match note.method.as_str() {
            DidOpenTextDocument::METHOD => {
                let Ok(p) =
                    serde_json::from_value::<lsp_types::DidOpenTextDocumentParams>(note.params)
                else {
                    return;
                };
                let Some(path) = uri_path(&p.text_document.uri) else {
                    return;
                };
                let typst = is_typst(&path);
                let text: Arc<str> = Arc::from(p.text_document.text);
                if typst && self.configured.is_none() {
                    // Розділ роботи (`src/chapters/1.typ`) перевіряється в
                    // документі, до якого належить
                    let main = guess_main(&path, &self.base).unwrap_or_else(|| path.clone());
                    let root = model::detect_root(&main, &|p: &Path| {
                        if p == path {
                            Some(text.to_string())
                        } else {
                            std::fs::read_to_string(p).ok()
                        }
                    });
                    if self.last_opened.as_ref().is_none_or(|(m, _)| *m != main) {
                        log(
                            &self.sender,
                            MessageType::INFO,
                            format!("main: {}, root: {}", main.display(), root.display()),
                        );
                    }
                    self.last_opened = Some((main, root));
                }
                lock(&self.docs).insert(
                    path,
                    Buffer {
                        text,
                        version: Some(p.text_document.version),
                    },
                );
                self.reload();
                self.schedule(Duration::ZERO);
                if typst {
                    self.schedule_full(false);
                }
            }
            DidChangeTextDocument::METHOD => {
                let Ok(p) =
                    serde_json::from_value::<lsp_types::DidChangeTextDocumentParams>(note.params)
                else {
                    return;
                };
                let Some(path) = uri_path(&p.text_document.uri) else {
                    return;
                };
                // Синхронізація повна: остання зміна містить весь текст
                if let Some(change) = p.content_changes.into_iter().last() {
                    lock(&self.docs).insert(
                        path,
                        Buffer {
                            text: Arc::from(change.text),
                            version: Some(p.text_document.version),
                        },
                    );
                }
                if self.settings.check_on_change.unwrap_or(true) {
                    self.schedule(self.debounce());
                }
            }
            DidSaveTextDocument::METHOD => {
                self.reload();
                self.schedule(Duration::ZERO);
                self.schedule_full(false);
            }
            DidCloseTextDocument::METHOD => {
                let Ok(p) =
                    serde_json::from_value::<lsp_types::DidCloseTextDocumentParams>(note.params)
                else {
                    return;
                };
                if let Some(path) = uri_path(&p.text_document.uri) {
                    lock(&self.docs).remove(&path);
                }
                self.schedule(self.debounce());
            }
            DidChangeConfiguration::METHOD => {
                let Ok(p) =
                    serde_json::from_value::<lsp_types::DidChangeConfigurationParams>(note.params)
                else {
                    return;
                };
                match parse_settings(p.settings, &self.base) {
                    Ok(config) => self.client_config = config,
                    Err(err) => log(
                        &self.sender,
                        MessageType::ERROR,
                        format!("didChangeConfiguration: {err}"),
                    ),
                }
                self.reload();
                self.schedule(Duration::ZERO);
                self.schedule_full(false);
            }
            _ => {}
        }
    }

    fn debounce(&self) -> Duration {
        Duration::from_millis(self.settings.debounce.unwrap_or(150))
    }

    fn target(&self) -> Option<Target> {
        if let Some(target) = &self.configured {
            return Some(target.clone());
        }
        let (main, root) = self.last_opened.clone()?;
        let root = match &self.settings.root {
            Some(root) => canonical(root),
            None => root,
        };
        Some(Target {
            root,
            main,
            guessed: true,
        })
    }

    fn schedule(&self, delay: Duration) {
        let Some(target) = self.target() else { return };
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let job = Job {
            generation,
            delay,
            target,
            overlays: lock(&self.docs).clone(),
            options: self.options.clone(),
        };
        if self.jobs.send(job).is_err() {
            log(
                &self.sender,
                MessageType::ERROR,
                "потік перевірки зупинився".into(),
            );
        }
    }

    /// Повна перевірка документа: якщо ввімкнена (чи запущена командою, `forced`) і
    /// є що перевіряти.
    fn schedule_full(&self, forced: bool) {
        if !forced && self.settings.full != Some(true) {
            return;
        }
        let Some(target) = self.target() else { return };
        let _ = self.full_jobs.send(FullJob {
            target,
            options: self.options.clone(),
            typst: self.settings.typst.clone(),
        });
    }
}

/// Потік повної перевірки: компілює документ, кешує факти й просить
/// статичну перевірку опублікувати зауваження з ними.
struct FullWorker {
    sender: Sender<Message>,
    jobs: Sender<Job>,
    generation: Arc<AtomicU64>,
    facts: FullFacts,
    docs: Docs,
    /// Клієнт показує `$/progress`.
    progress: bool,
    next_token: u64,
}

impl FullWorker {
    fn run(mut self, rx: Receiver<FullJob>) {
        let mut last_error: Option<String> = None;
        while let Ok(mut job) = rx.recv() {
            // Кілька збережень поспіль дають одну компіляцію
            while let Ok(newer) = rx.try_recv() {
                job = newer;
            }
            if !full::wanted(&job.options) {
                continue;
            }
            let settings = full::Settings {
                typst: job.typst.clone(),
                root: Some(job.target.root.clone()),
                timeout: None,
            };
            let token = self.begin_progress(&job.target.main);
            let started = Instant::now();
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                full::collect(&job.target.main, &settings, &job.options)
            }))
            .unwrap_or_else(|panic| Err(anyhow::anyhow!("паніка: {}", panic_message(&panic))));
            let elapsed = started.elapsed().as_millis();
            match result {
                Ok(facts) => {
                    log(
                        &self.sender,
                        MessageType::LOG,
                        format!(
                            "повна перевірка {}: {elapsed} мс",
                            job.target.main.display()
                        ),
                    );
                    last_error = None;
                    lock(&self.facts).insert(job.target.main.clone(), Arc::new(facts));
                    self.end_progress(token, format!("{elapsed} мс"));
                }
                Err(err) => {
                    // Документ не компілюється: лишаються статичні зауваження
                    let msg = format!("повну перевірку пропущено: {err:#}");
                    if last_error.as_ref() != Some(&msg) {
                        log(&self.sender, MessageType::WARNING, msg.clone());
                    }
                    last_error = Some(msg);
                    lock(&self.facts).remove(&job.target.main);
                    self.end_progress(token, "пропущено: документ не компілюється".into());
                }
            }
            let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
            let job = Job {
                generation,
                delay: Duration::ZERO,
                target: job.target,
                overlays: lock(&self.docs).clone(),
                options: job.options,
            };
            if self.jobs.send(job).is_err() {
                return;
            }
        }
    }

    /// `$/progress` повної перевірки, якщо клієнт його показує.
    fn begin_progress(&mut self, main: &Path) -> Option<NumberOrString> {
        if !self.progress {
            return None;
        }
        self.next_token += 1;
        let token = NumberOrString::String(format!("dsty-full-{}", self.next_token));
        let create = Request::new(
            RequestId::from(format!("dsty-progress-{}", self.next_token)),
            WorkDoneProgressCreate::METHOD.into(),
            WorkDoneProgressCreateParams {
                token: token.clone(),
            },
        );
        let _ = self.sender.send(create.into());
        let name = main
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.send_progress(
            &token,
            WorkDoneProgress::Begin(WorkDoneProgressBegin {
                title: "dsty: повна перевірка".into(),
                message: Some(name),
                cancellable: Some(false),
                percentage: None,
            }),
        );
        Some(token)
    }

    fn end_progress(&self, token: Option<NumberOrString>, message: String) {
        if let Some(token) = token {
            self.send_progress(
                &token,
                WorkDoneProgress::End(WorkDoneProgressEnd {
                    message: Some(message),
                }),
            );
        }
    }

    fn send_progress(&self, token: &NumberOrString, value: WorkDoneProgress) {
        let params = ProgressParams {
            token: token.clone(),
            value: ProgressParamsValue::WorkDone(value),
        };
        let _ = self
            .sender
            .send(Notification::new(Progress::METHOD.into(), params).into());
    }
}

fn worker(
    rx: Receiver<Job>,
    sender: Sender<Message>,
    generation: Arc<AtomicU64>,
    published: Published,
    facts: FullFacts,
) {
    let mut shown: HashSet<Url> = HashSet::new();

    while let Ok(mut job) = rx.recv() {
        // Debounce: поки приходять новіші запити, чекаємо далі
        let mut deadline = Instant::now() + job.delay;
        loop {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(newer) => {
                    deadline = Instant::now() + newer.delay;
                    job = newer;
                }
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }

        let started = Instant::now();
        // Паніка на дивному документі не має зупинити потік перевірки
        let checked = std::panic::catch_unwind(AssertUnwindSafe(|| check_job(&job, &facts)));
        if generation.load(Ordering::SeqCst) != job.generation {
            continue;
        }
        let (by_path, document, logs) = match checked {
            Ok(result) => result,
            Err(panic) => {
                log(
                    &sender,
                    MessageType::ERROR,
                    format!("перевірка: внутрішня помилка: {}", panic_message(&panic)),
                );
                continue;
            }
        };
        for (typ, msg) in logs {
            log(&sender, typ, msg);
        }
        log(
            &sender,
            MessageType::LOG,
            format!(
                "{}: зауважень {}, {} мс",
                job.target.main.display(),
                by_path.values().map(Vec::len).sum::<usize>(),
                started.elapsed().as_millis()
            ),
        );

        let mut by_file: HashMap<Url, (Option<i32>, Vec<Diagnostic>)> = HashMap::new();
        // Відкриті документи публікуються завжди, навіть без зауважень: редактор знає,
        // що їх перевірено
        for (path, buffer) in &job.overlays {
            if is_typst(path)
                && let Ok(uri) = Url::from_file_path(path)
            {
                by_file.insert(uri, (buffer.version, Vec::new()));
            }
        }
        for (path, diags) in &by_path {
            let Ok(uri) = Url::from_file_path(path) else {
                continue;
            };
            let version = job.overlays.get(path).and_then(|b| b.version);
            let entry = by_file.entry(uri).or_insert((version, Vec::new()));
            entry.1.extend(diags.iter().map(to_lsp));
        }
        *lock(&published) = Checked { by_path, document };

        // Файли, у яких зауваження зникли, очищаються
        for uri in shown.drain().collect::<Vec<_>>() {
            by_file.entry(uri).or_insert((None, Vec::new()));
        }
        for (uri, (version, diagnostics)) in by_file {
            if !diagnostics.is_empty() {
                shown.insert(uri.clone());
            }
            let params = PublishDiagnosticsParams {
                uri,
                diagnostics,
                version,
            };
            let note = Notification::new(PublishDiagnostics::METHOD.into(), params);
            if sender.send(note.into()).is_err() {
                return;
            }
        }
    }
}

type Logs = Vec<(MessageType, String)>;

/// Статична перевірка (з фактами повної, якщо є) документа й відкритих
/// файлів, яких main не підключає. Зауваження за файлом і записи для журналу.
fn check_job(
    job: &Job,
    facts: &FullFacts,
) -> (
    HashMap<PathBuf, Vec<lint::Diagnostic>>,
    HashSet<PathBuf>,
    Logs,
) {
    let target = &job.target;
    let overlays = &job.overlays;
    let options = &*job.options;
    let load = |path: &Path| {
        overlays
            .get(path)
            .map(|b| b.text.to_string())
            .or_else(|| std::fs::read_to_string(path).ok())
    };
    let mut logs: Logs = Vec::new();
    let full = lock(facts).get(&target.main).cloned();
    let result = lint::check_full(&target.root, &target.main, &load, options, full.as_deref());

    let mut by_path: HashMap<PathBuf, Vec<lint::Diagnostic>> = HashMap::new();
    let document: HashSet<PathBuf> = result
        .as_ref()
        .map(|r| r.files.iter().cloned().collect())
        .unwrap_or_default();
    let mut covered = document.clone();
    // Відкриті файли, яких main не підключає (чернетка, стара версія),
    // перевіряються як окремі документи
    let mut orphans: Vec<&PathBuf> = overlays
        .keys()
        .filter(|p| is_typst(p) && !covered.contains(*p))
        .collect();
    orphans.sort();
    for path in orphans {
        if covered.contains(path) {
            continue;
        }
        let root = orphan_root(path, &target.root);
        match lint::check(&root, path, &load, options) {
            Ok(report) => {
                for diag in report.diagnostics {
                    if !covered.contains(&diag.path) {
                        by_path.entry(diag.path.clone()).or_default().push(diag);
                    }
                }
                covered.extend(report.files);
            }
            Err(err) => logs.push((MessageType::WARNING, format!("{err:#}"))),
        }
    }
    match result {
        Ok(report) => {
            for (file, pos, path) in &report.missing {
                logs.push((
                    MessageType::WARNING,
                    format!(
                        "{}:{}: немає файлу {}",
                        file.display(),
                        pos.line + 1,
                        path.display()
                    ),
                ));
            }
            for diag in report.diagnostics {
                by_path.entry(diag.path.clone()).or_default().push(diag);
            }
        }
        Err(err) => {
            logs.push((MessageType::WARNING, format!("{err:#}")));
            if !target.guessed {
                // Зауваження без правила: лише показати помилку на main-файлі
                by_path
                    .entry(target.main.clone())
                    .or_default()
                    .push(error_diagnostic(&target.main, &format!("dsty: {err:#}")));
            }
        }
    }
    (by_path, document, logs)
}

/// Корінь для відкритого файлу, якого main не підключає: корінь документа,
/// якщо файл у ньому, інакше тека файлу.
fn orphan_root(path: &Path, root: &Path) -> PathBuf {
    if path.starts_with(root) {
        root.to_path_buf()
    } else {
        path.parent()
            .map_or_else(|| root.to_path_buf(), Path::to_path_buf)
    }
}

/// Main-файл документа, до якого, найпевніше, належить відкритий файл:
/// `src/main.typ` чи `main.typ` у його теці або вище, не вище робочої теки
/// й теки проєкту (`typst.toml`, `.git`, `.dsty.toml`). Якщо main його не
/// підключає, файл однаково перевіряється окремо.
fn guess_main(path: &Path, base: &Path) -> Option<PathBuf> {
    let inside = path.starts_with(base);
    for dir in path.ancestors().skip(1) {
        for candidate in config::MAIN_FILES {
            let main = dir.join(candidate);
            if main.is_file() {
                return Some(canonical(&main));
            }
        }
        let project = ["typst.toml", ".git"]
            .iter()
            .chain(config::FILE_NAMES)
            .any(|m| dir.join(m).exists());
        if project || (inside && dir == base) {
            break;
        }
    }
    None
}

/// Помилка перевірки як зауваження на початку main-файлу.
fn error_diagnostic(path: &Path, msg: &str) -> lint::Diagnostic {
    lint::Diagnostic {
        path: path.to_path_buf(),
        range: 0..0,
        start: lint::Pos::default(),
        end: lint::Pos::default(),
        rule: "",
        clause: "",
        msg: msg.to_owned(),
        severity: Severity::Info,
        unnecessary: false,
        related: Vec::new(),
        context: None,
        fixes: Vec::new(),
    }
}

fn pos(p: lint::Pos) -> Position {
    Position::new(p.line as u32, p.utf16 as u32)
}

fn overlaps(a: Range, b: Range) -> bool {
    let key = |p: Position| (p.line, p.character);
    key(a.start) <= key(b.end) && key(b.start) <= key(a.end)
}

/// Межі назви правила (`[a-z0-9-]`) навколо байта `col`. Межі завжди
/// припадають на межі символів, навіть коли поруч кирилиця.
fn rule_word(line: &str, col: usize) -> (usize, usize) {
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '-';
    let start = line[..col].trim_end_matches(is_word).len();
    let end = line.len() - line[col..].trim_start_matches(is_word).len();
    (start, end)
}

/// Байт у рядку за колонкою UTF-16.
fn utf16_to_byte(line: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (i, c) in line.char_indices() {
        if units >= utf16 {
            return i;
        }
        units += c.len_utf16();
    }
    line.len()
}

fn severity(s: Severity) -> DiagnosticSeverity {
    match s {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
        Severity::Info => DiagnosticSeverity::INFORMATION,
        Severity::Hint => DiagnosticSeverity::HINT,
    }
}

fn to_lsp(diag: &lint::Diagnostic) -> Diagnostic {
    let related: Vec<DiagnosticRelatedInformation> = diag
        .related
        .iter()
        .filter_map(|(path, start, end, note)| {
            Some(DiagnosticRelatedInformation {
                location: Location {
                    uri: Url::from_file_path(path).ok()?,
                    range: Range::new(pos(*start), pos(*end)),
                },
                message: note.clone(),
            })
        })
        .collect();
    Diagnostic {
        range: Range::new(pos(diag.start), pos(diag.end)),
        severity: Some(severity(diag.severity)),
        code: (!diag.rule.is_empty()).then(|| NumberOrString::String(diag.rule.into())),
        source: Some("dsty".into()),
        message: diag.message(),
        tags: diag.unnecessary.then(|| vec![DiagnosticTag::UNNECESSARY]),
        related_information: (!related.is_empty()).then_some(related),
        ..Default::default()
    }
}

fn log(sender: &Sender<Message>, typ: MessageType, message: String) {
    let params = LogMessageParams { typ, message };
    let _ = sender.send(Notification::new(LogMessage::METHOD.into(), params).into());
}

#[allow(deprecated, reason = "root_uri still sent by many clients")]
fn workspace_root(params: &InitializeParams) -> Option<PathBuf> {
    let uri = params
        .workspace_folders
        .as_ref()
        .and_then(|folders| folders.first())
        .map(|folder| &folder.uri)
        .or(params.root_uri.as_ref())?;
    uri_path(uri)
}

/// Main і корінь з налаштувань або за відомими шляхами в робочій теці.
fn configured_target(config: &Config, base: &Path) -> Option<Target> {
    let main = match &config.main {
        Some(main) => base.join(main),
        None => ["src/main.typ", "main.typ"]
            .iter()
            .map(|p| base.join(p))
            .find(|p| p.is_file())?,
    };
    let main = canonical(&main);
    let root = match &config.root {
        Some(root) => canonical(&base.join(root)),
        None => model::detect_root(&main, &|p: &Path| std::fs::read_to_string(p).ok()),
    };
    Some(Target {
        root,
        main,
        guessed: false,
    })
}

fn uri_path(uri: &Url) -> Option<PathBuf> {
    uri.to_file_path().ok().map(|p| canonical(&p))
}

/// Канонічний шлях; для ще не збереженого файлу канонічна тека й ім'я.
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize()
        .unwrap_or_else(|_| match (path.parent(), path.file_name()) {
            (Some(dir), Some(name)) => dir
                .canonicalize()
                .map_or_else(|_| path.to_path_buf(), |d| d.join(name)),
            _ => path.to_path_buf(),
        })
}

fn is_typst(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "typ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_is_guessed_upwards() {
        let dir = std::env::temp_dir().join(format!("dsty-guess-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let work = dir.join("course/pz1");
        std::fs::create_dir_all(work.join("src/chapters")).unwrap();
        std::fs::write(work.join("src/main.typ"), "").unwrap();
        std::fs::write(work.join("src/chapters/one.typ"), "").unwrap();
        let dir = canonical(&dir);
        let work = dir.join("course/pz1");
        let chapter = work.join("src/chapters/one.typ");
        assert_eq!(
            guess_main(&chapter, &dir.join("course")),
            Some(work.join("src/main.typ"))
        );
        // Не вище робочої теки
        assert_eq!(guess_main(&chapter, &work.join("src/chapters")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rule_word_on_cyrillic() {
        // Курсор одразу після кириличної літери: межа не всередині `я`
        let line = "Звітя";
        let col = utf16_to_byte(line, 5);
        assert_eq!(rule_word(line, col), (line.len(), line.len()));
        let line = "// dstu-ignore: я,heading-case тут";
        let col = utf16_to_byte(line, 20);
        let (start, end) = rule_word(line, col);
        assert_eq!(&line[start..end], "heading-case");
        for c in 0..=line.encode_utf16().count() {
            let (start, end) = rule_word(line, utf16_to_byte(line, c));
            let _ = &line[start..end];
        }
    }

    #[test]
    fn utf16_columns() {
        assert_eq!(utf16_to_byte("абв", 2), 4);
        assert_eq!(utf16_to_byte("a😀b", 3), 5);
        assert_eq!(utf16_to_byte("ab", 9), 2);
    }

    #[test]
    fn settings_nested_or_flat() {
        let base = Path::new("/w");
        let flat = parse_settings(
            serde_json::json!({ "main": "src/main.typ", "full": true }),
            base,
        )
        .unwrap();
        assert_eq!(flat.main.as_deref(), Some(Path::new("/w/src/main.typ")));
        let nested = parse_settings(serde_json::json!({ "dsty": { "full": true } }), base).unwrap();
        assert_eq!(nested.full, Some(true));
        // Налаштування всіх серверів разом
        let all = parse_settings(
            serde_json::json!({ "dsty": { "full": true }, "tinymist": { "x": 1 } }),
            base,
        )
        .unwrap();
        assert_eq!(all.full, Some(true));
    }
}
