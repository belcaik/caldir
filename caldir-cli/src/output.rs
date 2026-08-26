use serde::Serialize;

pub mod agenda;
mod agenda_json;
mod agenda_text;
pub mod calendars;
pub mod config;
pub mod diff;
pub mod event;
pub mod time;

/// Human text rendering for a command's result.
pub trait TextRender {
    fn to_text(&self) -> String;
}

/// A command result renderable as either human text or JSON.
pub trait Output: TextRender {
    fn to_json(&self) -> serde_json::Value;
}

impl<T: Serialize + TextRender> Output for T {
    fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("view serialization is infallible")
    }
}

#[derive(Clone, Copy)]
pub enum OutputFormat {
    Text,
    Json,
}

/// Render an agenda, plus — under `--json` only — the notices the schema has
/// no room for.
///
/// `--json` is the item stream and nothing else, so a listing that came back
/// short has no key to say so with, and adding one would change the public
/// schema. stderr is not part of that contract: a script reads the array from
/// stdout exactly as before, and can now tell a partial agenda from a complete
/// one by looking at the other stream. Text output already carries the same
/// notice above the agenda, so it is not repeated there.
pub fn emit_agenda(agenda: &agenda::Agenda, format: OutputFormat) {
    if let OutputFormat::Json = format {
        for notice in agenda_text::incomplete_lines(&agenda.incomplete) {
            eprintln!("{notice}");
        }
    }

    emit(agenda, format);
}

/// Render a command result to stdout in the chosen format.
pub fn emit(output: &dyn Output, format: OutputFormat) {
    match format {
        OutputFormat::Text => println!("{}", output.to_text()),
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string(&output.to_json()).expect("view serialization is infallible")
        ),
    }
}
