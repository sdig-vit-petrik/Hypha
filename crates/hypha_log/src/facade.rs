//! The `log` crate facade: passes each record to the engine.

use std::borrow::Cow;
use std::sync::OnceLock;

use crate::config::valid_module_name;
use crate::{Level, enabled, module_or_default, report};

/// The logger that the `log` crate calls. It keeps no state.
struct Facade;

static FACADE: Facade = Facade;

/// Makes [`install`] run its body once per process.
static INSTALLED: OnceLock<()> = OnceLock::new();

fn level_of(level: log::Level) -> Level {
    match level {
        log::Level::Error => Level::Error,
        log::Level::Warn => Level::Warn,
        log::Level::Info => Level::Info,
        log::Level::Debug => Level::Debug,
        log::Level::Trace => Level::Trace,
    }
}

/// The engine module name for a `log` target: the name itself when the engine accepts
/// it, otherwise its leading identifier path with `::` as `/`, at most 64 bytes
/// (`iceoryx2::service` stays `iceoryx2/service`).
fn module_name(target: &str) -> Cow<'_, str> {
    if valid_module_name(target) {
        return Cow::Borrowed(target);
    }
    let mut chars = target.chars().peekable();
    while chars.next_if(|c| !c.is_ascii_alphanumeric()).is_some() {}
    let mut name = String::new();
    while let Some(c) = chars.next() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '/' {
            name.push(c);
        } else if c == ':' && chars.next_if_eq(&':').is_some() {
            name.push('/');
        } else {
            break;
        }
        if name.len() >= 64 {
            name.truncate(64);
            break;
        }
    }
    Cow::Owned(if name.is_empty() { "default".to_owned() } else { name })
}

/// Whether a record of `module` passes the facade. Hypha and iceoryx2 records always
/// pass; any other crate's at `warn` or above, or when the `[log]` table names its module.
fn admitted(module: &str, level: Level) -> bool {
    module.starts_with("hypha")
        || module.starts_with("iceoryx2")
        || level >= Level::Warn
        || crate::engine::engine().sets_module(module)
}

impl log::Log for Facade {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        let module_name = module_name(metadata.target());
        let level = level_of(metadata.level());
        if !admitted(&module_name, level) {
            return false;
        }
        let Ok(module) = module_or_default(&module_name) else {
            return false;
        };
        enabled(module, level)
    }

    fn log(&self, record: &log::Record<'_>) {
        let module_name = module_name(record.target());
        let level = level_of(record.level());
        if admitted(&module_name, level) {
            report(&module_name, level, *record.args());
        }
    }

    fn flush(&self) {}
}

/// Route the `log` crate to the engine. Later calls do nothing. If another logger was
/// installed first, that is reported once as a warning.
pub fn install() {
    INSTALLED.get_or_init(|| {
        if log::set_logger(&FACADE).is_ok() {
            log::set_max_level(log::LevelFilter::Trace);
        } else if !std::ptr::addr_eq(log::logger(), std::ptr::from_ref::<dyn log::Log>(&FACADE)) {
            report(
                "hypha",
                Level::Warn,
                format_args!("hypha: another logger owns the log crate, so hypha lines use it"),
            );
        }
    });
}
