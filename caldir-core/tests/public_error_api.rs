//! `B-6` (0.15.0 window). These tests live outside the crate on purpose: an
//! integration test is compiled as a **separate crate**, so it sees exactly
//! what a downstream consumer sees. Everything here is a compile-level
//! property — naming an error type and matching its variants — which is why
//! the value under test is obtained through the real public API rather than
//! constructed, and why each `match` carries the wildcard arm that
//! `#[non_exhaustive]` (`B-4`) requires of every consumer outside the crate.

use caldir_core::provider::ProviderCapabilities;
use caldir_core::{
    Calendar, CalendarError, CalendarEventError, CalendarItem, ConnectionError, Event, EventError,
    EventTime, ParticipationStatus, RemoteError,
};

/// D-5's acceptance test for the whole capabilities effort: an out-of-tree
/// provider (Proton) needs no in-repo source. A `pub(crate)` slip, or a
/// `#[non_exhaustive]` that blocks struct-literal construction, is invisible
/// from inside `caldir-core` and fatal from here.
#[test]
fn a_downstream_crate_can_name_and_construct_provider_capabilities() {
    let capabilities = ProviderCapabilities {
        events: caldir_core::provider::Capability {
            read: true,
            write: true,
        },
        tasks: caldir_core::provider::Capability {
            read: true,
            write: false,
        },
    };

    assert!(capabilities.events.read);
    assert!(capabilities.events.write);
    assert!(capabilities.tasks.read);
    assert!(!capabilities.tasks.write);
}

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

/// `RemoteError` is the error type of every public `Remote` method. This
/// test drives its `UnsupportedItem` arm through the public
/// `From<EventError>` conversion `#[from]` generates — the `Provider` arm's
/// payload (`ProviderError`) is exercised separately, below
/// (`a_downstream_crate_distinguishes_the_six_error_classes_on_provider_error`,
/// W2-C1). `EventError` itself is now nameable — see the two tests below,
/// which match *into* the `UnsupportedItem` payload rather than only past it.
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

/// `EventError` is the payload of the public `RemoteError::UnsupportedItem`
/// (R71) and the error type of `Event::set_attendee_status`, yet `mod event`
/// was private and `lib.rs` re-exported no name for it. A consumer could hold
/// an `UnsupportedItem` and still not write a `match` over what went wrong.
#[test]
fn a_downstream_crate_can_name_and_match_event_error() {
    let mut event = Event::new(
        "Standup",
        EventTime::Date(chrono::NaiveDate::from_ymd_opt(2026, 8, 14).unwrap()),
    );

    let err: EventError = event
        .set_attendee_status("nobody@example.com", ParticipationStatus::Accepted)
        .expect_err("nobody is on the invite");

    let described = match &err {
        EventError::AttendeeNotFound { email } => format!("no attendee {email}"),
        EventError::UnsupportedComponent(name) => format!("unsupported {name}"),
        // The arm `#[non_exhaustive]` forces on every downstream match.
        _ => "unrecognised".to_string(),
    };

    assert_eq!(described, "no attendee nobody@example.com");
}

/// The R71 contract, end to end: a consumer that catches `UnsupportedItem` can
/// destructure the `EventError` inside it and branch on the component that was
/// refused. This is the match `public_error_api` could not write before.
#[test]
fn a_downstream_crate_can_match_inside_remote_error_unsupported_item() {
    let unsupported = CalendarItem::from_single_ics_str(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VJOURNAL\r\nUID:j-1\r\n\
         END:VJOURNAL\r\nEND:VCALENDAR\r\n",
    )
    .expect_err("a VJOURNAL is not a supported component");

    let err: RemoteError = unsupported.into();

    let named = match &err {
        RemoteError::UnsupportedItem(EventError::UnsupportedComponent(name)) => name.clone(),
        RemoteError::UnsupportedItem(_) => "other event error".to_string(),
        _ => "unrecognised".to_string(),
    };

    assert_eq!(named, "VJOURNAL");
}

/// W2-C1: `P2-04` built the six §16 classes so the CLI could distinguish
/// provider failures without matching on the `Display` string, but
/// `ProviderError` was re-exported `pub(crate)` — unnameable, so every
/// downstream caller could only reach the string the taxonomy was built to
/// replace. `ProviderError`'s data variants carry no invariants, so
/// constructing them directly here is the same shape as the in-crate sibling
/// test `ConnectionError`'s `a_missing_scope_failure_is_distinguishable_
/// without_string_matching`, just now legal from outside the crate.
/// `ProviderTransportError` is exposed alongside it (also `#[non_exhaustive]`)
/// so the `network` class — carried inside `ProviderError::Transport` rather
/// than as its own top-level variant — is distinguishable from an unrelated
/// local subprocess failure, not just lumped in with it.
#[test]
fn a_downstream_crate_distinguishes_the_six_error_classes_on_provider_error() {
    use caldir_core::provider::{ProviderError, ProviderTransportError};

    fn describe(err: &ProviderError) -> &'static str {
        match err {
            ProviderError::UnsupportedOperation(_) => "unsupported_operation",
            ProviderError::MissingAuthScope(_) => "missing_auth_scope",
            ProviderError::RemoteNotFound(_) => "remote_not_found",
            ProviderError::Conflict(_) => "conflict",
            ProviderError::SemanticLimitation(_) => "semantic_limitation",
            ProviderError::Transport(ProviderTransportError::Network(_)) => "network",
            // The arm `#[non_exhaustive]` forces on every downstream match —
            // on both enums.
            _ => "some other failure",
        }
    }

    let cases: [(ProviderError, &str); 6] = [
        (
            ProviderError::UnsupportedOperation("task writes unsupported".into()),
            "unsupported_operation",
        ),
        (
            ProviderError::MissingAuthScope("reauth required".into()),
            "missing_auth_scope",
        ),
        (
            ProviderError::RemoteNotFound("event gone".into()),
            "remote_not_found",
        ),
        (ProviderError::Conflict("stale etag".into()), "conflict"),
        (
            ProviderError::SemanticLimitation("field truncated".into()),
            "semantic_limitation",
        ),
        (
            ProviderError::Transport(ProviderTransportError::Network("dns failure".into())),
            "network",
        ),
    ];

    for (provider_error, expected_class) in cases {
        assert_eq!(describe(&provider_error), expected_class);

        // The class survives the real conversion path a `Remote` failure
        // takes, not just a bare `ProviderError` in isolation.
        let remote_error: RemoteError = provider_error.into();
        match &remote_error {
            RemoteError::Provider(inner) => {
                assert_eq!(describe(inner), expected_class);
            }
            RemoteError::UnsupportedItem(_) => panic!("expected a provider failure"),
            _ => panic!("unrecognised RemoteError variant"),
        }
    }
}

/// W2-C3: `W2-C1` let a *consumer* outside the crate distinguish the six §16
/// classes on `ProviderError`, but nothing let a *producer* attach one —
/// `Classified` was `pub(crate)` to `handler.rs`, reachable only by the
/// trait's own `unsupported_operation` defaults. This is the reachability
/// half of that fix: `classified_error` must be nameable and callable from a
/// separately compiled crate, the same bar `a_downstream_crate_distinguishes_
/// the_six_error_classes_on_provider_error` set for `ProviderError` itself.
/// The wire round trip that proves the attached class actually arrives as
/// the matching variant lives in-crate, where the transport mock is
/// available (`provider::handler::tests::
/// a_handler_attached_error_class_survives_the_wire_round_trip`); this test
/// only proves the constructor compiles and runs from outside.
#[test]
fn a_downstream_crate_can_construct_a_classified_error() {
    use caldir_core::provider::classified_error;
    use caldir_core::rpc::ErrorCode;

    let err = classified_error(ErrorCode::Conflict, "stale etag");

    assert_eq!(err.to_string(), "stale etag");
}

/// `ConnectionError` is the error type of every public `Connection` method —
/// `diff`, `apply_outgoing_diff`, `discard_outgoing_diff` — and was unnameable
/// downstream too. Driven through the `From<CalendarError>`
/// conversion `#[from]` generates, which needs no name for the source type.
#[test]
fn a_downstream_crate_can_name_and_match_connection_error() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("work");

    Calendar::create(&path, None).unwrap();
    let calendar_error = Calendar::create(&path, None).expect_err("the second create collides");

    let err: ConnectionError = calendar_error.into();

    let described = match &err {
        ConnectionError::Calendar(inner) => format!("calendar: {inner}"),
        ConnectionError::Remote(_) => "remote".to_string(),
        _ => "unrecognised".to_string(),
    };

    assert!(described.starts_with("calendar: "), "{described}");
    assert!(
        described.contains(&path.display().to_string()),
        "{described}"
    );
}

/// The attribute itself, not just the wildcard it forces.
///
/// A `_` arm alongside a *complete* variant list is unreachable — unless the
/// enum is `#[non_exhaustive]`, which keeps it live for the variants a future
/// version adds. `deny(unreachable_patterns)` therefore turns "someone dropped
/// the attribute" into a compile error here, downstream, which is the only
/// place the attribute has any meaning at all. Without this the three
/// attributes could be deleted and every runtime test would still pass.
///
/// Adding a variant upstream never breaks this: the `_` arm only goes
/// unreachable when the enum stops being open.
#[test]
#[deny(unreachable_patterns)]
fn the_three_error_enums_are_open_to_downstream_additions() {
    fn probe_event_error(err: &EventError) -> &'static str {
        match err {
            EventError::Io(..)
            | EventError::InvalidIcs(..)
            | EventError::UnexpectedEventCount { .. }
            | EventError::UnexpectedComponentCount { .. }
            | EventError::UnsupportedComponent(_)
            | EventError::MissingStart
            | EventError::MissingUid
            | EventError::UnparseableProperty { .. }
            | EventError::AttendeeNotFound { .. } => "known",
            // Live only because `EventError` is `#[non_exhaustive]`.
            _ => "added after this test was written",
        }
    }

    fn probe_calendar_event_error(err: &CalendarEventError) -> &'static str {
        match err {
            CalendarEventError::InvalidEvent(..)
            | CalendarEventError::ExpectedSingleEvent { .. }
            | CalendarEventError::NotAnEvent
            | CalendarEventError::ExpectedSingleComponent { .. }
            | CalendarEventError::NoSupportedComponent { .. }
            | CalendarEventError::Io(_)
            | CalendarEventError::NotFound(_)
            | CalendarEventError::Event(_)
            | CalendarEventError::NotRecurring(_) => "known",
            _ => "added after this test was written",
        }
    }

    fn probe_connection_error(err: &ConnectionError) -> &'static str {
        match err {
            ConnectionError::Remote(_) | ConnectionError::Calendar(_) => "known",
            _ => "added after this test was written",
        }
    }

    // Values of the variants that *do* exist never reach the open arm.
    assert_eq!(probe_event_error(&EventError::MissingUid), "known");
    assert_eq!(
        probe_calendar_event_error(&CalendarEventError::NotAnEvent),
        "known"
    );

    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("work");
    Calendar::create(&path, None).unwrap();
    let err: ConnectionError = Calendar::create(&path, None).unwrap_err().into();
    assert_eq!(probe_connection_error(&err), "known");
}

/// The same lock for `CalendarItem`, the one open enum that is not an error.
///
/// Sibling of `the_three_error_enums_are_open_to_downstream_additions`, kept
/// separate only so that test's name stays truthful. `CalendarItem` is
/// `#[non_exhaustive]` for the same reason the error enums are: adding
/// `VJOURNAL` later must be one more variant, not a breaking change. Until
/// this existed the attribute could be deleted and `cargo test` stayed green —
/// only CI's `-D warnings` caught it, and then incidentally.
#[test]
#[deny(unreachable_patterns)]
fn calendar_item_is_open_to_downstream_additions() {
    fn probe(item: &CalendarItem) -> &'static str {
        match item {
            CalendarItem::Event(_) | CalendarItem::Todo(_) => "known",
            // Live only because `CalendarItem` is `#[non_exhaustive]`.
            _ => "added after this test was written",
        }
    }

    let event = CalendarItem::from_single_ics_str(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:e-1\r\n\
         DTSTART:20260901T090000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
    )
    .expect("a VEVENT is a supported component");
    let task = CalendarItem::from_single_ics_str(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTODO\r\nUID:t-1\r\n\
         DUE:20260905T170000Z\r\nEND:VTODO\r\nEND:VCALENDAR\r\n",
    )
    .expect("a VTODO is a supported component");

    // Values of the variants that *do* exist never reach the open arm.
    assert_eq!(probe(&event), "known");
    assert_eq!(probe(&task), "known");
}
