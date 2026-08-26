//! Create a new event on a CalDAV calendar.

use anyhow::{Context, Result};
use caldir_core::{CalendarItem, Event};
use http::Request;
use libdav::caldav::GetCalendarResources;

use crate::caldav::{CalDavClient_, create_caldav_client, event_url, url_to_href};

use super::{find_event_location, parse_item, put_ics};

/// The CalDAV XML namespace the conflict element lives in.
const CALDAV_NS: &str = "urn:ietf:params:xml:ns:caldav";

/// Is this error body the server saying "that UID already lives elsewhere"?
///
/// Matched on the **element name** in the CalDAV namespace, never on the
/// serialized bytes: Radicale writes `<C:no-uid-conflict />` with a space, and
/// the prefix is the server's to choose. A body that is not XML is not a
/// conflict.
fn is_no_uid_conflict(body: &str) -> bool {
    roxmltree::Document::parse(body).is_ok_and(|doc| {
        doc.descendants().any(|node| {
            node.tag_name().name() == "no-uid-conflict"
                && node.tag_name().namespace() == Some(CALDAV_NS)
        })
    })
}

/// Serialize an item to a single-component `VCALENDAR` document, from the
/// kind's own serializer. Never routed through a shared "as event" path —
/// that is exactly how a task would get rewritten as a `VEVENT` (CROSS-6).
pub(super) fn item_to_ics_string(item: &CalendarItem) -> String {
    match item {
        CalendarItem::Event(event) => event.to_ics_string(),
        CalendarItem::Todo(todo) => todo.to_ics_string(),
        _ => unreachable!("caldir_core::CalendarItem grew a variant; give it a serializer here"),
    }
}

/// Fetch the resource at `href` back and decode it, whichever kind it holds.
///
/// `None` on any failure to reach or decode it — the caller falls back to the
/// value it sent rather than failing a create that already succeeded.
pub(super) async fn read_back(
    caldav: &CalDavClient_,
    calendar_url: &str,
    href: &str,
) -> Option<CalendarItem> {
    let calendar_href = url_to_href(calendar_url);
    let response = caldav
        .request(GetCalendarResources::new(&calendar_href).with_hrefs([href]))
        .await
        .ok()?;

    let content = response.resources.into_iter().next()?.content.ok()?;
    parse_item(&content.data).ok()
}

/// The `409 <C:no-uid-conflict/>` fallback: the UID already lives at a
/// server-assigned href, so this create is really an update. Locates the
/// resource **by the item's own kind, never a constant** — a task's lookup
/// landing on somebody's event would overwrite the wrong resource — and PUTs
/// there conditionally on its etag. Runs at most once: this never calls back
/// into `create_item`, so the fallback cannot recurse or loop.
async fn update_after_uid_conflict(
    caldav: &CalDavClient_,
    calendar_url: &str,
    item: CalendarItem,
    ics_content: String,
) -> Result<CalendarItem> {
    let calendar_href = url_to_href(calendar_url);
    let location = find_event_location(
        caldav,
        &calendar_href,
        calendar_url,
        item.uid().as_str(),
        item.instance_id().kind(),
    )
    .await?;

    let uri = caldav.relative_uri(&location.href)?;
    let request = Request::builder()
        .method(http::Method::PUT)
        .uri(&uri)
        .header("Content-Type", "text/calendar")
        .header("If-Match", &location.etag)
        .body(ics_content)?;

    let (parts, _body) = put_ics(caldav, request)
        .await
        .context("Failed to update event")?;

    if !parts.status.is_success() {
        anyhow::bail!("Failed to update event: server returned {}", parts.status);
    }

    Ok(read_back(caldav, calendar_url, &location.href)
        .await
        .unwrap_or(item))
}

/// Create a new item — event or task — on a CalDAV calendar, as its own
/// component: a task is uploaded as `VTODO` calendar data and is never
/// rewritten as a `VEVENT` (CROSS-6).
///
/// Uses a raw PUT request to avoid duplicate Content-Type headers (a libdav quirk).
/// Fetches the created item back to get server-assigned values.
///
/// A `409 <C:no-uid-conflict/>` is not a failure but the server saying the UID
/// already lives at another href; caldir then does what that means — locate the
/// resource and update it. Once, never in a loop.
pub async fn create_item(
    username: &str,
    password: &str,
    calendar_url: &str,
    item: CalendarItem,
) -> Result<CalendarItem> {
    let caldav = create_caldav_client(calendar_url, username, password)?;

    let ics_content = item_to_ics_string(&item);

    let full_url = event_url(calendar_url, item.uid().as_str());
    let href = url_to_href(&full_url);

    // Use request_raw instead of libdav's PutResource to avoid a duplicate
    // Content-Type header. WebDavClient::request() sets a default
    // "Content-Type: application/xml", then PutResource appends
    // "Content-Type: text/calendar" — some servers reject the duplicate.
    let uri = caldav.relative_uri(&href)?;
    let request = Request::builder()
        .method(http::Method::PUT)
        .uri(&uri)
        .header("Content-Type", "text/calendar")
        .header("If-None-Match", "*")
        .body(ics_content.clone())?;

    let (parts, response_body) = put_ics(&caldav, request)
        .await
        .context("Failed to create event")?;

    if !parts.status.is_success() {
        let response_body = String::from_utf8_lossy(&response_body);

        // `409 <C:no-uid-conflict/>`: the UID already lives at some other href
        // (a server-assigned filename), so this create is really an update.
        if parts.status == http::StatusCode::CONFLICT && is_no_uid_conflict(&response_body) {
            return update_after_uid_conflict(&caldav, calendar_url, item, ics_content).await;
        }

        anyhow::bail!(
            "Failed to create event: server returned {}: {}",
            parts.status,
            response_body
        );
    }

    Ok(read_back(&caldav, calendar_url, &href)
        .await
        .unwrap_or(item))
}

/// Create a new event on a CalDAV calendar.
///
/// A thin wrapper over [`create_item`]: the event-only entry point every
/// pre-VTODO caller already uses, unchanged in signature and behaviour.
pub async fn create_event(
    username: &str,
    password: &str,
    calendar_url: &str,
    event: Event,
) -> Result<Event> {
    match create_item(username, password, calendar_url, CalendarItem::Event(event)).await? {
        CalendarItem::Event(event) => Ok(event),
        item => anyhow::bail!(
            "create_event: server returned a {:?}, not an event",
            item.instance_id().kind()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caldav::ops::test_dav::{calendar_url, mount_report, multistatus};
    use caldir_core::{CalendarItem, EventTime, EventUid, Todo};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A local task, uid pinned so `event_url`'s derivation is assertable.
    fn local_todo() -> Todo {
        let mut todo = Todo::new("Sent by caldir");
        todo.uid = EventUid::new("task-1@caldir");
        todo
    }

    /// **AC 1, 2 (§8.1, R62, CROSS-6).** The two MUSTs are both statements
    /// about the bytes on the wire: a `VTODO` PUT at the UID-derived href,
    /// never rewritten as a `VEVENT`.
    #[tokio::test]
    async fn a_task_is_put_as_vtodo_at_the_uid_derived_href() {
        let server = MockServer::start().await;

        Mock::given(method("PUT"))
            .and(path("/cal/task-1@caldir.ics"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;

        create_item(
            "user",
            "pass",
            &calendar_url(&server),
            CalendarItem::Todo(local_todo()),
        )
        .await
        .expect("creating a task succeeds");

        let puts = puts(&server).await;
        assert_eq!(puts.len(), 1, "exactly one PUT for a task create");
        assert_eq!(puts[0].url.path(), "/cal/task-1@caldir.ics");

        let body = String::from_utf8_lossy(&puts[0].body);
        assert!(body.contains("BEGIN:VTODO"), "got: {body}");
        assert!(
            !body.contains("BEGIN:VEVENT"),
            "a task must never be rewritten as an event, got: {body}"
        );
        assert_eq!(
            puts[0]
                .headers
                .get("Content-Type")
                .map(|v| v.to_str().unwrap()),
            Some("text/calendar")
        );
        assert_eq!(
            puts[0]
                .headers
                .get("If-None-Match")
                .map(|v| v.to_str().unwrap()),
            Some("*")
        );
    }

    /// What the *server* holds after the PUT: a different SUMMARY and an
    /// `X-` property caldir never sent. Raw ICS, so the remote is an
    /// independent store rather than the encoder under test replaying itself.
    const SERVER_HELD_ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:evt-1@caldir\r\nDTSTART:20260615T100000Z\r\nDTEND:20260615T110000Z\r\n\
SUMMARY:Renamed by the server\r\nX-RADICALE-NAME:9f2c.ics\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    /// **M8.** `create_event` reads the resource back and returns the
    /// *server's* version, not the one it sent. Until this test the read-back
    /// branch was dead weight: `&& false` on it passed the whole suite, and
    /// server-assigned values were silently lost on every create.
    #[tokio::test]
    async fn create_returns_the_resource_the_server_holds_not_the_one_sent() {
        let server = MockServer::start().await;

        Mock::given(method("PUT"))
            .and(path("/cal/evt-1@caldir.ics"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;
        mount_report(
            &server,
            "calendar-multiget",
            multistatus("/cal/evt-1@caldir.ics", "\"e-1\"", SERVER_HELD_ICS),
        )
        .await;

        let mut local = Event::new(
            "Sent by caldir",
            EventTime::DateTimeUtc(
                "2026-06-15T10:00:00Z"
                    .parse::<chrono::DateTime<chrono::Utc>>()
                    .expect("a valid timestamp"),
            ),
        );
        local.uid = EventUid::new("evt-1@caldir");

        let created = create_event("user", "pass", &calendar_url(&server), local)
            .await
            .expect("the create succeeds");

        assert_eq!(
            created.summary.as_deref(),
            Some("Renamed by the server"),
            "the read-back must win: dropping it loses every server-assigned value"
        );
        assert!(
            created
                .x_properties
                .iter()
                .any(|prop| prop.name == "X-RADICALE-NAME"),
            "server-only properties must survive the read-back, got {:?}",
            created.x_properties
        );
    }

    /// What the server holds after the PUT: a different `SUMMARY` and an
    /// `X-RADICALE-NAME` caldir never sent. Raw ICS, so the remote is an
    /// independent store rather than the encoder under test replaying itself.
    const SERVER_HELD_VTODO_ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTODO\r\nUID:task-1@caldir\r\nDTSTAMP:20260801T090000Z\r\n\
SUMMARY:Renamed by the server\r\nX-RADICALE-NAME:9f2c.ics\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

    /// **AC 5 (§8.4).** The shipped read-back arm is
    /// `if let Ok(CalendarItem::Event(fetched_event)) = parse_item(..)` — a
    /// `Todo` read-back falls straight through it and the function silently
    /// returns the locally-sent item. That failure is invisible until
    /// `P2-02` writes the wrong sync base and the next sync duplicates.
    #[tokio::test]
    async fn create_returns_the_task_the_server_holds_not_the_one_sent() {
        let server = MockServer::start().await;

        Mock::given(method("PUT"))
            .and(path("/cal/task-1@caldir.ics"))
            .respond_with(ResponseTemplate::new(201))
            .mount(&server)
            .await;
        mount_report(
            &server,
            "calendar-multiget",
            multistatus("/cal/task-1@caldir.ics", "\"t-1\"", SERVER_HELD_VTODO_ICS),
        )
        .await;

        let created = create_item(
            "user",
            "pass",
            &calendar_url(&server),
            CalendarItem::Todo(local_todo()),
        )
        .await
        .expect("the create succeeds");

        let CalendarItem::Todo(created) = created else {
            panic!("expected the server's Todo back, got {created:?}");
        };

        assert_eq!(
            created.summary.as_deref(),
            Some("Renamed by the server"),
            "the read-back must win for a task exactly as it does for an event"
        );
        assert!(
            created
                .x_properties
                .iter()
                .any(|prop| prop.name == "X-RADICALE-NAME"),
            "server-only properties must survive the read-back, got {:?}",
            created.x_properties
        );
    }

    // ========================================================================
    // T-40 — `409 <C:no-uid-conflict/>` falls back to an update
    // ========================================================================

    /// The verbatim Radicale 409 body (probe Q-E.3). Note the space before
    /// `/>`: substring-matching the serialized element is exactly the trap
    /// this fixture exists to spring.
    const NO_UID_CONFLICT_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<error xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><C:no-uid-conflict /></error>"#;

    /// The href `{uid}.ics` resolves to for `evt-1@caldir`.
    const UID_HREF: &str = "/cal/evt-1@caldir.ics";
    /// Where the server actually keeps that UID — a server-assigned filename,
    /// the only reachable trigger for the 409 (INDEX §0a ③).
    const ASSIGNED_HREF: &str = "/cal/other-href.ics";

    fn local_event() -> Event {
        let mut local = Event::new(
            "Sent by caldir",
            EventTime::DateTimeUtc(
                "2026-06-15T10:00:00Z"
                    .parse::<chrono::DateTime<chrono::Utc>>()
                    .expect("a valid timestamp"),
            ),
        );
        local.uid = EventUid::new("evt-1@caldir");
        local
    }

    /// Every PUT the double received, in arrival order.
    async fn puts(server: &MockServer) -> Vec<wiremock::Request> {
        server
            .received_requests()
            .await
            .expect("the double records requests")
            .into_iter()
            .filter(|request| request.method == http::Method::PUT)
            .collect()
    }

    /// **T-40** (R63, R79). A `409` carrying `<C:no-uid-conflict/>` means "that
    /// UID already lives at another href" — the create is really an update.
    /// caldir must locate the existing resource and issue a *conditional* PUT
    /// there. Retrying the same href would loop; bailing would strand the user.
    #[tokio::test]
    async fn a_409_no_uid_conflict_falls_back_to_update() {
        let server = MockServer::start().await;

        Mock::given(method("PUT"))
            .and(path(UID_HREF))
            .respond_with(
                ResponseTemplate::new(409)
                    .set_body_raw(NO_UID_CONFLICT_BODY, "application/xml; charset=utf-8"),
            )
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path(ASSIGNED_HREF))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        // No `calendar-multiget` double: nothing sits at `{uid}.ics`, so the
        // UID-lookup REPORT is what resolves the resource.
        mount_report(
            &server,
            "calendar-query",
            multistatus(ASSIGNED_HREF, "\"srv-9\"", SERVER_HELD_ICS),
        )
        .await;

        create_event("user", "pass", &calendar_url(&server), local_event())
            .await
            .expect("a 409 no-uid-conflict is an update, not a failure");

        let puts = puts(&server).await;
        assert_eq!(
            puts.len(),
            2,
            "exactly one fallback attempt: a retry against the same href loops \
             forever, got {:?}",
            puts.iter().map(|r| r.url.path()).collect::<Vec<_>>()
        );
        assert_eq!(
            puts[0].url.path(),
            UID_HREF,
            "the first attempt is still the plain {{uid}}.ics create (R62)"
        );
        assert_eq!(
            puts[0]
                .headers
                .get("If-None-Match")
                .map(|v| v.to_str().unwrap()),
            Some("*"),
            "the create stays conditional: without it a create blindly \
             overwrites whatever is already there"
        );
        assert_eq!(
            puts[1].url.path(),
            ASSIGNED_HREF,
            "the fallback must target the located resource, not {UID_HREF}"
        );
        assert_eq!(
            puts[1].headers.get("If-Match").map(|v| v.to_str().unwrap()),
            Some("\"srv-9\""),
            "the fallback PUT must be conditional on the located etag"
        );
        assert!(
            !puts[1].headers.contains_key("If-None-Match"),
            "If-None-Match: * on the fallback would re-trigger the conflict"
        );
    }

    /// **AC 4 (INDEX §4 P3-02 risk, SESSION-LOG §7).** The same fallback for a
    /// task, asserted as an **outcome** — one resource, correct body, no
    /// duplicate — never as a status code. The reachable trigger is a
    /// resource at another href already holding the UID (a server-assigned
    /// filename); a local rename yields the *same* href, because `event_url`
    /// derives it from the UID alone. Three ticket briefs in the prior effort
    /// asserted the rename story and were wrong.
    #[tokio::test]
    async fn a_409_no_uid_conflict_on_a_task_updates_the_one_resource_that_holds_the_uid() {
        const TASK_UID_HREF: &str = "/cal/task-1@caldir.ics";
        const TASK_ASSIGNED_HREF: &str = "/cal/other-task-href.ics";

        let server = MockServer::start().await;

        Mock::given(method("PUT"))
            .and(path(TASK_UID_HREF))
            .respond_with(
                ResponseTemplate::new(409)
                    .set_body_raw(NO_UID_CONFLICT_BODY, "application/xml; charset=utf-8"),
            )
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path(TASK_ASSIGNED_HREF))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        // No `calendar-multiget` double: nothing sits at `{uid}.ics`, so the
        // UID-lookup REPORT is what resolves the resource — and it must
        // filter on VTODO, never VEVENT.
        mount_report(
            &server,
            r#"<C:comp-filter name="VTODO">"#,
            multistatus(TASK_ASSIGNED_HREF, "\"tsk-9\"", SERVER_HELD_VTODO_ICS),
        )
        .await;

        create_item(
            "user",
            "pass",
            &calendar_url(&server),
            CalendarItem::Todo(local_todo()),
        )
        .await
        .expect("a 409 no-uid-conflict on a task is an update, not a failure");

        // The outcome: one resource holds the task, at the located href, and
        // no duplicate sits at the original {uid}.ics attempt. The status
        // code the server answered with is deliberately not asserted.
        let puts = puts(&server).await;
        assert_eq!(
            puts.len(),
            2,
            "exactly one fallback attempt, got {:?}",
            puts.iter().map(|r| r.url.path()).collect::<Vec<_>>()
        );
        assert_eq!(puts[0].url.path(), TASK_UID_HREF);
        assert_eq!(
            puts[1].url.path(),
            TASK_ASSIGNED_HREF,
            "the fallback must target the located resource, not a duplicate at {TASK_UID_HREF}"
        );
        assert_eq!(
            puts[1].headers.get("If-Match").map(|v| v.to_str().unwrap()),
            Some("\"tsk-9\""),
            "the fallback PUT must be conditional on the located etag"
        );
        assert!(
            !puts[1].headers.contains_key("If-None-Match"),
            "If-None-Match: * on the fallback would re-trigger the conflict"
        );
        let body = String::from_utf8_lossy(&puts[1].body);
        assert!(
            body.contains("BEGIN:VTODO"),
            "the correct body: the fallback PUT must still carry the task, got: {body}"
        );
    }

    /// **AC 6, INDEX §4 P3-02 risk.** `find_event_location` takes the kind
    /// from the item being written, never a constant: an item-typed rewrite
    /// that hardcodes `ComponentKind::Event` compiles, passes every other
    /// test, and overwrites somebody's meeting with a task. Here a `VEVENT`
    /// sharing the UID sits right at `{uid}.ics` — the fallback must never
    /// land there.
    #[tokio::test]
    async fn a_task_create_never_locates_a_same_uid_event() {
        const TASK_UID_HREF: &str = "/cal/task-1@caldir.ics";
        const TASK_ASSIGNED_HREF: &str = "/cal/other-task-href.ics";

        /// A VEVENT sharing the task's UID, sitting right at `{uid}.ics`.
        const SAME_UID_VEVENT_ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:task-1@caldir\r\nDTSTART:20260615T100000Z\r\nDTEND:20260615T110000Z\r\n\
SUMMARY:Somebody's meeting\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

        let server = MockServer::start().await;

        Mock::given(method("PUT"))
            .and(path(TASK_UID_HREF))
            .respond_with(
                ResponseTemplate::new(409)
                    .set_body_raw(NO_UID_CONFLICT_BODY, "application/xml; charset=utf-8"),
            )
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path(TASK_ASSIGNED_HREF))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        // The fast path: a VEVENT, not the task, sits at {uid}.ics.
        mount_report(
            &server,
            "calendar-multiget",
            multistatus(TASK_UID_HREF, "\"evt-9\"", SAME_UID_VEVENT_ICS),
        )
        .await;
        // The wrong-kind resource at the fast path must fall through to the
        // VTODO-filtered lookup, which is the task's real location.
        mount_report(
            &server,
            r#"<C:comp-filter name="VTODO">"#,
            multistatus(TASK_ASSIGNED_HREF, "\"tsk-9\"", SERVER_HELD_VTODO_ICS),
        )
        .await;

        create_item(
            "user",
            "pass",
            &calendar_url(&server),
            CalendarItem::Todo(local_todo()),
        )
        .await
        .expect("a same-uid VEVENT must not prevent locating the task");

        let puts = puts(&server).await;
        assert_eq!(
            puts.len(),
            2,
            "got {:?}",
            puts.iter().map(|r| r.url.path()).collect::<Vec<_>>()
        );
        assert_eq!(puts[0].url.path(), TASK_UID_HREF);
        assert_eq!(
            puts[1].url.path(),
            TASK_ASSIGNED_HREF,
            "the fallback must never land on the VEVENT's href"
        );
    }

    /// **R79's "not opaque" clause.** A `409` that is *not* a UID conflict is
    /// still a failure, and the message must carry the status **and** the body
    /// so the user can see what the server objected to.
    #[tokio::test]
    async fn a_409_without_no_uid_conflict_is_a_loud_error_carrying_the_body() {
        const OTHER_CONFLICT: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<error xmlns="DAV:"><lock-token-submitted/></error>"#;

        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .respond_with(
                ResponseTemplate::new(409)
                    .set_body_raw(OTHER_CONFLICT, "application/xml; charset=utf-8"),
            )
            .mount(&server)
            .await;

        let err = create_event("user", "pass", &calendar_url(&server), local_event())
            .await
            .expect_err("an unrelated 409 must not be swallowed by the fallback");

        let message = format!("{err:#}");
        assert!(
            message.contains("409"),
            "the status must survive, got: {message}"
        );
        assert!(
            message.contains("lock-token-submitted"),
            "R79: the body must be quoted, not discarded, got: {message}"
        );
        assert_eq!(
            puts(&server).await.len(),
            1,
            "no fallback PUT for a conflict that is not a UID conflict"
        );
    }

    /// The adjacent status. `If-None-Match: *` against an existing href yields
    /// `412`, not `409`; treating it as a UID conflict would turn "somebody
    /// else wrote first" into a silent overwrite. The body here deliberately
    /// *does* carry `<C:no-uid-conflict/>` so that dropping the status check
    /// alone fails this test.
    #[tokio::test]
    async fn a_412_precondition_failed_is_not_treated_as_a_uid_conflict() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .respond_with(
                ResponseTemplate::new(412)
                    .set_body_raw(NO_UID_CONFLICT_BODY, "application/xml; charset=utf-8"),
            )
            .mount(&server)
            .await;

        let err = create_event("user", "pass", &calendar_url(&server), local_event())
            .await
            .expect_err("a 412 is a lost race, never an update");

        let message = format!("{err:#}");
        assert!(
            message.contains("412"),
            "the status must survive, got: {message}"
        );
        assert_eq!(
            puts(&server).await.len(),
            1,
            "a 412 must never be followed by an unconditional overwrite"
        );
    }

    /// **AC 2.** The conflict is recognised by *element name in the CalDAV
    /// namespace*. Radicale writes `<C:no-uid-conflict />` with a space, the
    /// prefix is the server's to choose, and a same-named element in another
    /// namespace is a different error entirely.
    #[test]
    fn the_conflict_is_matched_by_element_name_in_the_caldav_namespace() {
        assert!(
            is_no_uid_conflict(NO_UID_CONFLICT_BODY),
            "the verbatim Radicale body must match despite the space before `/>`"
        );
        assert!(
            is_no_uid_conflict(
                r#"<D:error xmlns:D="DAV:" xmlns:cal="urn:ietf:params:xml:ns:caldav"><cal:no-uid-conflict/></D:error>"#
            ),
            "the namespace prefix is the server's to choose"
        );
        assert!(
            !is_no_uid_conflict(r#"<error xmlns="DAV:"><no-uid-conflict/></error>"#),
            "the same local name in another namespace is a different error"
        );
        assert!(
            !is_no_uid_conflict(r#"<error xmlns="DAV:"><lock-token-submitted/></error>"#),
            "an unrelated DAV error is not a UID conflict"
        );
        assert!(
            !is_no_uid_conflict("no-uid-conflict"),
            "a non-XML body is never a conflict, however it reads"
        );
    }
}
