//! List calendars under a CalDAV calendar home URL.

use anyhow::{Context, Result};
use http::{Method, Request, Uri};
use libdav::caldav::{CalendarComponent, FindCalendars, GetSupportedComponents};
use libdav::dav::{GetProperty, make_relative_url};
use libdav::names;
use libdav::requests::{DavRequest, ParseResponseError};

use crate::caldav::{CalDavClient_, absolute_url, create_caldav_client};

/// A raw calendar entry returned by CalDAV discovery.
pub struct RawCalendar {
    /// The href path of the calendar collection.
    pub href: String,
    /// Display name of the calendar.
    pub name: String,
    /// Calendar color (may be in #RRGGBBAA or #RRGGBB format depending on server).
    pub color: Option<String>,
    /// Absolute URL of the calendar.
    pub url: String,
    /// Whether the authenticated user lacks write access to this calendar,
    /// derived from `DAV:current-user-privilege-set`. `None` if the server
    /// did not return a privilege set (assume writable).
    pub read_only: Option<bool>,
    /// Whether the collection's `supported-calendar-component-set` names
    /// `VTODO`. Three-valued (C-14, R92): `None` when the property could not
    /// be read at all — absent, malformed, or the request itself failed —
    /// which must never collapse into `Some(false)`. This is advisory only
    /// and is never used to gate a request; it is recorded here purely so a
    /// later rejection can be classified against fresh evidence, not this
    /// stale snapshot (see `collection_openly_lacks_vtodo` in
    /// `caldav::ops::list_events`).
    pub supports_tasks: Option<bool>,
}

/// List all calendars under a calendar home URL.
///
/// Returns raw calendar data without provider-specific wrapping, so each
/// provider can apply its own color normalization and remote config.
pub async fn list_calendars_raw(
    username: &str,
    password: &str,
    calendar_home_url: &str,
) -> Result<Vec<RawCalendar>> {
    let caldav = create_caldav_client(calendar_home_url, username, password)?;

    let calendar_home_uri: http::Uri = calendar_home_url
        .parse()
        .context("Invalid calendar home URL")?;

    let response = caldav
        .request(FindCalendars::new(calendar_home_uri.path()))
        .await
        .context("Failed to list calendars")?;

    if response.calendars.is_empty() {
        anyhow::bail!("No calendars found for this account");
    }

    let mut calendars = Vec::new();

    for calendar in response.calendars {
        // Get displayname
        let display_name = caldav
            .request(GetProperty::new(&calendar.href, &names::DISPLAY_NAME))
            .await
            .ok()
            .and_then(|r| r.value)
            .unwrap_or_else(|| {
                calendar
                    .href
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or("Calendar")
                    .to_string()
            });

        // Get calendar color
        let color = caldav
            .request(GetProperty::new(&calendar.href, &names::CALENDAR_COLOUR))
            .await
            .ok()
            .and_then(|r| r.value);

        // Get current-user-privilege-set (RFC 3744). If the server doesn't
        // expose it, leave `read_only` as None so the calendar is treated as
        // writable by default — matches the existing behaviour.
        let read_only = caldav
            .request(GetCurrentUserPrivilegeSet::new(&calendar.href))
            .await
            .ok()
            .map(|privs| !is_writable_privilege_set(&privs));

        let url = absolute_url(&caldav, &calendar.href);
        let supports_tasks = fetch_supports_tasks(&caldav, &calendar.href).await;

        calendars.push(RawCalendar {
            href: calendar.href,
            name: display_name,
            color,
            url,
            read_only,
            supports_tasks,
        });
    }

    Ok(calendars)
}

/// Read a collection's `supported-calendar-component-set` and report whether
/// it names `VTODO`.
///
/// Three-valued, deliberately: `None` for anything short of a clean read
/// (absent property, malformed response, failed request) — collapsing that
/// into `Some(false)` reproduces R91's bug, where "absent = no VTODO" wiped
/// out task support for every existing user on upgrade. This value is
/// recorded for later classification only; it is never a gate (C-14).
async fn fetch_supports_tasks(caldav: &CalDavClient_, href: &str) -> Option<bool> {
    caldav
        .request(GetSupportedComponents::new(href))
        .await
        .ok()
        .map(|response| response.components.contains(&CalendarComponent::VTodo))
}

/// Decide whether a calendar is writable from the privileges returned by
/// `DAV:current-user-privilege-set`. We treat any of `all`, `write`, or
/// `bind` as sufficient — `bind` is what's needed to create new resources
/// in a collection, which is the relevant capability for pushing events.
fn is_writable_privilege_set(privileges: &[String]) -> bool {
    privileges
        .iter()
        .any(|p| matches!(p.as_str(), "all" | "write" | "bind"))
}

// ============================================================================
// Custom DAV request to fetch the current-user-privilege-set property
// ============================================================================

/// Request to fetch the `DAV:current-user-privilege-set` property (RFC 3744)
/// for a single resource.
///
/// Returns the list of privilege element names granted to the authenticated
/// user (e.g. `"read"`, `"write"`, `"write-content"`, `"bind"`, `"all"`).
struct GetCurrentUserPrivilegeSet<'a> {
    href: &'a str,
}

impl<'a> GetCurrentUserPrivilegeSet<'a> {
    fn new(href: &'a str) -> Self {
        Self { href }
    }
}

impl DavRequest for GetCurrentUserPrivilegeSet<'_> {
    type Response = Vec<String>;
    type ParseError = ParseResponseError;
    type Error<E> = libdav::dav::WebDavError<E>;

    fn prepare_request(&self, base_url: Uri) -> std::result::Result<Request<String>, http::Error> {
        let body = r#"<D:propfind xmlns:D="DAV:">
    <D:prop>
        <D:current-user-privilege-set/>
    </D:prop>
</D:propfind>"#
            .to_string();

        Request::builder()
            .method(Method::from_bytes(b"PROPFIND")?)
            .uri(make_relative_url(base_url, self.href)?)
            .header("Depth", "0")
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

        let mut privileges = Vec::new();

        for set in root
            .descendants()
            .filter(|n| n.tag_name().name() == "current-user-privilege-set")
        {
            for privilege in set
                .descendants()
                .filter(|n| n.tag_name().name() == "privilege")
            {
                for child in privilege.children().filter(|n| n.is_element()) {
                    privileges.push(child.tag_name().name().to_string());
                }
            }
        }

        Ok(privileges)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caldav::create_caldav_client;
    use crate::caldav::ops::test_dav::{
        calendar_url, component_set_multistatus, mount_propfind, mount_propfind_status,
    };
    use wiremock::MockServer;

    /// AC 3. Three-valued, not a `bool`: `Some(false)` (advertised, no
    /// `VTODO`) and `None` (unreadable) are the two states this ticket treats
    /// differently, and collapsing them would re-create R91's bug.
    #[tokio::test]
    async fn the_supported_component_set_is_parsed_onto_the_raw_calendar() {
        let server = MockServer::start().await;
        mount_propfind(
            &server,
            component_set_multistatus(&["VEVENT", "VTODO", "VJOURNAL"]),
        )
        .await;
        let caldav = create_caldav_client(&calendar_url(&server), "user", "pass").unwrap();

        let supports_tasks = fetch_supports_tasks(&caldav, "/cal/").await;
        assert_eq!(supports_tasks, Some(true));
    }

    #[tokio::test]
    async fn a_component_set_without_vtodo_yields_some_false() {
        let server = MockServer::start().await;
        mount_propfind(&server, component_set_multistatus(&["VEVENT"])).await;
        let caldav = create_caldav_client(&calendar_url(&server), "user", "pass").unwrap();

        let supports_tasks = fetch_supports_tasks(&caldav, "/cal/").await;
        assert_eq!(supports_tasks, Some(false));
    }

    #[tokio::test]
    async fn a_failed_propfind_yields_none_never_false() {
        let server = MockServer::start().await;
        mount_propfind_status(&server, 500).await;
        let caldav = create_caldav_client(&calendar_url(&server), "user", "pass").unwrap();

        let supports_tasks = fetch_supports_tasks(&caldav, "/cal/").await;
        assert_eq!(
            supports_tasks, None,
            "an unreadable component set must never collapse into false (R91)"
        );
    }

    #[test]
    fn writable_when_privilege_set_contains_all() {
        assert!(is_writable_privilege_set(&["all".to_string()]));
    }

    #[test]
    fn writable_when_privilege_set_contains_write() {
        assert!(is_writable_privilege_set(&[
            "read".to_string(),
            "write".to_string()
        ]));
    }

    #[test]
    fn writable_when_privilege_set_contains_bind() {
        assert!(is_writable_privilege_set(&[
            "read".to_string(),
            "bind".to_string()
        ]));
    }

    #[test]
    fn not_writable_when_only_read_privileges() {
        assert!(!is_writable_privilege_set(&[
            "read".to_string(),
            "read-current-user-privilege-set".to_string()
        ]));
    }

    #[test]
    fn not_writable_when_privilege_set_empty() {
        assert!(!is_writable_privilege_set(&[]));
    }
}
