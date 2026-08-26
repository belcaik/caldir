//! `B-6` (0.15.0 window). These tests live outside the crate on purpose: an
//! integration test is compiled as a **separate crate**, so it sees exactly
//! what a downstream consumer sees. Everything here is a compile-level
//! property — naming an error type and matching its variants — which is why
//! the value under test is obtained through the real public API rather than
//! constructed, and why each `match` carries the wildcard arm that
//! `#[non_exhaustive]` (`B-4`) requires of every consumer outside the crate.

use caldir_core::{
    Calendar, CalendarError, CalendarEventError, CalendarItem, Event, EventTime,
    ParticipationStatus, RemoteError,
};

/// `CalendarError` is the error type of the public `Calendar::events()`,
/// `Calendar::create()` and `SkippedEvent::error()`. Before the re-export a
/// consumer could only `Display` it through method resolution; it could not
/// name it, so `#[non_exhaustive]` on it had nothing to constrain.
#[test]
fn a_downstream_crate_can_name_and_match_calendar_error() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("work");

    Calendar::create(&path, None).unwrap();
    let err = Calendar::create(&path, None).expect_err("the second create collides");

    let described = match &err {
        CalendarError::AlreadyExists(at) => format!("already exists at {}", at.display()),
        CalendarError::NotFound(_) => "not found".to_string(),
        // The arm `#[non_exhaustive]` forces on every downstream match.
        _ => "unrecognised".to_string(),
    };

    assert_eq!(described, format!("already exists at {}", path.display()));
}

/// `CalendarEventError` is the error type of the public `CalendarItem::load`
/// and `CalendarEvent::load`, yet it was `pub(crate) use`d.
#[test]
fn a_downstream_crate_can_name_and_match_calendar_event_error() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("missing.ics");

    let err = CalendarItem::load(&missing).expect_err("no such file");

    let described = match &err {
        CalendarEventError::NotFound(at) => format!("not found: {}", at.display()),
        CalendarEventError::NotAnEvent => "not an event".to_string(),
        _ => "unrecognised".to_string(),
    };

    assert_eq!(described, format!("not found: {}", missing.display()));
}

/// `RemoteError` is the error type of every public `Remote` method. It cannot
/// be built directly from outside — both payload types are still crate-private
/// — so this drives it through the public `From<EventError>` conversion that
/// `#[from]` generates, which needs no name for the source type.
#[test]
fn a_downstream_crate_can_name_and_match_remote_error() {
    let mut event = Event::new(
        "Standup",
        EventTime::Date(chrono::NaiveDate::from_ymd_opt(2026, 8, 14).unwrap()),
    );

    let event_error = event
        .set_attendee_status("nobody@example.com", ParticipationStatus::Accepted)
        .expect_err("nobody is on the invite");

    let err: RemoteError = event_error.into();

    let described = match &err {
        RemoteError::Provider(_) => "provider",
        RemoteError::UnsupportedItem(_) => "unsupported item",
        _ => "unrecognised",
    };

    assert_eq!(described, "unsupported item");
    assert!(err.to_string().contains("nobody@example.com"), "{err}");
}
