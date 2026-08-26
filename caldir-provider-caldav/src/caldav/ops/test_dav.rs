//! HTTP-double helpers for the CalDAV request/response gates.
//!
//! Dev-only (`#[cfg(test)]`): `wiremock` is a dev-dependency and never ships.
//! It exists because the two-REPORT merge in `fetch_events` and the fast path
//! in `find_event_location` are *transport* behaviour — no pure function sees
//! them, and a mutation to either passed the whole suite before this module.

use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The collection every double serves.
pub const COLLECTION_PATH: &str = "/cal/";

/// The calendar URL a caller passes to the ops under test.
pub fn calendar_url(server: &MockServer) -> String {
    format!("{}{COLLECTION_PATH}", server.uri())
}

/// A `207 Multi-Status` carrying exactly one resource's href, etag and
/// calendar-data. Written as raw XML around raw ICS, so nothing on the remote
/// side is produced by the encoder under test.
pub fn multistatus(href: &str, etag: &str, ics: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<multistatus xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <response>
    <href>{href}</href>
    <propstat>
      <prop>
        <getetag>{etag}</getetag>
        <C:calendar-data>{ics}</C:calendar-data>
      </prop>
      <status>HTTP/1.1 200 OK</status>
    </propstat>
  </response>
</multistatus>"#
    )
}

/// Answer every `REPORT` on the collection whose body contains `marker` with
/// `body`.
///
/// `marker` is what tells the doubles apart: `"VEVENT"`/`"VTODO"` for the two
/// listing queries, `"calendar-multiget"`/`"calendar-query"` for the fast path
/// versus the UID lookup.
pub async fn mount_report(server: &MockServer, marker: &str, body: String) {
    Mock::given(method("REPORT"))
        .and(path(COLLECTION_PATH))
        .and(body_string_contains(marker.to_string()))
        .respond_with(
            ResponseTemplate::new(207).set_body_raw(body, "application/xml; charset=utf-8"),
        )
        .mount(server)
        .await;
}

/// Answer every `REPORT` on the collection whose body contains `marker` with
/// a bare `status` and no body — simulating a server rejecting that query
/// (e.g. a bare-`VTODO` `calendar-query` on a task-hostile collection).
pub async fn mount_report_status(server: &MockServer, marker: &str, status: u16) {
    Mock::given(method("REPORT"))
        .and(path(COLLECTION_PATH))
        .and(body_string_contains(marker.to_string()))
        .respond_with(ResponseTemplate::new(status))
        .mount(server)
        .await;
}

/// A `207 Multi-Status` for `PROPFIND` on `DAV:supported-calendar-component-set`,
/// advertising exactly `names` (e.g. `&["VEVENT"]`, `&["VEVENT", "VTODO"]`).
pub fn component_set_multistatus(names: &[&str]) -> String {
    let comps: String = names
        .iter()
        .map(|name| format!(r#"<C:comp name="{name}"/>"#))
        .collect();

    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<multistatus xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <response>
    <href>{COLLECTION_PATH}</href>
    <propstat>
      <prop>
        <C:supported-calendar-component-set>{comps}</C:supported-calendar-component-set>
      </prop>
      <status>HTTP/1.1 200 OK</status>
    </propstat>
  </response>
</multistatus>"#
    )
}

/// Answer the collection's `PROPFIND` with `body` (207 Multi-Status).
pub async fn mount_propfind(server: &MockServer, body: String) {
    Mock::given(method("PROPFIND"))
        .and(path(COLLECTION_PATH))
        .respond_with(
            ResponseTemplate::new(207).set_body_raw(body, "application/xml; charset=utf-8"),
        )
        .mount(server)
        .await;
}

/// Answer the collection's `PROPFIND` with a bare failure `status`.
pub async fn mount_propfind_status(server: &MockServer, status: u16) {
    Mock::given(method("PROPFIND"))
        .and(path(COLLECTION_PATH))
        .respond_with(ResponseTemplate::new(status))
        .mount(server)
        .await;
}

/// The property **names** inside the one `VTODO` component of a document, as a
/// sorted, de-duplicated set.
///
/// Only the `VTODO`: a sibling `VTIMEZONE` component is expected and must not
/// be counted, and nested `VALARM` properties belong to the alarm. Shared
/// between the live Radicale tier and the ops-level fidelity gates so the
/// extractor cannot drift into two copies.
pub(crate) fn vtodo_property_names(ics: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut depth = 0usize;

    for line in ics.lines() {
        if let Some(component) = line.strip_prefix("BEGIN:") {
            depth += 1;
            if component == "VTODO" {
                // Reset: only the VTODO's own properties count.
                names.clear();
            }
            continue;
        }
        if line.starts_with("END:") {
            if line.starts_with("END:VTODO") {
                break;
            }
            depth = depth.saturating_sub(1);
            continue;
        }
        // Only lines directly inside the VTODO (VCALENDAR + VTODO = depth 2),
        // and never a folded continuation line.
        if depth == 2
            && !line.starts_with(' ')
            && !line.starts_with('\t')
            && let Some(name) = line.split([';', ':']).next()
            && !name.is_empty()
        {
            names.push(name.to_string());
        }
    }

    names.sort();
    names.dedup();
    names
}

/// Every request body the double received, in arrival order.
pub async fn received_bodies(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .expect("the double records requests")
        .iter()
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .collect()
}
