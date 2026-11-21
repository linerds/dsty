//! Лінтер ДСТУ 3008:2015 для Typst: розбір вихідного коду без компіляції
//! ([`model`]), правила ([`rules`]), зауваження з виправленнями ([`lint`]),
//! необов'язкова повна перевірка через нативний `typst` ([`full`]) і
//! LSP-сервер ([`lsp`]).

pub mod config;
pub mod full;
pub mod lint;
pub mod lsp;
pub mod model;
pub mod render;
pub mod rules;
pub mod standard;
pub mod text;
