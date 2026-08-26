//! Fetch events from a CalDAV calendar within a time range.

use anyhow::{Context, Result};
use caldir_core::CalendarItem;
use http::{Method, Request, Uri};
use libdav::caldav::{CalendarComponent, GetSupportedComponents};
use libdav::dav::make_relative_url;
use libdav::requests::{DavRequest, ParseResponseError};

use crate::caldav::{CalDavClient_, create_caldav_client, format_caldav_datetime, url_to_href};

use super::parse_item;

/// Fetch a CalDAV calendar's items: events in the window, and every task.
///
/// Item-typed, because a `VTODO` has nowhere else to go — narrowing back to
/// `Event` here would re-create the silent drop this listing exists to avoid.
pub async fn fetch_events(
    username: &str,
    password: &str,
    calendar_url: &str,
    from: &str,
    to: &str,
) -> Result<Vec<CalendarItem>> {
    let caldav = create_caldav_client(calendar_url, username, password)?;

    let calendar_href = url_to_href(calendar_url);

    let from_caldav = format_caldav_datetime(from);
    let to_caldav = format_caldav_datetime(to);

    let mut resources = Vec::new();
    for query in calendar_queries(&calendar_href, &from_caldav, &to_caldav) {
        let is_task_query = matches!(query.filter, ComponentFilter::AllTasks);

        match caldav.request(query).await {
            Ok(response) => resources.extend(response.resources),
            Err(err)
                if is_task_query
                    && collection_openly_lacks_vtodo(&caldav, &calendar_href).await =>
            {
                eprintln!(
                    "warning: collection {calendar_href} rejected the VTODO calendar-query and \
                     its supported-calendar-component-set omits VTODO; treating it as an \
                     events-only collection for this listing ({err:#})"
                );
            }
            Err(err) => {
                return Err(err).with_context(|| {
                    format!("Failed to fetch calendar resources from collection {calendar_href}")
                });
            }
        }
    }

    decode_resources(resources)
}

/// Classify a failed `VTODO` REPORT as fatal-vs-degrade, **after** the server
/// has already rejected it. Never a gate on whether to send the
/// REPORT: the property is advisory and can be wrong (R92), so a negative
/// hint alone must never suppress the request. This only decides whether a
/// rejection the server already produced may be downgraded to "no tasks
/// here" — and only when the collection's own advertisement says so
/// explicitly. Absent, unreadable or malformed evidence leaves the original
/// failure fatal, because guessing here can delete a collection's worth of
/// tasks.
async fn collection_openly_lacks_vtodo(caldav: &CalDavClient_, collection_href: &str) -> bool {
    caldav
        .request(GetSupportedComponents::new(collection_href))
        .await
        .map(|response| !response.components.contains(&CalendarComponent::VTodo))
        .unwrap_or(false)
}

/// Decode every fetched resource, or fail the whole listing.
///
/// **A partially-decoded listing is forbidden.** Core reads a
/// remote listing as the complete remote state, so an item missing from it is
/// a delete. "Log and continue" here would turn one unreadable resource into a
/// deleted local file; the run aborts instead, before any apply.
fn decode_resources(resources: Vec<CalendarResource>) -> Result<Vec<CalendarItem>> {
    resources
        .into_iter()
        .map(|resource| {
            parse_item(&resource.data)
                .with_context(|| format!("Failed to decode calendar resource {}", resource.href))
        })
        .collect()
}

/// The REPORTs one `fetch_events` run issues, in order.
///
/// **Two REPORTs, never one query with sibling comp-filters.** RFC 4791
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
    /// `YYYYMMDDTHHMMSSZ`. Byte-identical to what caldir has always sent:
    /// this arm must not move.
    EventsInRange { start: &'a str, end: &'a str },
    /// `VTODO`, **bare** — no `<C:time-range>` child.
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

/// The REPORT bodies one `fetch_events` run actually puts on the wire, in
/// order, taken from `prepare_request` rather than re-derived.
///
/// Test-only seam. The live tier asserts on the **request**,
/// not on a server's answer: Radicale implements RFC 4791 §9.9 faithfully, so
/// a reintroduced `<C:time-range>` on the VTODO query silently omits an
/// undated task and core turns that absence into a delete of the user's file.
/// Reaching through `prepare_request` means a filter rewired anywhere between
/// `calendar_queries` and the wire is caught here.
#[cfg(test)]
pub(crate) fn report_request_bodies(collection_href: &str, start: &str, end: &str) -> Vec<String> {
    let base: Uri = "http://127.0.0.1/".parse().expect("a valid base URI");

    calendar_queries(collection_href, start, end)
        .iter()
        .map(|query| {
            query
                .prepare_request(base.clone())
                .expect("the REPORT request builds")
                .into_body()
        })
        .collect()
}

/// A fetched calendar resource with its ICS data.
#[derive(Debug)]
struct CalendarResource {
    href: String,
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
                href,
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
    use crate::caldav::ops::test_dav::{
        calendar_url, component_set_multistatus, mount_propfind, mount_propfind_status,
        mount_report, mount_report_status, multistatus,
    };
    use wiremock::MockServer;

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

    const VEVENT_RESOURCE_ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:evt-1@caldir\r\nDTSTART:20260615T100000Z\r\nDTEND:20260615T110000Z\r\n\
SUMMARY:Meeting\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    const VTODO_RESOURCE_ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTODO\r\nUID:task-1@caldir\r\nDTSTAMP:20260801T090000Z\r\nSTATUS:COMPLETED\r\n\
SUMMARY:File the tax return\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

    /// A `VJOURNAL` — supported by no caldir version, and the shape a
    /// forward-dated server can put on the wire at any time.
    const VJOURNAL_RESOURCE_ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VJOURNAL\r\nUID:journal-1@caldir\r\nDTSTAMP:20260801T090000Z\r\nEND:VJOURNAL\r\n\
END:VCALENDAR\r\n";

    fn resource(href: &str, data: &str) -> CalendarResource {
        CalendarResource {
            href: href.to_string(),
            _etag: None,
            data: data.to_string(),
        }
    }

    /// R53/R66. The VTODO REPORT's resources reach the caller as tasks. Before
    /// `L5-01` they were fetched and then dropped on the floor by
    /// `find_map(Result::ok)`, which reads to core as "the server does not have
    /// this task" — i.e. a delete.
    #[test]
    fn a_task_resource_reaches_the_caller_as_a_task() {
        let items = decode_resources(vec![
            resource("/cal/evt-1.ics", VEVENT_RESOURCE_ICS),
            resource("/cal/task-1.ics", VTODO_RESOURCE_ICS),
        ])
        .expect("both resources decode");

        let kinds: Vec<&str> = items
            .iter()
            .map(|item| match item {
                CalendarItem::Event(_) => "event",
                CalendarItem::Todo(_) => "todo",
                _ => "other",
            })
            .collect();

        assert_eq!(kinds, ["event", "todo"], "got {items:?}");
        assert_eq!(items[1].uid().as_str(), "task-1@caldir");
    }

    /// AC 3 (R74, R48, R19). One undecodable resource fails the **whole**
    /// listing. A partial listing is forbidden: core treats a remote listing as
    /// the complete remote state, so a resource quietly missing from it is a
    /// delete of the user's local file.
    #[test]
    fn one_undecodable_resource_fails_the_whole_listing() {
        let err = decode_resources(vec![
            resource("/cal/evt-1.ics", VEVENT_RESOURCE_ICS),
            resource("/cal/journal-1.ics", VJOURNAL_RESOURCE_ICS),
            resource("/cal/task-1.ics", VTODO_RESOURCE_ICS),
        ])
        .expect_err("a partially-decoded listing must never be returned");

        let rendered = format!("{err:#}");
        assert!(
            rendered.contains("/cal/journal-1.ics"),
            "the failure must name the offending resource, got: {rendered}"
        );
        assert!(
            rendered.contains("VJOURNAL"),
            "R19: the failure must name the component, got: {rendered}"
        );
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

    /// The **M7 lock**. `fetch_events` issues two REPORTs and merges their
    /// resources; before this test nothing held the merge, and a mutation that
    /// consumed only the first REPORT's resources passed the entire suite.
    /// That is silent task loss: core reads a remote listing as the complete
    /// remote state, so a task missing from it is an incoming delete of the
    /// user's file (R19, R48, R74).
    ///
    /// Driven from raw ICS on a real HTTP double, never from domain values
    /// re-encoded by the code under test.
    #[tokio::test]
    async fn both_reports_resources_reach_the_caller() {
        let server = MockServer::start().await;

        mount_report(
            &server,
            "VEVENT",
            multistatus("/cal/evt-1.ics", "\"e-1\"", VEVENT_RESOURCE_ICS),
        )
        .await;
        mount_report(
            &server,
            "VTODO",
            multistatus("/cal/task-1.ics", "\"t-1\"", VTODO_RESOURCE_ICS),
        )
        .await;

        let items = fetch_events(
            "user",
            "pass",
            &calendar_url(&server),
            "2025-08-25T00:00:00Z",
            "2027-08-25T00:00:00Z",
        )
        .await
        .expect("both REPORTs answer");

        let uids: Vec<&str> = items.iter().map(|item| item.uid().as_str()).collect();

        assert!(
            uids.contains(&"evt-1@caldir"),
            "the VEVENT REPORT's resources must reach the caller, got {uids:?}"
        );
        assert!(
            uids.contains(&"task-1@caldir"),
            "M7: the VTODO REPORT's resources must reach the caller — dropping \
             them reads to core as 'the server does not have this task', i.e. a \
             delete. Got {uids:?}"
        );
    }

    const SAME_UID_EVENT_ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:shared-1@caldir\r\nDTSTART:20260615T100000Z\r\nDTEND:20260615T110000Z\r\n\
SUMMARY:Meeting\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    const SAME_UID_TODO_ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTODO\r\nUID:shared-1@caldir\r\nDTSTAMP:20260801T090000Z\r\nSTATUS:NEEDS-ACTION\r\n\
SUMMARY:File the tax return\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

    /// P3-05, C-9/C-15. A same-UID `VEVENT`/`VTODO` pair, served at two
    /// different hrefs, both survive one listing as two distinct items.
    ///
    /// Right proof because `both_reports_resources_reach_the_caller` uses two
    /// *different* UIDs, so nothing there would catch a merge keyed on UID
    /// alone: such a merge drops one of the two, and core reads that as a
    /// delete of the user's file. `instance_id()`, not `uid()`, is the
    /// identity that must differ.
    #[tokio::test]
    async fn a_same_uid_event_and_task_both_survive_one_listing() {
        let server = MockServer::start().await;

        mount_report(
            &server,
            "VEVENT",
            multistatus("/cal/evt-shared.ics", "\"e-shared\"", SAME_UID_EVENT_ICS),
        )
        .await;
        mount_report(
            &server,
            "VTODO",
            multistatus("/cal/task-shared.ics", "\"t-shared\"", SAME_UID_TODO_ICS),
        )
        .await;

        let items = fetch_events(
            "user",
            "pass",
            &calendar_url(&server),
            "2025-08-25T00:00:00Z",
            "2027-08-25T00:00:00Z",
        )
        .await
        .expect("both REPORTs answer");

        assert_eq!(
            items.len(),
            2,
            "a same-UID event/task pair must yield two items, got {items:?}"
        );

        let uids: Vec<&str> = items.iter().map(|item| item.uid().as_str()).collect();
        assert_eq!(
            uids,
            ["shared-1@caldir", "shared-1@caldir"],
            "both resources share the UID, got {uids:?}"
        );

        assert_ne!(
            items[0].instance_id(),
            items[1].instance_id(),
            "component kind is part of caldir identity: a merge keyed on UID \
             alone collapses this pair into one, which core reads as a delete"
        );
    }

    /// R2/C13. A server that rejects the bare-`VTODO` `calendar-query` and
    /// openly says (via `supported-calendar-component-set`) that it has no
    /// `VTODO` support must not lose the caller's **event** sync. Before this
    /// fix, the second REPORT's `?` discarded a perfectly good first REPORT.
    #[tokio::test]
    async fn a_rejected_vtodo_report_does_not_break_an_event_only_listing() {
        let server = MockServer::start().await;

        mount_report(
            &server,
            "VEVENT",
            multistatus("/cal/evt-1.ics", "\"e-1\"", VEVENT_RESOURCE_ICS),
        )
        .await;
        mount_report_status(&server, "VTODO", 400).await;
        mount_propfind(&server, component_set_multistatus(&["VEVENT"])).await;

        let items = fetch_events(
            "user",
            "pass",
            &calendar_url(&server),
            "2025-08-25T00:00:00Z",
            "2027-08-25T00:00:00Z",
        )
        .await
        .expect("a task-hostile collection must not break event sync");

        let uids: Vec<&str> = items.iter().map(|item| item.uid().as_str()).collect();
        assert_eq!(uids, ["evt-1@caldir"]);
    }

    /// C-14's guard against a blanket swallow. A collection whose advertised
    /// component set **includes** `VTODO` and then refuses the REPORT anyway
    /// must still fail loudly — reporting "no tasks" here would read to core
    /// as a delete of every task the collection holds.
    #[tokio::test]
    async fn a_rejected_vtodo_report_on_a_task_capable_collection_is_still_an_error() {
        let server = MockServer::start().await;

        mount_report(
            &server,
            "VEVENT",
            multistatus("/cal/evt-1.ics", "\"e-1\"", VEVENT_RESOURCE_ICS),
        )
        .await;
        mount_report_status(&server, "VTODO", 400).await;
        mount_propfind(&server, component_set_multistatus(&["VEVENT", "VTODO"])).await;

        let err = fetch_events(
            "user",
            "pass",
            &calendar_url(&server),
            "2025-08-25T00:00:00Z",
            "2027-08-25T00:00:00Z",
        )
        .await
        .expect_err("a task-capable collection refusing the REPORT must not be read as 'no tasks'");

        let rendered = format!("{err:#}");
        assert!(
            rendered.contains(crate::caldav::ops::test_dav::COLLECTION_PATH),
            "the failure must name the collection, got: {rendered}"
        );
    }

    /// R92, the executable form: the property is a post-mortem, never a
    /// gate. A healthy listing issues exactly the two REPORTs it always has —
    /// no `PROPFIND` at all.
    #[tokio::test]
    async fn a_healthy_listing_never_asks_for_the_component_set() {
        let server = MockServer::start().await;

        mount_report(
            &server,
            "VEVENT",
            multistatus("/cal/evt-1.ics", "\"e-1\"", VEVENT_RESOURCE_ICS),
        )
        .await;
        mount_report(
            &server,
            "VTODO",
            multistatus("/cal/task-1.ics", "\"t-1\"", VTODO_RESOURCE_ICS),
        )
        .await;

        fetch_events(
            "user",
            "pass",
            &calendar_url(&server),
            "2025-08-25T00:00:00Z",
            "2027-08-25T00:00:00Z",
        )
        .await
        .expect("both REPORTs answer");

        let bodies = crate::caldav::ops::test_dav::received_bodies(&server).await;
        assert_eq!(bodies.len(), 2, "exactly two requests, got: {bodies:?}");
        for body in &bodies {
            assert!(
                !body.contains("supported-calendar-component-set"),
                "the happy path must fire zero extra requests, got: {body}"
            );
        }
    }

    /// C-14's other guard: when the classifying `PROPFIND` itself cannot be
    /// read (fails, 404s, is malformed), the original failure stays fatal.
    /// Guessing here costs data — an unreadable component set could be a
    /// transient `500` on a collection full of tasks.
    #[tokio::test]
    async fn an_unreadable_component_set_leaves_the_failure_fatal() {
        let server = MockServer::start().await;

        mount_report(
            &server,
            "VEVENT",
            multistatus("/cal/evt-1.ics", "\"e-1\"", VEVENT_RESOURCE_ICS),
        )
        .await;
        mount_report_status(&server, "VTODO", 400).await;
        mount_propfind_status(&server, 500).await;

        fetch_events(
            "user",
            "pass",
            &calendar_url(&server),
            "2025-08-25T00:00:00Z",
            "2027-08-25T00:00:00Z",
        )
        .await
        .expect_err("an unreadable component set must not turn a rejection into a degrade");
    }
}
