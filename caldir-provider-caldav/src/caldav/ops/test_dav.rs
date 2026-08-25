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
