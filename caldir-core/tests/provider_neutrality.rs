//! `P6-04`/`CROSS-2`/`CROSS-6`: two structural guarantees that no behavioural
//! test can catch, because both are about code that must *never be written*
//! rather than about a wrong answer from code that exists. A source scan is
//! the only proof that survives the commit that introduces the violation.
//!
//! Lives outside the crate (an integration test, its own compilation unit)
//! because the pattern it copies — `rpc.rs`'s `scan_rust_sources` /
//! `contains_word` — is `#[cfg(test)]`-private and cannot be imported; the
//! twenty lines are reimplemented here rather than loosened into a public
//! matcher just to be shared.

use std::path::Path;

/// `needle` as a whole identifier: not a prefix or suffix of a longer one.
/// Mirrors `rpc.rs`'s `contains_word` so both scans reject the same class of
/// false positive (e.g. `into_event` must not fire on `into_events`).
fn contains_word(haystack: &str, needle: &str) -> bool {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';

    haystack.match_indices(needle).any(|(at, _)| {
        let before = haystack[..at].chars().next_back();
        let after = haystack[at + needle.len()..].chars().next();

        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

fn scan_rust_sources(path: &Path, needle: &str, offenders: &mut Vec<String>) {
    if path.is_dir() {
        for entry in std::fs::read_dir(path).unwrap() {
            scan_rust_sources(&entry.unwrap().path(), needle, offenders);
        }
        return;
    }

    if !path.extension().is_some_and(|ext| ext == "rs") {
        return;
    }

    let source = std::fs::read_to_string(path).unwrap();

    if contains_word(&source, needle) {
        offenders.push(path.display().to_string());
    }
}

/// Everything from this file's `#[cfg(test)]\nmod tests {` marker onward —
/// every file this scan cares about keeps its one test module at the tail
/// (the convention `rpc.rs`, `remote.rs`, `item.rs` and this file's own
/// sibling all follow), so slicing there, rather than allow-listing a whole
/// path, keeps the scan sensitive to a violation landing anywhere in the
/// file's *production* code while still tolerating a named fixture inside
/// its tests.
fn strip_trailing_test_module(source: &str) -> &str {
    match source.find("#[cfg(test)]\nmod tests {") {
        Some(at) => &source[..at],
        None => source,
    }
}

fn scan_rust_sources_outside_test_modules(path: &Path, needle: &str, offenders: &mut Vec<String>) {
    if path.is_dir() {
        for entry in std::fs::read_dir(path).unwrap() {
            scan_rust_sources_outside_test_modules(&entry.unwrap().path(), needle, offenders);
        }
        return;
    }

    if !path.extension().is_some_and(|ext| ext == "rs") {
        return;
    }

    let source = std::fs::read_to_string(path).unwrap();
    let production = strip_trailing_test_module(&source);

    if contains_word(production, needle) {
        offenders.push(path.display().to_string());
    }
}

fn src_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// `CROSS-2`/D-3: no shipped provider slug may appear as a string literal
/// anywhere under `caldir-core/src`, because that is the only shape
/// provider-name branching could take — core carries a `ProviderSlug` as an
/// opaque string and must never match on its contents.
///
/// Right proof: `CROSS-2` is a *negative* about code that does not exist yet.
/// No behavioural test can catch its introduction — a `match slug.as_str() {
/// "google" => ..., _ => ... }` compiles, passes every other test in the
/// suite, and only a scan of the source itself notices the new branch.
///
/// Scans each file's *production* code only — everything before its trailing
/// `#[cfg(test)] mod tests { ... }` (`strip_trailing_test_module`) — so two
/// known `#[cfg(test)]` fixtures never have to be allow-listed by path and
/// stay covered like everything else the moment they leave their test
/// module: `remote/config.rs` round-trips `RemoteConfig::from_toml(r#"provider
/// = "caldav""#)` (a slug carried through TOML parsing, not a match on it),
/// and `remote.rs`'s own `a_task_write_produces_the_same_request_for_every_
/// provider_slug` (`P6-04`/`CROSS-1`) constructs two `MockProvider`s literally
/// named `"caldav"` and `"google"` to *prove* core's outbound bytes do not
/// depend on which one is listening.
#[test]
fn caldir_core_never_branches_on_a_provider_name() {
    let root = src_root();

    let mut offenders = Vec::new();
    for slug in ["caldav", "google", "icloud", "outlook", "webcal"] {
        let needle = format!("\"{slug}\"");
        let mut hits = Vec::new();
        scan_rust_sources_outside_test_modules(&root, &needle, &mut hits);

        for hit in hits {
            offenders.push(format!("{hit} ({needle})"));
        }
    }

    assert!(
        offenders.is_empty(),
        "a provider-name string literal has leaked into caldir-core's production \
         code; provider-name branching must live inside providers, never in core \
         (D-3): {offenders:?}"
    );
}

/// `CROSS-6`: `CalendarItem::into_event` is the *only* narrowing from a task
/// to an event anywhere in `caldir-core/src`. Scans for the other shapes such
/// a narrowing could take — a `From<Todo> for Event` impl, or a sibling
/// `as_event`/`to_event` method — and allow-lists only `item.rs`'s own
/// `into_event` (the sanctioned, deliberately fallible backstop) and its
/// `#[cfg(test)]` sibling `as_event` (a test-only accessor, never a
/// narrowing a production path can reach).
///
/// Right proof: a silent VTODO-to-VEVENT conversion looks like a successful
/// push — the failure is silent and destructive, not a wrong answer a
/// behavioural assertion would notice — so the guard has to be structural.
#[test]
fn no_caldir_core_path_converts_a_vtodo_into_a_vevent() {
    let root = src_root();

    let mut literal_impl = Vec::new();
    scan_rust_sources(&root, "impl From<Todo> for Event", &mut literal_impl);
    assert!(
        literal_impl.is_empty(),
        "a Todo-to-Event From impl must never exist: {literal_impl:?}"
    );

    let mut from_todo = Vec::new();
    scan_rust_sources(&root, "from_todo", &mut from_todo);
    assert!(
        from_todo.is_empty(),
        "no `from_todo` constructor may exist: {from_todo:?}"
    );

    // Scans for *definitions* (`fn into_event`, not every call site or doc
    // mention of the existing one) — calling the sanctioned narrowing is
    // fine and expected; a second definition of it is the violation.
    // `item.rs` carries the two sanctioned hits: `into_event` (the fallible
    // backstop) and `as_event` (its `#[cfg(test)]`-only accessor). Anything
    // else defining one of these three method shapes is a new narrowing this
    // effort never intended.
    let sanctioned_file = root.join("item.rs");
    for method in ["into_event", "as_event", "to_event"] {
        let needle = format!("fn {method}");
        let mut hits = Vec::new();
        scan_rust_sources(&root, &needle, &mut hits);

        let offenders: Vec<_> = hits
            .into_iter()
            .filter(|hit| Path::new(hit) != sanctioned_file)
            .collect();

        assert!(
            offenders.is_empty(),
            "only `caldir-core/src/item.rs` may define `{needle}`, found in {offenders:?}"
        );
    }
}
