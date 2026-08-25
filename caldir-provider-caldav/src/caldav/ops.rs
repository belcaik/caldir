//! Pure CalDAV operations taking credentials and URLs as parameters.
//!
//! These functions are provider-agnostic and can be used by any CalDAV-based
//! provider (iCloud, generic CalDAV, etc.).

use anyhow::{Context, Result};
use caldir_core::{CalendarItem, ComponentKind};
use http::{Method, Request, Uri};
use libdav::caldav::GetCalendarResources;
use libdav::dav::make_relative_url;
use libdav::requests::{DavRequest, ParseResponseError};

use crate::caldav::{CalDavClient_, event_url, url_to_href};

pub mod create_event;
pub mod delete_event;
pub mod discover;
pub mod list_calendars;
pub mod list_events;
pub mod update_event;

#[cfg(test)]
pub(crate) mod test_dav;

pub use create_event::create_event;
pub use delete_event::delete_event;
pub use discover::{DiscoveredEndpoints, discover_endpoints};
pub use list_calendars::{RawCalendar, list_calendars_raw};
pub use list_events::fetch_events;
pub use update_event::update_event;

/// Decode the one supported item on a single CalDAV resource.
///
/// A CalDAV resource holds exactly one supported component — a `VEVENT` (plus
/// any `RECURRENCE-ID` overrides) or a `VTODO` — optionally beside a
/// `VTIMEZONE`, which is exempt from the count (R18, R102). Anything else is
/// an error naming what it found.
///
/// Loud, never a skip: a resource that decodes to nothing is indistinguishable
/// from a resource the server never returned, and that absence is exactly what
/// the delete path acts on (R19, R48, R74). The arity and component-name rules
/// are `CalendarItem`'s, not re-derived here (R54).
pub(super) fn parse_item(ics: &str) -> Result<CalendarItem> {
    Ok(CalendarItem::from_single_ics_str(ics)?)
}

/// Result of looking up a CalDAV resource by UID.
pub(super) struct EventLocation {
    pub href: String,
    pub etag: String,
}

/// The iCalendar component name a [`ComponentKind`] filters on.
fn component_name(kind: ComponentKind) -> &'static str {
    match kind {
        ComponentKind::Event => "VEVENT",
        ComponentKind::Todo => "VTODO",
    }
}

/// Find an item's href and etag on the server, by `(uid, kind)`.
///
/// First tries the conventional `{uid}.ics` URL (works for iCloud, most CalDAV servers).
/// If nothing of the requested kind is there, falls back to a UID-based calendar-query
/// REPORT (for servers like Runbox/Sabre that use server-assigned filenames).
///
/// **The kind is part of the lookup, on both paths (R60, R61).** Radicale enforces UID
/// uniqueness per collection component-agnostically, so "the wrong kind at this href"
/// is a real state — and returning it would point a task's PUT at somebody's meeting.
/// The href itself is `{uid}.ics` for both kinds, unchanged (R62).
pub(super) async fn find_event_location(
    caldav: &CalDavClient_,
    calendar_href: &str,
    calendar_url: &str,
    uid: &str,
    kind: ComponentKind,
) -> Result<EventLocation> {
    // Fast path: try {uid}.ics directly
    let uid_href = url_to_href(&event_url(calendar_url, uid));
    if let Some(location) = location_at_href(caldav, calendar_href, &uid_href, kind).await {
        return Ok(location);
    }

    // Fallback: query by UID (handles servers with non-standard resource filenames)
    caldav
        .request(FindEventByUid::new(calendar_href, uid, kind))
        .await
        .context("Failed to find event on server - it may have been deleted")
}

/// The resource at `href`, but only if it really holds an item of `kind`.
///
/// `None` means "fall through to the REPORT": not found, undecodable, or the
/// wrong component. A decode failure is never "return this href anyway" — that
/// would hand a caller an href whose contents it has not identified.
async fn location_at_href(
    caldav: &CalDavClient_,
    calendar_href: &str,
    href: &str,
    kind: ComponentKind,
) -> Option<EventLocation> {
    let response = caldav
        .request(GetCalendarResources::new(calendar_href).with_hrefs([href]))
        .await
        .ok()?;

    let content = response.resources.into_iter().next()?.content.ok()?;
    let item = parse_item(&content.data).ok()?;

    (item.instance_id().kind() == kind).then(|| EventLocation {
        href: href.to_string(),
        etag: content.etag,
    })
}

// ============================================================================
// Custom CalDAV request to find an event by UID
// ============================================================================

/// Request to find a calendar resource by its UID property.
///
/// Uses a CalDAV calendar-query REPORT with a prop-filter on UID.
/// Returns the href and etag of the matching resource.
struct FindEventByUid<'a> {
    collection_href: &'a str,
    uid: &'a str,
    kind: ComponentKind,
}

impl<'a> FindEventByUid<'a> {
    fn new(collection_href: &'a str, uid: &'a str, kind: ComponentKind) -> Self {
        Self {
            collection_href,
            uid,
            kind,
        }
    }
}

/// The UID-lookup REPORT body.
///
/// Pure and `String`-returning so the body is assertable without a transport;
/// `prepare_request` is the only production caller.
///
/// **No `<C:time-range>`, ever (R60).** The lookup asks for one named item, and
/// a range would hide an undated or long-completed task from its own update.
/// Only the component name is parameterised: for `ComponentKind::Event` the
/// bytes are exactly what caldir sent before VTODO support existed (R1/R2).
fn find_event_by_uid_body(uid: &str, kind: ComponentKind) -> String {
    format!(
        r#"<C:calendar-query xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
    <prop>
        <getetag/>
    </prop>
    <C:filter>
        <C:comp-filter name="VCALENDAR">
            <C:comp-filter name="{component}">
                <C:prop-filter name="UID">
                    <C:text-match collation="i;octet">{uid}</C:text-match>
                </C:prop-filter>
            </C:comp-filter>
        </C:comp-filter>
    </C:filter>
</C:calendar-query>"#,
        component = component_name(kind),
    )
}

impl DavRequest for FindEventByUid<'_> {
    type Response = EventLocation;
    type ParseError = ParseResponseError;
    type Error<E> = libdav::dav::WebDavError<E>;

    fn prepare_request(&self, base_url: Uri) -> std::result::Result<Request<String>, http::Error> {
        let body = find_event_by_uid_body(self.uid, self.kind);

        Request::builder()
            .method(Method::from_bytes(b"REPORT")?)
            .uri(make_relative_url(base_url, self.collection_href)?)
            .header("Depth", "1")
            .header("Content-Type", "application/xml")
            .body(body)
    }

    fn parse_response(
        &self,
        parts: &http::response::Parts,
        body: &[u8],
    ) -> std::result::Result<Self::Response, ParseResponseError> {
        if !parts.status.is_success() {
            return Err(ParseResponseError::BadStatusCode(parts.status));
        }

        let text = std::str::from_utf8(body)?;
        let doc = roxmltree::Document::parse(text)?;
        let root = doc.root_element();

        for response in root
            .descendants()
            .filter(|n| n.tag_name().name() == "response")
        {
            let href = response
                .descendants()
                .find(|n| n.tag_name().name() == "href")
                .and_then(|n| n.text());
            let etag = response
                .descendants()
                .find(|n| n.tag_name().name() == "getetag")
                .and_then(|n| n.text());

            if let (Some(href), Some(etag)) = (href, etag) {
                return Ok(EventLocation {
                    href: href.to_string(),
                    etag: etag.to_string(),
                });
            }
        }

        Err(ParseResponseError::BadStatusCode(
            http::StatusCode::NOT_FOUND,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Radicale shape for any zoned task: a synthesized `VTIMEZONE`
    /// sibling alongside the one `VTODO` (probe Q-C). Inline, so the
    /// multi-component shape this fixture exists for cannot be edited out.
    const RADICALE_ZONED_VTODO_ICS: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Radicale//NONSGML Radicale Server//EN\r\n\
BEGIN:VTIMEZONE\r\n\
TZID:Europe/Stockholm\r\n\
BEGIN:STANDARD\r\n\
DTSTART:19701025T030000\r\n\
TZOFFSETFROM:+0200\r\n\
TZOFFSETTO:+0100\r\n\
TZNAME:CET\r\n\
END:STANDARD\r\n\
BEGIN:DAYLIGHT\r\n\
DTSTART:19700329T020000\r\n\
TZOFFSETFROM:+0100\r\n\
TZOFFSETTO:+0200\r\n\
TZNAME:CEST\r\n\
END:DAYLIGHT\r\n\
END:VTIMEZONE\r\n\
BEGIN:VTODO\r\n\
UID:zoned-task@u-crawler\r\n\
DTSTAMP:20260801T090000Z\r\n\
DUE;TZID=Europe/Stockholm:20260815T235900\r\n\
SUMMARY:Zoned deadline\r\n\
END:VTODO\r\n\
END:VCALENDAR\r\n";

    #[test]
    fn parse_event_extracts_uid_and_summary() {
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:evt-1@caldir\r\nDTSTART:20260615T100000Z\r\nDTEND:20260615T110000Z\r\nSUMMARY:Test\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

        let item = parse_item(ics).expect("should parse");
        let CalendarItem::Event(event) = item else {
            panic!("expected the VEVENT, got {item:?}");
        };
        assert_eq!(event.uid.as_str(), "evt-1@caldir");
        assert_eq!(event.summary.as_deref(), Some("Test"));
    }

    #[test]
    fn parse_event_errors_on_invalid_ics() {
        assert!(parse_item("not ics").is_err());
    }

    /// `R66`/`R19`. The old `find_map(Result::ok)` returned the VEVENT and
    /// dropped the VTODO without a word; a short remote list is exactly what
    /// the delete path acts on (R48, R74). A multi-component resource is now
    /// an error naming the count.
    #[test]
    fn a_multi_component_resource_errors_rather_than_keeping_the_first() {
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:evt-1@caldir\r\nDTSTART:20260615T100000Z\r\nDTEND:20260615T110000Z\r\nSUMMARY:Meeting\r\nEND:VEVENT\r\n\
BEGIN:VTODO\r\nUID:task-1@caldir\r\nSUMMARY:Task\r\nEND:VTODO\r\n\
END:VCALENDAR\r\n";

        let err = parse_item(ics).expect_err("keeping the first component is the R66 defect");

        assert!(
            err.to_string().contains('2'),
            "the error must name the count it found (R75), got: {err}"
        );
    }

    /// `R19`. An unsupported data-bearing component names itself and is never
    /// an empty `Ok`.
    #[test]
    fn a_vjournal_only_resource_errors_and_names_the_component() {
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VJOURNAL\r\nUID:journal-1@caldir\r\nDTSTAMP:20260801T090000Z\r\nEND:VJOURNAL\r\n\
END:VCALENDAR\r\n";

        let err = parse_item(ics).expect_err("an unsupported component must not decode");

        assert!(
            err.to_string().contains("VJOURNAL"),
            "R19: the error must name the component, got: {err}"
        );
    }

    /// `R18`/`R102`. The single most likely regression of a naive arity check:
    /// every zoned task Radicale returns carries a `VTIMEZONE` sibling.
    #[test]
    fn a_vtimezone_sibling_does_not_break_a_zoned_task() {
        let item = parse_item(RADICALE_ZONED_VTODO_ICS)
            .expect("a VTIMEZONE sibling must not be counted towards arity");

        let CalendarItem::Todo(todo) = item else {
            panic!("expected the VTODO, got {item:?}");
        };

        assert_eq!(todo.uid.as_str(), "zoned-task@u-crawler");
        assert_eq!(todo.summary.as_deref(), Some("Zoned deadline"));
    }

    // ========================================================================
    // T-39 — `find_event_location` is component-aware on both paths
    // ========================================================================

    use crate::caldav::create_caldav_client;
    use crate::caldav::ops::test_dav::{calendar_url, mount_report, multistatus, received_bodies};
    use caldir_core::ComponentKind;
    use wiremock::MockServer;

    const TASK_UID: &str = "task-1@u-crawler";

    /// A `VEVENT` sitting at `{uid}.ics` under the UID a *task* is looking for.
    /// This is the resource a kind-blind fast path would hand to a task PUT,
    /// destroying somebody's meeting.
    const VEVENT_AT_TASK_UID: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:task-1@u-crawler\r\nDTSTART:20260615T100000Z\r\nDTEND:20260615T110000Z\r\n\
SUMMARY:Not the task\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    const VTODO_AT_TASK_UID: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTODO\r\nUID:task-1@u-crawler\r\nDTSTAMP:20260801T090000Z\r\n\
SUMMARY:File the tax return\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

    /// The href `{uid}.ics` resolves to (R62: unchanged for both kinds).
    const UID_HREF: &str = "/cal/task-1@u-crawler.ics";
    /// A server-assigned filename — the only reachable reason the REPORT
    /// fallback exists (INDEX §0a ③).
    const ASSIGNED_HREF: &str = "/cal/9f2c-server-assigned.ics";

    async fn locate(server: &MockServer, kind: ComponentKind) -> Result<EventLocation> {
        let calendar_url = calendar_url(server);
        let caldav = create_caldav_client(&calendar_url, "user", "pass")?;
        find_event_location(
            &caldav,
            &url_to_href(&calendar_url),
            &calendar_url,
            TASK_UID,
            kind,
        )
        .await
    }

    /// The UID-lookup REPORT body the double received. Panics if none was sent.
    async fn uid_query_body(server: &MockServer) -> String {
        received_bodies(server)
            .await
            .into_iter()
            .find(|body| body.contains("calendar-query"))
            .expect("the REPORT fallback must have run")
    }

    /// **T-39** (R60, R61, R62). `find_event_location` resolves by
    /// `(uid, kind)`, on the `{uid}.ics` fast path *and* in the REPORT
    /// fallback. Four sub-cases, driven from raw ICS on an HTTP double.
    #[tokio::test]
    async fn find_event_location_is_component_aware_on_both_paths() {
        // (a) A task whose `{uid}.ics` really holds the VTODO: fast path.
        let server = MockServer::start().await;
        mount_report(
            &server,
            "calendar-multiget",
            multistatus(UID_HREF, "\"t-1\"", VTODO_AT_TASK_UID),
        )
        .await;

        let found = locate(&server, ComponentKind::Todo)
            .await
            .expect("the task at {uid}.ics is found on the fast path");
        assert_eq!(found.href, UID_HREF, "R62: the href stays {{uid}}.ics");
        assert_eq!(found.etag, "\"t-1\"");

        // (b) A task whose `{uid}.ics` holds an EVENT: the fast path must not
        // return it, and the fallback must ask for VTODO.
        let server = MockServer::start().await;
        mount_report(
            &server,
            "calendar-multiget",
            multistatus(UID_HREF, "\"e-1\"", VEVENT_AT_TASK_UID),
        )
        .await;
        mount_report(
            &server,
            "calendar-query",
            multistatus(ASSIGNED_HREF, "\"srv-1\"", VTODO_AT_TASK_UID),
        )
        .await;

        let found = locate(&server, ComponentKind::Todo)
            .await
            .expect("the REPORT fallback resolves the task");
        assert_eq!(
            found.href, ASSIGNED_HREF,
            "a task must never be located at an href holding an event — a PUT \
             there overwrites somebody's meeting"
        );
        assert_eq!(found.etag, "\"srv-1\"");
        assert!(
            uid_query_body(&server)
                .await
                .contains(r#"<C:comp-filter name="VTODO">"#),
            "R60: the UID-lookup REPORT must filter on the requested component"
        );

        // (c) R2/C2 no-regression: an event at its own `{uid}.ics` resolves
        // exactly as it does today.
        let server = MockServer::start().await;
        mount_report(
            &server,
            "calendar-multiget",
            multistatus(UID_HREF, "\"e-1\"", VEVENT_AT_TASK_UID),
        )
        .await;

        let found = locate(&server, ComponentKind::Event)
            .await
            .expect("the event at {uid}.ics is found on the fast path");
        assert_eq!(found.href, UID_HREF);
        assert_eq!(found.etag, "\"e-1\"");

        // (d) Fast path 404 (nothing at `{uid}.ics`): the fallback carries the
        // REQUESTED component name, and never a time-range.
        for (kind, expected) in [
            (ComponentKind::Event, "VEVENT"),
            (ComponentKind::Todo, "VTODO"),
        ] {
            let server = MockServer::start().await;
            mount_report(
                &server,
                "calendar-query",
                multistatus(ASSIGNED_HREF, "\"srv-1\"", VTODO_AT_TASK_UID),
            )
            .await;

            let found = locate(&server, kind)
                .await
                .expect("the REPORT fallback answers");
            assert_eq!(found.href, ASSIGNED_HREF);

            let body = uid_query_body(&server).await;
            assert!(
                body.contains(&format!(r#"<C:comp-filter name="{expected}">"#)),
                "R60: {kind:?} must query {expected}; got:\n{body}"
            );
            assert!(
                !body.contains("time-range"),
                "R60: the UID lookup has no <C:time-range> and never gains one; \
                 got:\n{body}"
            );
        }
    }

    /// **AC 3 (R1/R2).** The UID-lookup REPORT for an *event* is byte-identical
    /// to the one caldir sent before VTODO support existed. This literal is
    /// copied verbatim from the pre-change `prepare_request`; a moved space
    /// fails it.
    #[test]
    fn the_uid_lookup_body_for_an_event_is_byte_identical_to_the_pre_vtodo_query() {
        let expected = r#"<C:calendar-query xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
    <prop>
        <getetag/>
    </prop>
    <C:filter>
        <C:comp-filter name="VCALENDAR">
            <C:comp-filter name="VEVENT">
                <C:prop-filter name="UID">
                    <C:text-match collation="i;octet">evt-1@caldir</C:text-match>
                </C:prop-filter>
            </C:comp-filter>
        </C:comp-filter>
    </C:filter>
</C:calendar-query>"#;

        assert_eq!(
            find_event_by_uid_body("evt-1@caldir", ComponentKind::Event),
            expected,
            "the event UID lookup must not move"
        );
    }

    /// **AC 2 (R60).** The VTODO lookup differs from the VEVENT one in the
    /// component name and nothing else — same `prop`, same `prop-filter`, same
    /// collation, still no `<C:time-range>`.
    #[test]
    fn the_uid_lookup_body_for_a_task_differs_only_in_the_component_name() {
        let event = find_event_by_uid_body(TASK_UID, ComponentKind::Event);
        let task = find_event_by_uid_body(TASK_UID, ComponentKind::Todo);

        assert_ne!(event, task, "the two lookups must not be the same query");
        assert_eq!(
            event.replace(r#"name="VEVENT""#, r#"name="VTODO""#),
            task,
            "R60: only the component name is parameterised"
        );
        assert!(!task.contains("time-range"), "got:\n{task}");
    }

    /// A resource caldir cannot decode at `{uid}.ics` means **fall through**,
    /// never "return this href anyway": handing back an href whose contents
    /// were never identified points the next PUT or DELETE at an unknown
    /// resource.
    #[tokio::test]
    async fn an_undecodable_resource_at_the_uid_href_falls_through_to_the_report() {
        const VJOURNAL: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VJOURNAL\r\nUID:task-1@u-crawler\r\nDTSTAMP:20260801T090000Z\r\nEND:VJOURNAL\r\n\
END:VCALENDAR\r\n";

        let server = MockServer::start().await;
        mount_report(
            &server,
            "calendar-multiget",
            multistatus(UID_HREF, "\"j-1\"", VJOURNAL),
        )
        .await;
        mount_report(
            &server,
            "calendar-query",
            multistatus(ASSIGNED_HREF, "\"srv-1\"", VTODO_AT_TASK_UID),
        )
        .await;

        let found = locate(&server, ComponentKind::Todo)
            .await
            .expect("the REPORT fallback resolves the task");

        assert_eq!(
            found.href, ASSIGNED_HREF,
            "an undecodable {{uid}}.ics must not be returned as the location"
        );
    }
}
