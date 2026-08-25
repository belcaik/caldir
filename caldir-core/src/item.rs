use crate::calendar::CalendarEventError;
use crate::event::{Event, EventError, EventInstanceId, EventUid};
use crate::todo::Todo;
use chrono::{DateTime, Utc};
use icalendar::{CalendarComponent, Component};
use std::path::PathBuf;

/// One supported calendar component parsed from an ICS document.
///
/// An item's kind comes solely from the component it was parsed from and is
/// preserved across every boundary (R7). Adding `VJOURNAL` later is one more
/// variant, one more struct and one more match arm (R93).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalendarItem {
    Event(Event),
    Todo(Todo),
}

/// Components that are neither supported nor an error: ignored for counting
/// and never reported as unsupported (R18).
const IGNORED_COMPONENTS: [&str; 1] = ["VTIMEZONE"];

impl CalendarItem {
    /// Parse an ICS document into its supported items, in document order.
    ///
    /// One ordered pass over `icalendar::Calendar::components` — never
    /// `events()` then `todos()`, which would lose document order (R17).
    /// An unsupported component aborts the pass with an error naming it; it is
    /// never dropped, because a silent drop feeds the delete path (R19).
    pub fn from_ics_str(contents: &str) -> Result<Vec<Result<Self, EventError>>, EventError> {
        Ok(Self::parse_document(contents)?.items)
    }

    /// Parse an ICS document holding exactly one supported component.
    ///
    /// The path-less arity check, for RPC payloads and sync bases. The "none"
    /// case is not given its own variant here: bases are always written by
    /// caldir, and the wire case is covered by R19 (R76).
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

    /// Load the single supported item from an `.ics` file on disk (R32).
    ///
    /// The path-flavoured arity check, which distinguishes "no supported
    /// component" from "too many" so the message can tell a VTODO-only file
    /// from an empty `VCALENDAR` (R75).
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, CalendarEventError> {
        let path = path.into();

        if !path.is_file() {
            return Err(CalendarEventError::NotFound(path));
        }

        let contents = std::fs::read_to_string(&path)?;

        let parsed = Self::parse_document(&contents)
            .map_err(|err| CalendarEventError::InvalidEvent(path.clone(), err))?;

        let found_description = parsed.found_description();

        match <[Result<Self, EventError>; 1]>::try_from(parsed.items) {
            Ok([result]) => result.map_err(|err| CalendarEventError::InvalidEvent(path, err)),
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
    /// the key, so a VEVENT and a VTODO sharing a UID never collide (R3).
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
    /// The narrowing the VEVENT-only sync seams still need. It is not a drop:
    /// every caller looks the item up by an *event* identity, and a stored
    /// base always shares its key's kind, so `None` here is unreachable rather
    /// than lossy — and where it is reached anyway it means "no base", which
    /// costs a three-way compare, never a delete (R19).
    pub(crate) fn as_event(&self) -> Option<&Event> {
        match self {
            CalendarItem::Event(event) => Some(event),
            CalendarItem::Todo(_) => None,
        }
    }

    /// The item's `SUMMARY`, from whichever kind backs it.
    pub fn summary(&self) -> Option<&str> {
        match self {
            CalendarItem::Event(event) => event.summary.as_deref(),
            CalendarItem::Todo(todo) => todo.summary.as_deref(),
        }
    }

    /// The filename stem for this item, from the kind's own slug rule (R33,
    /// R34). Never a shared rule: an event's stem must stay byte-identical
    /// (C1) while a task takes the reserved `_task__` prefix.
    pub(crate) fn base_slug(&self) -> String {
        match self {
            CalendarItem::Event(event) => event.base_slug(),
            CalendarItem::Todo(todo) => todo.base_slug(),
        }
    }

    /// Serialize to a single-component `VCALENDAR` document (R27, R32), from
    /// the kind's own serializer.
    pub(crate) fn to_ics_string(&self) -> String {
        match self {
            CalendarItem::Event(event) => event.to_ics_string(),
            CalendarItem::Todo(todo) => todo.to_ics_string(),
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
                // `CalendarComponent` is `#[non_exhaustive]`, so this arm is
                // required. It errors and names what it found — there is no
                // skip arm here, ever: a silent drop feeds the delete path
                // (R19, INVIOLABLE).
                _ => return Err(EventError::UnsupportedComponent(name)),
            }
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
}

impl ParsedDocument {
    /// The `{names}` half of R75's "none" message: the components present, or
    /// the literal `nothing` for a component-less `VCALENDAR`.
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
    use crate::test_utils::{EMPTY_VCALENDAR_ICS, VTODO_ICS};

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

    /// R19 on the wire/base seam: an unsupported component aborts the pass
    /// rather than yielding a short vector a caller would read as a delete.
    #[test]
    fn an_unsupported_component_aborts_rather_than_shortening_the_vec() {
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:a@caldir\r\nDTSTART:20260301T100000Z\r\nEND:VEVENT\r\n\
BEGIN:VFREEBUSY\r\nUID:b@caldir\r\nEND:VFREEBUSY\r\nEND:VCALENDAR\r\n";

        let err = CalendarItem::from_ics_str(ics).unwrap_err();

        assert!(
            matches!(err, EventError::UnsupportedComponent(ref name) if name == "VFREEBUSY"),
            "got {err:?}"
        );
    }

    /// `VVENUE` reaches the pass as its own `CalendarComponent` variant, not
    /// as `Other` — it must still be named, not skipped.
    #[test]
    fn a_vvenue_is_unsupported_and_named() {
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VVENUE\r\nUID:venue@caldir\r\nEND:VVENUE\r\n\
BEGIN:VEVENT\r\nUID:a@caldir\r\nDTSTART:20260301T100000Z\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";

        let err = CalendarItem::from_ics_str(ics).unwrap_err();

        assert!(
            matches!(err, EventError::UnsupportedComponent(ref name) if name == "VVENUE"),
            "got {err:?}"
        );
    }
}
