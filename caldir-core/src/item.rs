use crate::calendar::CalendarEventError;
use crate::event::{Event, EventError, EventInstanceId, EventUid};
use crate::todo::Todo;
use chrono::{DateTime, Utc};
use icalendar::{CalendarComponent, Component};
use std::path::PathBuf;

/// One supported calendar component parsed from an ICS document.
///
/// An item's kind comes solely from the component it was parsed from and is
/// preserved across every boundary. Adding `VJOURNAL` later is one more
/// variant, one more struct and one more match arm.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CalendarItem {
    Event(Event),
    Todo(Todo),
}

/// Components that are neither supported nor an error: ignored for counting
/// and never reported as unsupported.
const IGNORED_COMPONENTS: [&str; 1] = ["VTIMEZONE"];

impl CalendarItem {
    /// Parse an ICS document into its supported items, in document order.
    ///
    /// One ordered pass over `icalendar::Calendar::components` — never
    /// `events()` then `todos()`, which would lose document order.
    /// An unsupported component beside a supported one is ignored — `main`
    /// ignored it too, and it carries no identity the diff could read as a
    /// delete. A document whose components are *all* unsupported still aborts
    /// with an error naming them, because yielding nothing is what feeds the
    /// delete path.
    pub fn from_ics_str(contents: &str) -> Result<Vec<Result<Self, EventError>>, EventError> {
        Ok(Self::parse_document(contents)?.items)
    }

    /// Parse an ICS document holding exactly one supported component.
    ///
    /// The path-less arity check, for RPC payloads and sync bases. The "none"
    /// case is not given its own variant here: bases are always written by
    /// caldir, and on the wire an unsupported component is already an error
    /// naming itself.
    pub fn from_single_ics_str(contents: &str) -> Result<Self, EventError> {
        let items = Self::from_ics_str(contents)?;

        match <[Result<Self, EventError>; 1]>::try_from(items) {
            Ok([result]) => result,
            Err(items) => Err(EventError::UnexpectedComponentCount {
                expected: 1,
                found: items.len(),
            }),
        }
    }

    /// Load the single supported item from an `.ics` file on disk.
    ///
    /// The path-flavoured arity check, which distinguishes "no supported
    /// component" from "too many" so the message can tell a VTODO-only file
    /// from an empty `VCALENDAR`.
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, CalendarEventError> {
        Self::load_with_ignored(path).map(|(item, _)| item)
    }

    /// [`Self::load`], plus the names of the unsupported components that sat
    /// beside the item and were ignored.
    ///
    /// The ignored list is the other half of sibling tolerance: the item
    /// loads, but nothing is dropped *silently* — `doctor` renders these so the user
    /// learns the file holds data caldir will not preserve if it rewrites it
    /// (`write_best_event_file` serializes one item and nothing else).
    pub(crate) fn load_with_ignored(
        path: impl Into<PathBuf>,
    ) -> Result<(Self, Vec<String>), CalendarEventError> {
        let path = path.into();

        if !path.is_file() {
            return Err(CalendarEventError::NotFound(path));
        }

        let contents = std::fs::read_to_string(&path)?;

        let parsed = Self::parse_document(&contents)
            .map_err(|err| CalendarEventError::InvalidEvent(path.clone(), err))?;

        let found_description = parsed.found_description();
        let ignored = parsed.ignored;

        match <[Result<Self, EventError>; 1]>::try_from(parsed.items) {
            Ok([result]) => result
                .map(|item| (item, ignored))
                .map_err(|err| CalendarEventError::InvalidEvent(path, err)),
            Err(items) if items.is_empty() => Err(CalendarEventError::NoSupportedComponent {
                path,
                found: found_description,
            }),
            Err(items) => Err(CalendarEventError::ExpectedSingleComponent {
                path,
                found: items.len(),
            }),
        }
    }

    /// The item's UID, from whichever kind backs it.
    pub fn uid(&self) -> &EventUid {
        match self {
            CalendarItem::Event(event) => &event.uid,
            CalendarItem::Todo(todo) => &todo.uid,
        }
    }

    /// The item's identity: `(kind, uid, recurrence_id)`. The kind is part of
    /// the key, so a VEVENT and a VTODO sharing a UID never collide.
    pub fn instance_id(&self) -> EventInstanceId {
        match self {
            CalendarItem::Event(event) => event.event_instance_id(),
            CalendarItem::Todo(todo) => {
                EventInstanceId::todo(todo.uid.clone(), todo.recurrence_id.clone())
            }
        }
    }

    /// The event backing this item, or `None` for any other kind.
    ///
    /// Test-only: the sync seams are item-typed end to end, so production never
    /// narrows here. It survives for the test-side base accessor, which asserts
    /// against an `Event`.
    #[cfg(test)]
    pub(crate) fn as_event(&self) -> Option<&Event> {
        match self {
            CalendarItem::Event(event) => Some(event),
            CalendarItem::Todo(_) => None,
        }
    }

    /// Whether the item is `STATUS:CANCELLED`, in whichever kind's own value
    /// set.
    ///
    /// `CANCELLED` **only**. `COMPLETED` is not a tombstone: a task can be
    /// completed on the server and still have to be created locally, and the
    /// MVP's own fixture is `STATUS:COMPLETED`. Folding it in here would make
    /// a completed task un-pullable.
    pub(crate) fn is_cancelled(&self) -> bool {
        match self {
            CalendarItem::Event(event) => event.status == crate::event::Status::Cancelled,
            CalendarItem::Todo(todo) => todo.status == crate::todo::TodoStatus::Cancelled,
        }
    }

    /// `SEQUENCE`, the tiebreak direction detection falls back on when the
    /// remote reports no `LAST-MODIFIED`.
    pub(crate) fn sequence(&self) -> i32 {
        match self {
            CalendarItem::Event(event) => event.sequence,
            CalendarItem::Todo(todo) => todo.sequence,
        }
    }

    /// Narrow to an `Event` for the event-only RPC wire.
    ///
    /// Not a drop: the `Err` is the arity error the wire already reports, so a
    /// task arriving here fails loudly instead of being pushed as something
    /// else.
    ///
    /// **This is a backstop, not the path a user travels.** Task changes are
    /// withheld when the diff is built, so a task never enters the outgoing
    /// set and this error never surfaces: the push reports what it held back
    /// and the calendar's events sync as usual. What remains here guards the
    /// seam itself — pushing a task as an event would be a silent corruption,
    /// and dropping it would be a silent data loss, so the narrowing has to
    /// stay fallible.
    ///
    /// Locked by `remote::tests::a_task_change_aborts_instead_of_reaching_the_provider`
    /// and its `_delete_` sibling.
    pub(crate) fn into_event(self) -> Result<Event, EventError> {
        match self {
            CalendarItem::Event(event) => Ok(event),
            CalendarItem::Todo(_) => Err(EventError::UnexpectedEventCount {
                expected: 1,
                found: 0,
            }),
        }
    }

    /// The item's `SUMMARY`, from whichever kind backs it.
    pub fn summary(&self) -> Option<&str> {
        match self {
            CalendarItem::Event(event) => event.summary.as_deref(),
            CalendarItem::Todo(todo) => todo.summary.as_deref(),
        }
    }

    /// The filename stem for this item, from the kind's own slug rule. Never a
    /// shared rule: an event's stem must stay byte-identical while a task takes
    /// the reserved `_task__` prefix.
    pub(crate) fn base_slug(&self) -> String {
        match self {
            CalendarItem::Event(event) => event.base_slug(),
            CalendarItem::Todo(todo) => todo.base_slug(),
        }
    }

    /// Serialize to a single-component `VCALENDAR` document, from
    /// the kind's own serializer.
    pub(crate) fn to_ics_string(&self) -> String {
        match self {
            CalendarItem::Event(event) => event.to_ics_string(),
            CalendarItem::Todo(todo) => todo.to_ics_string(),
        }
    }

    /// Whether this item falls inside `[from, to]` — the in-window predicate
    /// the diff's delete/create decision hangs on.
    ///
    /// One predicate per kind, never one shared rule. Events keep
    /// [`Event::has_occurrence_in_range`] byte for byte; tasks get
    /// RFC 4791 §9.9's "core's rule" via [`Todo::occurs_in_range`].
    ///
    /// Polarity, because it is easy to state backwards: `false` makes
    /// the caller `continue` — **neither** a delete **nor** a create — while
    /// `true` is what *authorizes* a delete. An undated task is unconditionally
    /// `true`, so it is the CalDAV query being bare of `<C:time-range>`,
    /// not this predicate, that keeps it from being deleted.
    pub(crate) fn has_occurrence_in_range(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> bool {
        match self {
            CalendarItem::Event(event) => event.has_occurrence_in_range(from, to),
            CalendarItem::Todo(todo) => todo.occurs_in_range(from, to),
        }
    }

    /// `LAST-MODIFIED`, which the writer pins the file mtime to so direction
    /// detection reflects when the item changed, not when bytes hit disk.
    pub(crate) fn last_modified(&self) -> Option<DateTime<Utc>> {
        match self {
            CalendarItem::Event(event) => event.last_modified,
            CalendarItem::Todo(todo) => todo.last_modified,
        }
    }

    fn parse_document(contents: &str) -> Result<ParsedDocument, EventError> {
        let icalendar: icalendar::Calendar = contents
            .parse()
            .map_err(|err| EventError::InvalidIcs(contents.to_string(), err))?;

        let mut parsed = ParsedDocument::default();

        for component in &icalendar.components {
            let name = component_name(component);
            parsed.component_names.push(name.clone());

            match component {
                CalendarComponent::Event(event) => {
                    parsed
                        .items
                        .push(Event::try_from(event).map(CalendarItem::Event));
                }
                CalendarComponent::Todo(todo) => {
                    parsed
                        .items
                        .push(Todo::try_from(todo).map(CalendarItem::Todo));
                }
                // Ignored for counting, and never reported as unsupported.
                _ if IGNORED_COMPONENTS.contains(&name.as_str()) => {}
                // Recorded, never dropped on the floor: the pass survives so a
                // supported sibling still loads, and the name is handed back
                // for `doctor` to report.
                _ => parsed.ignored.push(name),
            }
        }

        // The property being defended is that a document never *silently
        // yields nothing*. That is the zero-supported case, and it stays an error
        // naming what was there. A supported component beside unsupported
        // siblings carries its own identity, so ignoring the siblings cannot
        // read to the diff as a delete — and `main` ignored them for years.
        if parsed.items.is_empty() && !parsed.ignored.is_empty() {
            return Err(EventError::UnsupportedComponent(parsed.ignored.join(", ")));
        }

        Ok(parsed)
    }
}

impl From<Event> for CalendarItem {
    fn from(event: Event) -> Self {
        CalendarItem::Event(event)
    }
}

impl From<Todo> for CalendarItem {
    fn from(todo: Todo) -> Self {
        CalendarItem::Todo(todo)
    }
}

#[derive(Default)]
struct ParsedDocument {
    /// Supported items, in document order.
    items: Vec<Result<CalendarItem, EventError>>,
    /// Every component name seen, in document order — ignored ones included,
    /// so the "no supported component" error can say what *was* there.
    component_names: Vec<String>,
    /// The unsupported components that were ignored, in document order.
    /// `VTIMEZONE` is not one of them: it is expected furniture, not something
    /// the user needs telling about.
    ignored: Vec<String>,
}

impl ParsedDocument {
    /// The `{names}` half of the "no supported component" message: the
    /// components present, or the literal `nothing` for a component-less
    /// `VCALENDAR`.
    fn found_description(&self) -> String {
        if self.component_names.is_empty() {
            return "nothing".to_string();
        }

        self.component_names.join(", ")
    }
}

fn component_name(component: &CalendarComponent) -> String {
    match component {
        CalendarComponent::Event(component) => component.component_kind(),
        CalendarComponent::Todo(component) => component.component_kind(),
        CalendarComponent::Venue(component) => component.component_kind(),
        CalendarComponent::Other(component) => component.component_kind(),
        _ => "unknown component".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        EMPTY_VCALENDAR_ICS, VEVENT_WINDOW_PRODUCTION_ICS, VTODO_ICS, VTODO_OPEN_PRODUCTION_ICS,
    };

    /// R17: one ordered pass, so document order survives across kinds.
    /// `events()` then `todos()` would yield event, event, todo.
    #[test]
    fn from_ics_str_preserves_document_order_across_kinds() {
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:first@caldir\r\nDTSTART:20260301T100000Z\r\nEND:VEVENT\r\n\
BEGIN:VTODO\r\nUID:second@caldir\r\nEND:VTODO\r\n\
BEGIN:VEVENT\r\nUID:third@caldir\r\nDTSTART:20260303T100000Z\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";

        let items: Vec<CalendarItem> = CalendarItem::from_ics_str(ics)
            .unwrap()
            .into_iter()
            .map(Result::unwrap)
            .collect();

        let order: Vec<(&str, &str)> = items
            .iter()
            .map(|item| match item {
                CalendarItem::Event(event) => ("VEVENT", event.uid.as_str()),
                CalendarItem::Todo(todo) => ("VTODO", todo.uid.as_str()),
            })
            .collect();

        assert_eq!(
            order,
            vec![
                ("VEVENT", "first@caldir"),
                ("VTODO", "second@caldir"),
                ("VEVENT", "third@caldir"),
            ]
        );
    }

    /// A per-component conversion failure stays an inner `Err` — it does not
    /// abort the pass, and it still counts towards arity.
    #[test]
    fn from_ics_str_surfaces_per_component_errors_without_aborting() {
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTODO\r\nUID:good@caldir\r\nEND:VTODO\r\n\
BEGIN:VTODO\r\nSUMMARY:No UID\r\nEND:VTODO\r\n\
END:VCALENDAR\r\n";

        let items = CalendarItem::from_ics_str(ics).unwrap();

        assert_eq!(items.len(), 2);
        assert!(matches!(items[0], Ok(CalendarItem::Todo(_))));
        assert!(matches!(items[1], Err(EventError::MissingUid)));
    }

    #[test]
    fn from_single_ics_str_reads_the_one_supported_component() {
        let item = CalendarItem::from_single_ics_str(VTODO_ICS).unwrap();

        assert!(
            matches!(item, CalendarItem::Todo(ref todo) if todo.uid.as_str() == "assignment-123@u-crawler")
        );
    }

    #[test]
    fn from_single_ics_str_counts_supported_components_of_either_kind() {
        let mixed = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:a@caldir\r\nDTSTART:20260301T100000Z\r\nEND:VEVENT\r\n\
BEGIN:VTODO\r\nUID:b@caldir\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

        assert!(matches!(
            CalendarItem::from_single_ics_str(mixed),
            Err(EventError::UnexpectedComponentCount {
                expected: 1,
                found: 2
            })
        ));

        assert!(matches!(
            CalendarItem::from_single_ics_str(EMPTY_VCALENDAR_ICS),
            Err(EventError::UnexpectedComponentCount {
                expected: 1,
                found: 0
            })
        ));
    }

    /// R18: a VTIMEZONE sibling is neither counted nor unsupported, so a
    /// zoned single item still passes the arity check.
    #[test]
    fn a_vtimezone_sibling_does_not_count_towards_arity() {
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTIMEZONE\r\nTZID:Europe/Stockholm\r\nEND:VTIMEZONE\r\n\
BEGIN:VTODO\r\nUID:zoned@caldir\r\nDUE;TZID=Europe/Stockholm:20260815T235900\r\n\
END:VTODO\r\nEND:VCALENDAR\r\n";

        let item = CalendarItem::from_single_ics_str(ics).unwrap();

        assert!(
            matches!(item, CalendarItem::Todo(ref todo) if todo.uid.as_str() == "zoned@caldir")
        );
    }

    /// R19 on the wire/base seam, "nothing supported" half: a document whose
    /// only components are unsupported must not yield an empty vector a caller
    /// would read as a delete. It aborts, naming what it found.
    ///
    /// **Changed by the sibling-tolerance fix.** Before, this document was
    /// `VEVENT` + `VFREEBUSY` and asserted the whole pass aborted — which is
    /// exactly the regression the fix removes (`main` loaded the event and
    /// ignored the sibling). The R19 property it was reaching for lives in the
    /// *zero-supported* case, so the fixture drops the `VEVENT` and the
    /// assertion is unchanged.
    #[test]
    fn an_unsupported_component_aborts_rather_than_shortening_the_vec() {
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VFREEBUSY\r\nUID:b@caldir\r\nEND:VFREEBUSY\r\nEND:VCALENDAR\r\n";

        let err = CalendarItem::from_ics_str(ics).unwrap_err();

        assert!(
            matches!(err, EventError::UnsupportedComponent(ref name) if name == "VFREEBUSY"),
            "got {err:?}"
        );
    }

    /// `VVENUE` reaches the pass as its own `CalendarComponent` variant, not
    /// as `Other` — a document holding only one must still be named, not
    /// silently empty.
    ///
    /// **Changed by the sibling-tolerance fix**, for the same reason as its
    /// neighbour: the fixture used to carry a `VEVENT` beside the `VVENUE`.
    #[test]
    fn a_vvenue_is_unsupported_and_named() {
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VVENUE\r\nUID:venue@caldir\r\nEND:VVENUE\r\n\
END:VCALENDAR\r\n";

        let err = CalendarItem::from_ics_str(ics).unwrap_err();

        assert!(
            matches!(err, EventError::UnsupportedComponent(ref name) if name == "VVENUE"),
            "got {err:?}"
        );
    }

    /// The regression this whole seam exists to prevent: on `main`,
    /// `icalendar.events()` ignored every non-`VEVENT` component, so a `VEVENT`
    /// beside a `VFREEBUSY`, a `VJOURNAL`, a stray top-level `VALARM` or an
    /// `X-` component loaded fine. All four are legal `icalbody` content per
    /// RFC 5545 §3.4. Erroring the document lost the event entirely — measured
    /// as `--json` visibility 1 -> 0 — and then suppressed every delete for
    /// that calendar (C7).
    #[test]
    fn one_supported_component_still_loads_beside_unsupported_siblings() {
        for sibling in [
            "BEGIN:VFREEBUSY\r\nUID:fb@caldir\r\nEND:VFREEBUSY\r\n",
            "BEGIN:VJOURNAL\r\nUID:j@caldir\r\nEND:VJOURNAL\r\n",
            "BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\n",
            "BEGIN:X-WR-CUSTOM\r\nX-THING:1\r\nEND:X-WR-CUSTOM\r\n",
        ] {
            let ics = format!(
                "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:a@caldir\r\nDTSTART:20260301T100000Z\r\nEND:VEVENT\r\n\
{sibling}END:VCALENDAR\r\n"
            );

            let item = CalendarItem::from_single_ics_str(&ics)
                .unwrap_or_else(|err| panic!("sibling {sibling:?} lost the event: {err:?}"));

            assert!(
                matches!(item, CalendarItem::Event(ref event) if event.uid.as_str() == "a@caldir"),
                "sibling {sibling:?} produced {item:?}"
            );
        }
    }

    /// The arity rules the fix must not weaken: two supported components are
    /// still too many, even when an unsupported sibling sits beside them.
    #[test]
    fn two_supported_components_are_still_an_error_beside_a_sibling() {
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:a@caldir\r\nDTSTART:20260301T100000Z\r\nEND:VEVENT\r\n\
BEGIN:VFREEBUSY\r\nUID:fb@caldir\r\nEND:VFREEBUSY\r\n\
BEGIN:VTODO\r\nUID:b@caldir\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";

        assert!(matches!(
            CalendarItem::from_single_ics_str(ics),
            Err(EventError::UnexpectedComponentCount {
                expected: 1,
                found: 2
            })
        ));
    }

    /// Zero supported components still errors, and still names every
    /// unsupported component it found — one message, in document order.
    #[test]
    fn zero_supported_components_names_everything_it_found() {
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VJOURNAL\r\nUID:j@caldir\r\nEND:VJOURNAL\r\n\
BEGIN:VFREEBUSY\r\nUID:fb@caldir\r\nEND:VFREEBUSY\r\nEND:VCALENDAR\r\n";

        let err = CalendarItem::from_ics_str(ics).unwrap_err();

        assert!(
            matches!(err, EventError::UnsupportedComponent(ref names) if names == "VJOURNAL, VFREEBUSY"),
            "got {err:?}"
        );
    }

    /// An ignored sibling is never silent (R19): the loader hands its name
    /// back so `doctor` can tell the user the file holds something caldir will
    /// drop the next time it rewrites it.
    #[test]
    fn an_ignored_sibling_is_reported_to_the_caller() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("siblings.ics");
        std::fs::write(
            &path,
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:a@caldir\r\nDTSTART:20260301T100000Z\r\nEND:VEVENT\r\n\
BEGIN:VFREEBUSY\r\nUID:fb@caldir\r\nEND:VFREEBUSY\r\nEND:VCALENDAR\r\n",
        )
        .unwrap();

        let (item, ignored) = CalendarItem::load_with_ignored(&path).unwrap();

        assert!(matches!(item, CalendarItem::Event(_)));
        assert_eq!(ignored, vec!["VFREEBUSY".to_string()]);
    }

    /// A `VTIMEZONE` is *ignored for counting* (R18) but is not a surprise —
    /// it must not be reported as something caldir is dropping, or every zoned
    /// file in the caldir would grow a warning.
    #[test]
    fn a_vtimezone_is_not_reported_as_an_ignored_sibling() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("zoned.ics");
        std::fs::write(
            &path,
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTIMEZONE\r\nTZID:Europe/Stockholm\r\nEND:VTIMEZONE\r\n\
BEGIN:VEVENT\r\nUID:a@caldir\r\nDTSTART:20260301T100000Z\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n",
        )
        .unwrap();

        let (_, ignored) = CalendarItem::load_with_ignored(&path).unwrap();

        assert!(ignored.is_empty(), "got {ignored:?}");
    }

    /// P1-05 / Correction 2. Production's deadlines/VTODO and windows/VEVENT
    /// for the same assignment do NOT share a UID — pinning this in code so a
    /// later ticket cannot read the pair as §8.5's same-UID case and build a
    /// mapping on a property production does not have.
    #[test]
    fn the_production_task_and_its_window_event_are_distinct_identities() {
        let task = CalendarItem::from_single_ics_str(VTODO_OPEN_PRODUCTION_ICS).unwrap();
        let window = CalendarItem::from_single_ics_str(VEVENT_WINDOW_PRODUCTION_ICS).unwrap();

        let task_id = task.instance_id();
        let window_id = window.instance_id();

        assert_ne!(task_id.uid(), window_id.uid());
        assert_ne!(task_id.kind(), window_id.kind());
    }
}
