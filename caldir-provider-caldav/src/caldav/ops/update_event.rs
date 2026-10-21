//! Update an existing event on a CalDAV calendar.

use anyhow::{Context, Result};
use caldir_core::{CalendarItem, Event};
use http::Request;

use crate::caldav::{create_caldav_client, url_to_href};

use super::create_event::{item_to_ics_string, read_back};
use super::{find_event_location, put_ics};

/// A conditional `PUT`'s `If-Match` no longer matched the server's ETag —
/// RFC 7232 §12.1's `412 Precondition Failed`. A distinct, named error rather
/// than a status code folded into a message string, so a caller (the RPC
/// boundary in `commands::update_event`) can attach a typed error class by
/// downcasting instead of parsing text.
#[derive(Debug)]
pub struct StaleEtag {
    href: String,
}

impl StaleEtag {
    pub fn new(href: impl Into<String>) -> Self {
        Self { href: href.into() }
    }
}

impl std::fmt::Display for StaleEtag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the resource at {} changed since it was last read (stale ETag)",
            self.href
        )
    }
}

impl std::error::Error for StaleEtag {}

/// Update an existing item — event or task — on a CalDAV calendar, at the
/// resource `find_event_location` resolves for `(uid, kind)`. Never invents a
/// second resource: the located href and etag from that lookup are the only
/// PUT target, so a changed local item always lands on the remote resource
/// that already holds it (§8.2).
///
/// Uses a raw PUT request to avoid duplicate Content-Type headers (a libdav
/// quirk, see `create_event`). Fetches the updated item back to get
/// server-assigned values.
pub async fn update_item(
    username: &str,
    password: &str,
    calendar_url: &str,
    item: CalendarItem,
) -> Result<CalendarItem> {
    let caldav = create_caldav_client(calendar_url, username, password)?;

    let ics_content = item_to_ics_string(&item);
    let calendar_href = url_to_href(calendar_url);

    // Try {uid}.ics first (most servers), fall back to UID-based REPORT query.
    // The kind comes from the item being written, never a constant: locating
    // an event's href for a task's PUT would overwrite the wrong resource.
    let location = find_event_location(
        &caldav,
        &calendar_href,
        calendar_url,
        item.uid().as_str(),
        item.instance_id().kind(),
    )
    .await?;

    // Use request_raw to avoid duplicate Content-Type header (see create_event).
    let uri = caldav.relative_uri(&location.href)?;
    let request = Request::builder()
        .method(http::Method::PUT)
        .uri(&uri)
        .header("Content-Type", "text/calendar")
        .header("If-Match", &location.etag)
        .body(ics_content)?;

    let (parts, _body) = put_ics(&caldav, request)
        .await
        .context("Failed to update event")?;

    if !parts.status.is_success() {
        if parts.status == http::StatusCode::PRECONDITION_FAILED {
            return Err(StaleEtag::new(location.href.clone()).into());
        }
        anyhow::bail!("Failed to update event: server returned {}", parts.status);
    }

    // Fetch the updated item back to get server-assigned values, whichever
    // kind it is (§8.4, §7) — falling back to what was sent if the read-back
    // itself fails, since the write already succeeded.
    Ok(read_back(&caldav, calendar_url, &location.href)
        .await
        .unwrap_or(item))
}

/// Update an existing event on a CalDAV calendar.
///
/// A thin wrapper over [`update_item`]: the event-only entry point every
/// pre-VTODO caller already uses, unchanged in signature and behaviour.
pub async fn update_event(
    username: &str,
    password: &str,
    calendar_url: &str,
    event: Event,
) -> Result<Event> {
    match update_item(username, password, calendar_url, CalendarItem::Event(event)).await? {
        CalendarItem::Event(event) => Ok(event),
        item => anyhow::bail!(
            "update_event: server returned a {:?}, not an event",
            item.instance_id().kind()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caldav::ops::test_dav::{calendar_url, mount_report, multistatus};
    use caldir_core::{EventTime, EventUid, Todo, TodoStatus};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Every `PUT` the double received, in arrival order.
    async fn puts(server: &MockServer) -> Vec<wiremock::Request> {
        server
            .received_requests()
            .await
            .expect("the double records requests")
            .into_iter()
            .filter(|request| request.method == http::Method::PUT)
            .collect()
    }

    /// A server-held task at `{uid}.ics`, etag `"t-1"`.
    const SERVER_HELD_VTODO_ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTODO\r\nUID:task-1@caldir\r\nDTSTAMP:20260801T090000Z\r\n\
SUMMARY:Buy milk\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

    /// A local task, uid pinned to match `SERVER_HELD_VTODO_ICS`.
    fn local_todo() -> Todo {
        let mut todo = Todo::new("Buy milk");
        todo.uid = EventUid::new("task-1@caldir");
        todo
    }

    /// **AC 1, 2 (§8.2).** The named risk: creating a second resource instead
    /// of updating the first. Asserted by count, not merely by content — a
    /// duplicate would pass any test that only checks the body.
    #[tokio::test]
    async fn updating_a_task_puts_once_to_the_resource_that_already_holds_it() {
        let server = MockServer::start().await;

        mount_report(
            &server,
            "calendar-multiget",
            multistatus("/cal/task-1@caldir.ics", "\"t-1\"", SERVER_HELD_VTODO_ICS),
        )
        .await;
        Mock::given(method("PUT"))
            .and(path("/cal/task-1@caldir.ics"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;

        let mut updated = local_todo();
        updated.summary = Some("Buy oat milk".to_string());

        update_item(
            "user",
            "pass",
            &calendar_url(&server),
            CalendarItem::Todo(updated),
        )
        .await
        .expect("updating a task succeeds");

        let puts = puts(&server).await;
        assert_eq!(puts.len(), 1, "exactly one PUT for a task update");
        assert_eq!(puts[0].url.path(), "/cal/task-1@caldir.ics");
        assert_eq!(
            puts[0].headers.get("If-Match").map(|v| v.to_str().unwrap()),
            Some("\"t-1\""),
            "the update must be conditional on the located etag"
        );
        assert!(
            !puts[0].headers.contains_key("If-None-Match"),
            "an update is not a create: If-None-Match would reject an existing resource"
        );

        let body = String::from_utf8_lossy(&puts[0].body);
        assert!(body.contains("BEGIN:VTODO"), "got: {body}");
        assert!(
            !body.contains("BEGIN:VEVENT"),
            "a task must never be rewritten as an event (CROSS-6), got: {body}"
        );
    }

    /// **AC 3 (`CALDAV-3`, `CALDAV-4`, `CALDAV-5`).** Four separate
    /// acceptance IDs over one code path — proving one and assuming the rest
    /// is how a `STATUS` that never serializes ships green. `PRIORITY` is
    /// included because `Todo::priority` is a raw `i32` that must not be
    /// routed through `icalendar::Component::priority()`.
    #[tokio::test]
    async fn a_changed_summary_due_status_or_priority_all_reach_the_one_resource() {
        struct Case {
            name: &'static str,
            apply: fn(&mut Todo),
            expect_contains: &'static str,
        }

        let cases = [
            Case {
                name: "summary",
                apply: |todo| todo.summary = Some("Buy oat milk".to_string()),
                expect_contains: "SUMMARY:Buy oat milk",
            },
            Case {
                name: "due",
                apply: |todo| {
                    todo.due = Some(EventTime::DateTimeUtc(
                        "2026-09-01T12:00:00Z"
                            .parse::<chrono::DateTime<chrono::Utc>>()
                            .expect("a valid timestamp"),
                    ))
                },
                expect_contains: "DUE:20260901T120000Z",
            },
            Case {
                name: "status",
                apply: |todo| todo.status = TodoStatus::Completed,
                expect_contains: "STATUS:COMPLETED",
            },
            Case {
                name: "priority",
                apply: |todo| todo.priority = Some(1),
                expect_contains: "PRIORITY:1",
            },
        ];

        for case in cases {
            let server = MockServer::start().await;

            mount_report(
                &server,
                "calendar-multiget",
                multistatus("/cal/task-1@caldir.ics", "\"t-1\"", SERVER_HELD_VTODO_ICS),
            )
            .await;
            Mock::given(method("PUT"))
                .and(path("/cal/task-1@caldir.ics"))
                .respond_with(ResponseTemplate::new(204))
                .mount(&server)
                .await;

            let mut todo = local_todo();
            (case.apply)(&mut todo);

            update_item(
                "user",
                "pass",
                &calendar_url(&server),
                CalendarItem::Todo(todo),
            )
            .await
            .unwrap_or_else(|err| panic!("case {}: update failed: {err}", case.name));

            let puts = puts(&server).await;
            assert_eq!(
                puts.len(),
                1,
                "case {}: exactly one PUT to the one resource",
                case.name
            );

            let body = String::from_utf8_lossy(&puts[0].body);
            assert!(
                body.contains(case.expect_contains),
                "case {}: expected {:?} in body, got: {body}",
                case.name,
                case.expect_contains
            );
        }
    }

    /// A `VEVENT` sharing the UID with the task under test, served at
    /// `{uid}.ics` — the fast path returns `None` for the wrong kind, and
    /// the UID-lookup REPORT resolves the task's real, server-assigned href.
    const SAME_UID_VEVENT_ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:task-1@caldir\r\nDTSTART:20260615T100000Z\r\nDTEND:20260615T110000Z\r\n\
SUMMARY:A meeting sharing this UID\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    /// **AC 4 (R60, R61).** The kind passed to `find_event_location` must
    /// come from the item being written, never a constant: this is the test
    /// that stops an item-typed rewrite from passing a constant kind, which
    /// would silently PUT a task over a meeting.
    #[tokio::test]
    async fn an_update_never_lands_on_a_same_uid_event() {
        let server = MockServer::start().await;

        mount_report(
            &server,
            "calendar-multiget",
            multistatus("/cal/task-1@caldir.ics", "\"e-1\"", SAME_UID_VEVENT_ICS),
        )
        .await;
        mount_report(
            &server,
            "calendar-query",
            multistatus(
                "/cal/server-assigned-task.ics",
                "\"t-9\"",
                SERVER_HELD_VTODO_ICS,
            ),
        )
        .await;
        Mock::given(method("PUT"))
            .and(path("/cal/server-assigned-task.ics"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;

        update_item(
            "user",
            "pass",
            &calendar_url(&server),
            CalendarItem::Todo(local_todo()),
        )
        .await
        .expect("the task update succeeds via the UID-lookup fallback");

        let puts = puts(&server).await;
        assert_eq!(puts.len(), 1, "exactly one PUT, to the located task href");
        assert_eq!(puts[0].url.path(), "/cal/server-assigned-task.ics");

        let reports = crate::caldav::ops::test_dav::received_bodies(&server).await;
        assert!(
            reports
                .iter()
                .any(|body| body.contains(r#"<C:comp-filter name="VTODO">"#)),
            "the UID-lookup REPORT must filter on VTODO for a task, got: {reports:?}"
        );
    }

    /// What the server holds after the PUT: a different `SUMMARY` and an
    /// `X-` property caldir never sent. Raw ICS, so the remote is an
    /// independent store rather than the encoder under test replaying itself.
    const SERVER_RETURNED_VTODO_ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTODO\r\nUID:task-1@caldir\r\nDTSTAMP:20260801T090000Z\r\n\
SUMMARY:Renamed by the server\r\nX-RADICALE-NAME:9f2c.ics\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

    /// **AC 5 (§8.4, §7).** The read-back arm must decode a `Todo` exactly as
    /// it decodes an `Event` — the shipped bug was
    /// `if let Ok(CalendarItem::Event(fetched_event)) = parse_item(..)`, which
    /// falls straight through for a task and silently returns the locally
    /// sent item. Getting it wrong means every second sync reports a change
    /// forever, because `P2-02` stores this value as the sync base.
    #[tokio::test]
    async fn update_returns_the_task_the_server_holds_not_the_one_sent() {
        let server = MockServer::start().await;

        Mock::given(method("PUT"))
            .and(path("/cal/task-1@caldir.ics"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        mount_report(
            &server,
            "calendar-multiget",
            multistatus(
                "/cal/task-1@caldir.ics",
                "\"t-1\"",
                SERVER_RETURNED_VTODO_ICS,
            ),
        )
        .await;

        let updated = update_item(
            "user",
            "pass",
            &calendar_url(&server),
            CalendarItem::Todo(local_todo()),
        )
        .await
        .expect("the update succeeds");

        let CalendarItem::Todo(updated) = updated else {
            panic!("expected the server's Todo back, got {updated:?}");
        };

        assert_eq!(
            updated.summary.as_deref(),
            Some("Renamed by the server"),
            "the read-back must win for a task exactly as it does for an event"
        );
        assert!(
            updated
                .x_properties
                .iter()
                .any(|prop| prop.name == "X-RADICALE-NAME"),
            "server-only properties must survive the read-back, got {:?}",
            updated.x_properties
        );
    }

    /// A `412 Precondition Failed` on the conditional `PUT` means the
    /// `If-Match` etag no longer matches — someone else changed the resource
    /// since it was read. This must surface as the named [`StaleEtag`] error,
    /// not the generic "server returned 412" bail, so the RPC boundary can
    /// attach `ErrorCode::Conflict` (§16, `W2-C3`) by downcasting.
    #[tokio::test]
    async fn a_412_on_the_conditional_put_surfaces_as_a_stale_etag() {
        let server = MockServer::start().await;

        mount_report(
            &server,
            "calendar-multiget",
            multistatus("/cal/task-1@caldir.ics", "\"t-1\"", SERVER_HELD_VTODO_ICS),
        )
        .await;
        Mock::given(method("PUT"))
            .and(path("/cal/task-1@caldir.ics"))
            .respond_with(ResponseTemplate::new(412))
            .mount(&server)
            .await;

        let err = update_item(
            "user",
            "pass",
            &calendar_url(&server),
            CalendarItem::Todo(local_todo()),
        )
        .await
        .expect_err("a 412 must not be treated as success");

        assert!(
            err.downcast_ref::<StaleEtag>().is_some(),
            "a 412 precondition failure must downcast to StaleEtag, got: {err}"
        );
    }
}
