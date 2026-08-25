//! Create a new event on a CalDAV calendar.

use anyhow::{Context, Result};
use caldir_core::Event;
use http::Request;
use libdav::caldav::GetCalendarResources;

use crate::caldav::{create_caldav_client, event_url, url_to_href};

use super::parse_item;

/// Create a new event on a CalDAV calendar.
///
/// Uses a raw PUT request to avoid duplicate Content-Type headers (a libdav quirk).
/// Fetches the created event back to get server-assigned values.
pub async fn create_event(
    username: &str,
    password: &str,
    calendar_url: &str,
    event: Event,
) -> Result<Event> {
    let caldav = create_caldav_client(calendar_url, username, password)?;

    let ics_content = event.to_ics_string();

    let full_url = event_url(calendar_url, event.uid.as_str());
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
        .body(ics_content)?;

    let (parts, _body) = caldav
        .request_raw(request)
        .await
        .context("Failed to create event")?;

    if !parts.status.is_success() {
        anyhow::bail!("Failed to create event: server returned {}", parts.status);
    }

    // Fetch the created event to get server-assigned values
    let calendar_href = url_to_href(calendar_url);
    let get_response = caldav
        .request(GetCalendarResources::new(&calendar_href).with_hrefs([&href]))
        .await
        .ok();

    if let Some(response) = get_response
        && let Some(resource) = response.resources.into_iter().next()
        && let Ok(content) = resource.content
        && let Ok(caldir_core::CalendarItem::Event(fetched_event)) = parse_item(&content.data)
    {
        return Ok(fetched_event);
    }

    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caldav::ops::test_dav::{calendar_url, mount_report, multistatus};
    use caldir_core::{EventTime, EventUid};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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
}
