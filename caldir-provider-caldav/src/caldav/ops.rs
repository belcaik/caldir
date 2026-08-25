//! Pure CalDAV operations taking credentials and URLs as parameters.
//!
//! These functions are provider-agnostic and can be used by any CalDAV-based
//! provider (iCloud, generic CalDAV, etc.).

use anyhow::{Context, Result};
use caldir_core::CalendarItem;
use http::{Method, Request, Uri};
use libdav::dav::{GetEtag, make_relative_url};
use libdav::requests::{DavRequest, ParseResponseError};

use crate::caldav::{CalDavClient_, event_url, url_to_href};

pub mod create_event;
pub mod delete_event;
pub mod discover;
pub mod list_calendars;
pub mod list_events;
pub mod update_event;

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

/// Find an event's href and etag on the server.
///
/// First tries the conventional `{uid}.ics` URL (works for iCloud, most CalDAV servers).
/// If that returns 404, falls back to a UID-based calendar-query REPORT (for servers
/// like Runbox/Sabre that use server-assigned filenames).
pub(super) async fn find_event_location(
    caldav: &CalDavClient_,
    calendar_href: &str,
    calendar_url: &str,
    uid: &str,
) -> Result<EventLocation> {
    // Fast path: try {uid}.ics directly
    let uid_href = url_to_href(&event_url(calendar_url, uid));
    if let Ok(etag_response) = caldav.request(GetEtag::new(&uid_href)).await {
        return Ok(EventLocation {
            href: uid_href,
            etag: etag_response.etag,
        });
    }

    // Fallback: query by UID (handles servers with non-standard resource filenames)
    caldav
        .request(FindEventByUid::new(calendar_href, uid))
        .await
        .context("Failed to find event on server - it may have been deleted")
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
}

impl<'a> FindEventByUid<'a> {
    fn new(collection_href: &'a str, uid: &'a str) -> Self {
        Self {
            collection_href,
            uid,
        }
    }
}

impl DavRequest for FindEventByUid<'_> {
    type Response = EventLocation;
    type ParseError = ParseResponseError;
    type Error<E> = libdav::dav::WebDavError<E>;

    fn prepare_request(&self, base_url: Uri) -> std::result::Result<Request<String>, http::Error> {
        let body = format!(
            r#"<C:calendar-query xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
    <prop>
        <getetag/>
    </prop>
    <C:filter>
        <C:comp-filter name="VCALENDAR">
            <C:comp-filter name="VEVENT">
                <C:prop-filter name="UID">
                    <C:text-match collation="i;octet">{}</C:text-match>
                </C:prop-filter>
            </C:comp-filter>
        </C:comp-filter>
    </C:filter>
</C:calendar-query>"#,
            self.uid
        );

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
}
