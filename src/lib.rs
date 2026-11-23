//! Лінтер ДСТУ 3008:2015 для Typst: розбір вихідного коду без компіляції
//! ([`model`]), відтворення поведінки typst і нейтральна модель роботи
//! ([`work`]), правила над нею ([`rules`]), зауваження з виправленнями ([`lint`]),
//! необов'язкова повна перевірка через нативний `typst` ([`full`]) і
//! LSP-сервер ([`lsp`]).

pub mod config;
pub mod defaults;
pub mod eval;
pub mod full;
pub mod lint;
pub mod look;
pub mod lsp;
pub mod model;
pub mod render;
pub mod rules;
pub mod standard;
pub mod style;
pub mod text;
pub mod work;
