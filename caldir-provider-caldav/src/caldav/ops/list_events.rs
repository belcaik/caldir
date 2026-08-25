//! Fetch events from a CalDAV calendar within a time range.

use anyhow::{Context, Result};
use caldir_core::Event;
use http::{Method, Request, Uri};
use libdav::dav::make_relative_url;
use libdav::requests::{DavRequest, ParseResponseError};

use crate::caldav::{create_caldav_client, format_caldav_datetime, url_to_href};

use super::parse_event;

/// Fetch events from a CalDAV calendar within a time range.
pub async fn fetch_events(
    username: &str,
    password: &str,
    calendar_url: &str,
    from: &str,
    to: &str,
) -> Result<Vec<Event>> {
    let caldav = create_caldav_client(calendar_url, username, password)?;

    let calendar_href = url_to_href(calendar_url);

    let from_caldav = format_caldav_datetime(from);
    let to_caldav = format_caldav_datetime(to);

    let mut resources = Vec::new();
    for query in calendar_queries(&calendar_href, &from_caldav, &to_caldav) {
        let response = caldav
            .request(query)
            .await
            .context("Failed to fetch calendar resources")?;
        resources.extend(response.resources);
    }

    let mut events = Vec::new();
    for resource in resources {
        // TODO(L5-01): `parse_event` narrows to VEVENT and drops everything
        // else silently, so the VTODO REPORT's resources land here and vanish.
        // L5-01 (R66) owns making them visible; this is named, not hidden.
        if let Some(event) = parse_event(&resource.data) {
            events.push(event);
        }
    }

    Ok(events)
}

/// The REPORTs one `fetch_events` run issues, in order.
///
/// **Two REPORTs, never one query with sibling comp-filters (R59).** RFC 4791
/// §9.7.1 says a comp-filter matches when its children *all* match, so
/// `VEVENT` and `VTODO` side by side under one `VCALENDAR` filter read as AND
/// and would match nothing; whether any given server ORs siblings is
/// unverified, so caldir never relies on it.
fn calendar_queries<'a>(
    collection_href: &'a str,
    start: &'a str,
    end: &'a str,
) -> [CalendarQuery<'a>; 2] {
    [
        CalendarQuery {
            collection_href,
            filter: ComponentFilter::EventsInRange { start, end },
        },
        CalendarQuery {
            collection_href,
            filter: ComponentFilter::AllTasks,
        },
    ]
}

// ============================================================================
// Custom CalDAV request for time-range filtered calendar queries
// ============================================================================

/// A `calendar-query` REPORT against one collection.
struct CalendarQuery<'a> {
    collection_href: &'a str,
    filter: ComponentFilter<'a>,
}

/// The component filter a [`CalendarQuery`] carries.
#[derive(Clone, Copy)]
enum ComponentFilter<'a> {
    /// `VEVENT` narrowed by `<C:time-range>` — server-side filtering, far
    /// cheaper than fetching every event. `start`/`end` are in CalDAV format,
    /// `YYYYMMDDTHHMMSSZ`. Byte-identical to what caldir has always sent
    /// (R1/R2): this arm must not move.
    EventsInRange { start: &'a str, end: &'a str },
    /// `VTODO`, **bare** — no `<C:time-range>` child (R51, R58, INVIOLABLE).
    ///
    /// RFC 4791 §9.9's VTODO time-range table evaluates FALSE for a task whose
    /// only date property is a `COMPLETED` older than `start`, so a *compliant*
    /// server MUST NOT return it — confirmed live against Radicale 3.7.6
    /// (`docs/vtodo/context/radicale-probe.md` Q-B, fixture `t2`). Core would
    /// then read that absence as a delete and remove the user's file
    /// (spec §10, §12). If the payload ever bites, the fix is WebDAV-Sync
    /// (RFC 6578) sync tokens — **not** a filter.
    AllTasks,
}

/// The REPORT body for one filter.
///
/// Pure and `String`-returning so the bodies are assertable without a
/// transport; `prepare_request` is the only production caller.
fn calendar_query_body(filter: ComponentFilter<'_>) -> String {
    let comp_filter = match filter {
        ComponentFilter::EventsInRange { start, end } => format!(
            r#"<C:comp-filter name="VEVENT">
                <C:time-range start="{start}" end="{end}"/>
            </C:comp-filter>"#
        ),
        ComponentFilter::AllTasks => r#"<C:comp-filter name="VTODO"/>"#.to_string(),
    };

    format!(
        r#"<C:calendar-query xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
    <prop>
        <getetag/>
        <C:calendar-data/>
    </prop>
    <C:filter>
        <C:comp-filter name="VCALENDAR">
            {comp_filter}
        </C:comp-filter>
    </C:filter>
</C:calendar-query>"#
    )
}

/// A fetched calendar resource with its ICS data.
#[derive(Debug)]
struct CalendarResource {
    _href: String,
    _etag: Option<String>,
    data: String,
}

/// Response from a [`CalendarQuery`] request.
#[derive(Debug)]
struct CalendarQueryResponse {
    resources: Vec<CalendarResource>,
}

impl DavRequest for CalendarQuery<'_> {
    type Response = CalendarQueryResponse;
    type ParseError = ParseResponseError;
    type Error<E> = libdav::dav::WebDavError<E>;

    fn prepare_request(&self, base_url: Uri) -> std::result::Result<Request<String>, http::Error> {
        let body = calendar_query_body(self.filter);

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

        let resources = parse_calendar_resources(body)?;
        Ok(CalendarQueryResponse { resources })
    }
}

/// Parse calendar resources from a CalDAV multistatus response.
fn parse_calendar_resources(
    body: &[u8],
) -> std::result::Result<Vec<CalendarResource>, ParseResponseError> {
    let text = std::str::from_utf8(body)?;
    let doc = roxmltree::Document::parse(text)?;
    let root = doc.root_element();

    let mut resources = Vec::new();

    // Find all <response> elements
    for response in root
        .descendants()
        .filter(|n| n.tag_name().name() == "response")
    {
        // Get href
        let href = response
            .descendants()
            .find(|n| n.tag_name().name() == "href")
            .and_then(|n| n.text())
            .map(|s| s.to_string());

        let Some(href) = href else { continue };

        // Get etag
        let etag = response
            .descendants()
            .find(|n| n.tag_name().name() == "getetag")
            .and_then(|n| n.text())
            .map(|s| s.to_string());

        // Get calendar-data
        let data = response
            .descendants()
            .find(|n| n.tag_name().name() == "calendar-data")
            .and_then(|n| n.text())
            .map(|s| s.to_string());

        // Only include resources that have calendar data
        if let Some(data) = data {
            resources.push(CalendarResource {
                _href: href,
                _etag: etag,
                data,
            });
        }
    }

    Ok(resources)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HREF: &str = "/testuser/calendar/";
    const START: &str = "20250825T000000Z";
    const END: &str = "20270825T000000Z";

    fn vevent_body() -> String {
        calendar_query_body(ComponentFilter::EventsInRange {
            start: START,
            end: END,
        })
    }

    fn vtodo_body() -> String {
        calendar_query_body(ComponentFilter::AllTasks)
    }

    /// R1/R2, AC 7. The VEVENT REPORT body is byte-identical to the one caldir
    /// sent before VTODO support existed. This literal is copied verbatim from
    /// the pre-change `prepare_request`; if the refactor moved so much as a
    /// space, this fails.
    #[test]
    fn the_vevent_report_body_is_byte_identical_to_the_pre_vtodo_query() {
        let expected = format!(
            r#"<C:calendar-query xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
    <prop>
        <getetag/>
        <C:calendar-data/>
    </prop>
    <C:filter>
        <C:comp-filter name="VCALENDAR">
            <C:comp-filter name="VEVENT">
                <C:time-range start="{START}" end="{END}"/>
            </C:comp-filter>
        </C:comp-filter>
    </C:filter>
</C:calendar-query>"#
        );

        assert_eq!(vevent_body(), expected, "the VEVENT query must not move");
    }

    /// T-37 (R51, R58). The VTODO `calendar-query` carries **no**
    /// `<C:time-range>`.
    ///
    /// RFC 4791 §9.9's VTODO time-range table makes a task whose only date
    /// property is a `COMPLETED` older than `start` evaluate FALSE, so a
    /// *compliant* server MUST NOT return it — verified live against Radicale
    /// 3.7.6 (`docs/vtodo/context/radicale-probe.md` Q-B, fixture `t2`). Core
    /// then sees a previously-synced task missing from the remote and emits an
    /// incoming delete that removes the user's file (spec §10, §12). This
    /// assertion is the lock on the requirement the spec calls "the one most
    /// likely to be 'optimized' back into a bug": do not re-add a time-range
    /// here.
    #[test]
    fn the_vtodo_report_body_contains_no_time_range() {
        let body = vtodo_body();

        assert!(
            body.contains(r#"comp-filter name="VTODO""#),
            "the VTODO query must filter on VTODO; got:\n{body}"
        );
        assert!(
            !body.contains("time-range"),
            "RFC 4791 §9.9 + spec §10/§12: the VTODO calendar-query MUST carry \
             no <C:time-range> — a compliant server omits an undated task whose \
             only date property is an old COMPLETED, and core reads that \
             absence as a delete. Got:\n{body}"
        );
    }

    /// T-38 (R59). Two REPORTs, not one query with sibling comp-filters.
    ///
    /// RFC 4791 §9.7.1 says a comp-filter matches when its children *all*
    /// match, so `VEVENT` and `VTODO` side by side under one `VCALENDAR`
    /// filter read as AND and would match nothing. Whether Radicale ORs
    /// siblings is unverified, so caldir never relies on it.
    #[test]
    fn the_vtodo_report_is_a_separate_request_from_the_vevent_report() {
        let queries = calendar_queries(HREF, START, END);
        let bodies: Vec<String> = queries
            .iter()
            .map(|query| calendar_query_body(query.filter))
            .collect();

        assert_eq!(bodies.len(), 2, "one run issues exactly two REPORTs");
        assert_ne!(bodies[0], bodies[1], "the two REPORTs differ");

        let (events, tasks) = (&bodies[0], &bodies[1]);

        // The VEVENT REPORT still carries its time-range and knows nothing
        // about tasks.
        assert!(events.contains(r#"<C:comp-filter name="VEVENT">"#));
        assert!(events.contains(&format!(r#"<C:time-range start="{START}" end="{END}"/>"#)));
        assert!(
            !events.contains("VTODO"),
            "VTODO must not be a sibling inside the VEVENT query (R59)"
        );

        // …and the VTODO REPORT knows nothing about events.
        assert!(tasks.contains(r#"comp-filter name="VTODO""#));
        assert!(
            !tasks.contains("VEVENT"),
            "VEVENT must not be a sibling inside the VTODO query (R59)"
        );

        // Both address the same collection.
        for query in &queries {
            assert_eq!(query.collection_href, HREF);
        }
    }
}
