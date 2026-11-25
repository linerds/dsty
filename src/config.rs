//! Налаштування проєкту: `.dsty.toml` (чи `dsty.toml`) у теці
//! документа або вище. Один файл діє і на `dsty check`, і на LSP-сервер,
//! тож його можна покласти й над теками всіх робіт курсу.
//!
//! ```toml
//! main = "src/main.typ"      # для LSP: main-файл документа
//! root = "."                 # корінь для шляхів `/…`
//! full = true                # повна перевірка нативним typst
//! typst = "/шлях/до/typst"
//! ignore = ["heading-single"]
//! enable = ["units", "font-style"]
//!
//! [severity]
//! list-punct = "hint"        # error, warning, info, hint або off
//! ```
//!
//! Відносні шляхи відраховуються від теки файлу налаштувань.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::Deserialize;

use crate::lint::{Options, Severity};

/// Імена файлу налаштувань у порядку пошуку.
pub const FILE_NAMES: &[&str] = &[".dsty.toml", "dsty.toml"];

/// Main-файл документа відносно його теки, у порядку пошуку.
pub const MAIN_FILES: &[&str] = &["src/main.typ", "main.typ"];

/// Рівень правила в налаштуваннях: як [`Severity`] або `off` (вимкнути).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Off,
    Hint,
    Info,
    Warning,
    Error,
}

impl Level {
    fn severity(self) -> Option<Severity> {
        Some(match self {
            Level::Off => return None,
            Level::Hint => Severity::Hint,
            Level::Info => Severity::Info,
            Level::Warning => Severity::Warning,
            Level::Error => Severity::Error,
        })
    }
}

/// Налаштування з файлу чи редактора. Порожнє поле означає, що значення не задано.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Config {
    /// Main-файл (для LSP).
    pub main: Option<PathBuf>,
    /// Корінь проєкту для шляхів `/…` (`typst --root`).
    pub root: Option<PathBuf>,
    pub ignore: Vec<String>,
    pub enable: Vec<String>,
    pub severity: HashMap<String, Level>,
    /// Повна перевірка нативним `typst`.
    pub full: Option<bool>,
    /// `typst` для повної перевірки.
    pub typst: Option<PathBuf>,
    /// LSP: затримка перевірки після зміни тексту, мс.
    pub debounce: Option<u64>,
    /// LSP: перевіряти під час набору, а не лише при відкритті й збереженні.
    #[serde(alias = "check-on-change")]
    pub check_on_change: Option<bool>,
}

impl Config {
    /// Шукає файл налаштувань від теки `start` угору.
    pub fn discover(start: &Path) -> Option<PathBuf> {
        let mut dir = Some(start);
        while let Some(d) = dir {
            for name in FILE_NAMES {
                let path = d.join(name);
                if path.is_file() {
                    return Some(path);
                }
            }
            dir = d.parent();
        }
        None
    }

    /// Читає файл налаштувань; відносні шляхи відраховуються від його теки.
    pub fn load(path: &Path) -> anyhow::Result<Config> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("читання {}", path.display()))?;
        let mut config: Config =
            toml::from_str(&text).with_context(|| format!("розбір {}", path.display()))?;
        config.resolve_paths(path.parent().unwrap_or(Path::new(".")));
        Ok(config)
    }

    /// Відносні шляхи відраховуються від теки `base`. Назву `typst` без теки
    /// (`typst-0.15`) шукають у `PATH`, тож її не чіпаємо.
    pub fn resolve_paths(&mut self, base: &Path) {
        let bare = self
            .typst
            .as_ref()
            .is_some_and(|t| t.components().count() == 1 && t.is_relative());
        let typst = if bare { None } else { self.typst.as_mut() };
        for p in [self.main.as_mut(), self.root.as_mut(), typst]
            .into_iter()
            .flatten()
        {
            if p.is_relative() {
                *p = base.join(&*p);
            }
        }
    }

    /// `other` поверх `self`: задані поля `other` заміняють, переліки
    /// доповнюються.
    pub fn merged(mut self, other: Config) -> Config {
        self.main = other.main.or(self.main);
        self.root = other.root.or(self.root);
        self.ignore.extend(other.ignore);
        self.enable.extend(other.enable);
        self.severity.extend(other.severity);
        self.full = other.full.or(self.full);
        self.typst = other.typst.or(self.typst);
        self.debounce = other.debounce.or(self.debounce);
        self.check_on_change = other.check_on_change.or(self.check_on_change);
        self
    }

    /// Правила для перевірки: `off` діє так само, як `ignore`.
    pub fn options(&self) -> Options {
        let mut options = Options {
            ignore: self.ignore.clone(),
            enable: self.enable.clone(),
            severity: HashMap::new(),
        };
        for (rule, level) in &self.severity {
            match level.severity() {
                Some(s) => {
                    options.severity.insert(rule.clone(), s);
                }
                None => options.ignore.push(rule.clone()),
            }
        }
        options
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_merges() {
        let file: Config = toml::from_str(
            "full = true\nignore = [\"heading-single\"]\n[severity]\nlist-punct = \"hint\"\nlist-case = \"off\"\n",
        )
        .unwrap();
        let editor = Config {
            ignore: vec!["list-colon".into()],
            full: Some(false),
            ..Default::default()
        };
        let merged = file.merged(editor);
        assert_eq!(merged.full, Some(false));
        let options = merged.options();
        assert_eq!(
            options.ignore,
            ["heading-single", "list-colon", "list-case"]
        );
        assert_eq!(options.severity["list-punct"], Severity::Hint);
        assert!(toml::from_str::<Config>("nope = 1\n").is_err());
    }

    #[test]
    fn relative_paths() {
        let mut config: Config =
            toml::from_str("main = \"src/main.typ\"\ntypst = \"typst-0.15\"\n").unwrap();
        config.resolve_paths(Path::new("/course"));
        assert_eq!(
            config.main.as_deref(),
            Some(Path::new("/course/src/main.typ"))
        );
        // Назву без теки шукаємо в PATH
        assert_eq!(config.typst.as_deref(), Some(Path::new("typst-0.15")));
        let mut config: Config = toml::from_str("typst = \"bin/typst\"\n").unwrap();
        config.resolve_paths(Path::new("/course"));
        assert_eq!(
            config.typst.as_deref(),
            Some(Path::new("/course/bin/typst"))
        );
    }
}
