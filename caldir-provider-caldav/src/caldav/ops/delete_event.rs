//! Delete an event from a CalDAV calendar.

use anyhow::Result;
use caldir_core::ComponentKind;

use crate::caldav::{create_caldav_client, url_to_href};

use super::find_event_location;

/// Delete an item from a CalDAV calendar.
///
/// Tries the standard `{uid}.ics` URL first, falls back to UID-based REPORT query.
/// Treats "not found" (already deleted) as success — and because the lookup is
/// kind-aware, a resource of the *wrong kind* at `{uid}.ics` reaches that same
/// branch, which is the correct outcome here: never delete somebody else's
/// resource.
pub async fn delete_event(
    username: &str,
    password: &str,
    calendar_url: &str,
    event_id: &str,
    kind: ComponentKind,
) -> Result<()> {
    let caldav = create_caldav_client(calendar_url, username, password)?;
    let calendar_href = url_to_href(calendar_url);

    // Find the event's actual href (try {uid}.ics first, then UID-based REPORT)
    let location = find_event_location(&caldav, &calendar_href, calendar_url, event_id, kind).await;

    let href = match location {
        Ok(loc) => loc.href,
        Err(_) => {
            // Event not found on server — treat as already deleted
            return Ok(());
        }
    };

    let result = caldav
        .request(libdav::dav::Delete::new(&href).force())
        .await;

    match result {
        Ok(_) => Ok(()),
        Err(e) => {
            let error_string = format!("{:?}", e);
            if error_string.contains("404") || error_string.contains("NOT_FOUND") {
                Ok(())
            } else {
                Err(anyhow::anyhow!("Failed to delete event: {}", e))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caldav::ops::test_dav::{calendar_url, mount_report, multistatus};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A `VEVENT` holding the task's own UID — the same-UID pair `C-15` calls
    /// out as synthetic but load-bearing to test: the one shape a kind-blind
    /// delete cannot tell apart from the task it was asked to remove.
    const SAME_UID_VEVENT_ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:task-1@caldir\r\nDTSTART:20260615T100000Z\r\nDTEND:20260615T110000Z\r\n\
SUMMARY:Somebody's meeting\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    /// The server-held `VTODO` for the same UID, at a server-assigned href —
    /// what the UID-filtered fallback REPORT resolves to.
    const SERVER_HELD_VTODO_ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTODO\r\nUID:task-1@caldir\r\nDTSTAMP:20260801T090000Z\r\n\
SUMMARY:Hand in the assignment\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

    /// A plain `VEVENT`, own UID, sitting at its own `{uid}.ics`.
    const EVENT_ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:evt-1@caldir\r\nDTSTART:20260615T100000Z\r\nDTEND:20260615T110000Z\r\n\
SUMMARY:Team sync\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    /// A `207 Multi-Status` with no `<response>` element at all — what a
    /// `calendar-query` REPORT answers when nothing matches the UID filter.
    const EMPTY_MULTISTATUS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<multistatus xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"/>"#;

    /// Every `DELETE` the double received, in arrival order.
    async fn deletes(server: &MockServer) -> Vec<wiremock::Request> {
        server
            .received_requests()
            .await
            .expect("the double records requests")
            .into_iter()
            .filter(|request| request.method == http::Method::DELETE)
            .collect()
    }

    /// **AC 2, `CALDAV-6`.** A same-UID `VEVENT` sits at `{uid}.ics`; the
    /// task itself lives at a server-assigned href, found only by the
    /// `VTODO`-filtered fallback. A kind-blind delete would find *a* resource
    /// at `{uid}.ics`, delete it, and report success while the user's
    /// meeting is gone and the task is not — the one destructive asymmetry
    /// in the whole wave.
    #[tokio::test]
    async fn deleting_a_task_leaves_a_same_uid_event_alone() {
        const TASK_UID_HREF: &str = "/cal/task-1@caldir.ics";
        const TASK_ASSIGNED_HREF: &str = "/cal/other-task-href.ics";

        let server = MockServer::start().await;

        // The fast path: a VEVENT, not the task, sits at {uid}.ics.
        mount_report(
            &server,
            "calendar-multiget",
            multistatus(TASK_UID_HREF, "\"evt-9\"", SAME_UID_VEVENT_ICS),
        )
        .await;
        // The VTODO-filtered fallback resolves the task's real location.
        mount_report(
            &server,
            r#"<C:comp-filter name="VTODO">"#,
            multistatus(TASK_ASSIGNED_HREF, "\"tsk-9\"", SERVER_HELD_VTODO_ICS),
        )
        .await;
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;

        delete_event(
            "user",
            "pass",
            &calendar_url(&server),
            "task-1@caldir",
            ComponentKind::Todo,
        )
        .await
        .expect("deleting the task succeeds");

        let deletes = deletes(&server).await;
        assert_eq!(
            deletes.len(),
            1,
            "exactly one DELETE, got {:?}",
            deletes.iter().map(|r| r.url.path()).collect::<Vec<_>>()
        );
        assert_eq!(
            deletes[0].url.path(),
            TASK_ASSIGNED_HREF,
            "the DELETE must target the task's own href"
        );
        assert!(
            deletes.iter().all(|r| r.url.path() != TASK_UID_HREF),
            "the same-UID event at {TASK_UID_HREF} must never be deleted"
        );
    }

    /// **AC 3, §16.** No resource anywhere holds the UID — the fallback
    /// REPORT answers an empty `multistatus`. §16's "remote task not found"
    /// class is a success, and the count assertion is what stops a
    /// "delete whatever is at `{uid}.ics` anyway" fallback from being added
    /// as a convenience.
    #[tokio::test]
    async fn a_task_already_absent_from_the_server_is_a_successful_delete() {
        let server = MockServer::start().await;

        // No calendar-multiget double: {uid}.ics answers with nothing this
        // fixture recognises, so the fast path falls through.
        mount_report(
            &server,
            r#"<C:comp-filter name="VTODO">"#,
            EMPTY_MULTISTATUS.to_string(),
        )
        .await;

        delete_event(
            "user",
            "pass",
            &calendar_url(&server),
            "task-1@caldir",
            ComponentKind::Todo,
        )
        .await
        .expect("an already-absent resource is a successful delete");

        assert_eq!(
            deletes(&server).await.len(),
            0,
            "an absent resource must never trigger a DELETE"
        );
    }

    /// **AC 5, `CALDAV-8`.** The event control: the fast path finds the
    /// `VEVENT` at its own `{uid}.ics`, and the delete must stay a single
    /// lookup, single `DELETE` — byte-for-byte what shipped before VTODO
    /// support existed.
    #[tokio::test]
    async fn deleting_an_event_still_targets_its_own_href() {
        const EVENT_HREF: &str = "/cal/evt-1@caldir.ics";

        let server = MockServer::start().await;

        mount_report(
            &server,
            "calendar-multiget",
            multistatus(EVENT_HREF, "\"evt-1\"", EVENT_ICS),
        )
        .await;
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;

        delete_event(
            "user",
            "pass",
            &calendar_url(&server),
            "evt-1@caldir",
            ComponentKind::Event,
        )
        .await
        .expect("deleting the event succeeds");

        let deletes = deletes(&server).await;
        assert_eq!(deletes.len(), 1, "exactly one DELETE for an event delete");
        assert_eq!(deletes[0].url.path(), EVENT_HREF);
    }

    /// **AC 4 (R60, R61).** The UID-lookup REPORT for a delete is
    /// component-filtered and carries no `<C:time-range>` — a range would
    /// hide an undated or long-completed task from its own deletion, leaving
    /// an orphan resource that the next pull recreates locally.
    #[tokio::test]
    async fn the_uid_lookup_for_a_delete_filters_on_the_requested_component() {
        let server = MockServer::start().await;

        // No calendar-multiget double for either uid: the fast path always
        // falls through to the UID-lookup REPORT under test.
        mount_report(&server, "REPORT", EMPTY_MULTISTATUS.to_string()).await;

        let _ = delete_event(
            "user",
            "pass",
            &calendar_url(&server),
            "task-1@caldir",
            ComponentKind::Todo,
        )
        .await;
        let _ = delete_event(
            "user",
            "pass",
            &calendar_url(&server),
            "evt-1@caldir",
            ComponentKind::Event,
        )
        .await;

        let bodies = crate::caldav::ops::test_dav::received_bodies(&server).await;
        let report_bodies: Vec<&String> = bodies
            .iter()
            .filter(|body| body.contains("calendar-query"))
            .collect();

        assert!(
            report_bodies
                .iter()
                .any(|body| body.contains(r#"<C:comp-filter name="VTODO">"#)),
            "the task lookup must filter on VTODO, got: {report_bodies:?}"
        );
        assert!(
            report_bodies
                .iter()
                .any(|body| body.contains(r#"<C:comp-filter name="VEVENT">"#)),
            "the event lookup must filter on VEVENT, got: {report_bodies:?}"
        );
        assert!(
            report_bodies
                .iter()
                .all(|body| !body.contains("time-range")),
            "a delete lookup must never carry a time-range, got: {report_bodies:?}"
        );
    }
}
