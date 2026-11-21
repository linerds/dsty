//! Повна перевірка: необов'язковий шар над статичним. Документ компілює
//! нативний `typst` користувача (тож версія Typst і пакунків -- як у проєкті),
//! а вбудована обгортка з пробами збирає те, чого з коду не видно: сторінки,
//! ефективні стилі, відрендерені номери. Крейт `typst` не потрібен.
//!
//! Якщо typst не знайдено чи документ не компілюється, повна перевірка
//! пропускається, а статичні зауваження лишаються.

mod check;
pub mod facts;
mod run;

use std::path::Path;

pub use facts::Facts;
pub use run::{Sections, Settings, cancel_all, wrapper};

use crate::lint::{self, Options};
use crate::model::Doc;
use crate::rules::{Found, RULES, probe};

/// Які проби потрібні правилам, що ввімкнені.
pub fn sections(options: &Options) -> Sections {
    let probes = RULES
        .iter()
        .filter(|r| lint::is_active(options, r.code))
        .fold(0, |acc, r| acc | r.probes);
    Sections {
        text: probes & probe::TEXT != 0,
        par: probes & probe::PAR != 0,
        table: probes & probe::TABLE != 0,
        equation: probes & probe::EQUATION != 0,
        runs: probes & probe::RUNS != 0,
    }
}

/// Чи є серед увімкнених правил хоч одне повне.
pub fn wanted(options: &Options) -> bool {
    RULES
        .iter()
        .any(|r| r.full && lint::is_active(options, r.code))
}

/// Компілює документ і збирає факти.
pub fn collect(main: &Path, settings: &Settings, options: &Options) -> anyhow::Result<Facts> {
    run::collect(main, settings, sections(options))
}

pub(crate) fn check(doc: &Doc, facts: &Facts, active: &dyn Fn(&str) -> bool) -> Vec<Found> {
    check::run(doc, facts, active)
}
