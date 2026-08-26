use std::{collections::HashMap, path::Path};

use sha2::{Digest, Sha256};

use super::CalendarStateError;
use crate::{EventInstanceId, item::CalendarItem};

pub(crate) const EVENT_BASES_DIR_NAME: &str = "bases";

/// The last-synced form of each item, keyed by its kind-carrying identity.
///
/// The on-disk layout is unchanged (C5): one single-component `VCALENDAR` per
/// item at `bases/<sha256(identity_string)>.ics`. An event's identity string
/// is unprefixed, so an event's base filename is byte-identical to what every
/// existing caldir already wrote; a task's carries the `VTODO:` sigil and can
/// never land on an event's file even when they share a UID.
pub(crate) struct EventBases(HashMap<EventInstanceId, CalendarItem>);

impl IntoIterator for EventBases {
    type Item = (EventInstanceId, CalendarItem);
    type IntoIter = std::collections::hash_map::IntoIter<EventInstanceId, CalendarItem>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl EventBases {
    pub(crate) fn write_from<'a>(
        items: impl IntoIterator<Item = &'a CalendarItem>,
        path: &Path,
    ) -> Result<(), CalendarStateError> {
        std::fs::create_dir_all(path)?;

        for item in items {
            let filename = hash_filename(&item.instance_id().to_string());
            let item_path = path.join(format!("{filename}.ics"));

            // Skip unchanged bases to avoid churning every file on every sync.
            // Parse-compare, not byte-compare: to_ics_string stamps a fresh DTSTAMP.
            if CalendarItem::load(&item_path).is_ok_and(|existing| &existing == item) {
                continue;
            }

            let mut tmp = tempfile::NamedTempFile::new_in(path)?;
            std::io::Write::write_all(&mut tmp, item.to_ics_string().as_bytes())?;
            tmp.persist(item_path).map_err(|err| err.error)?;
        }

        Ok(())
    }

    pub(crate) fn load(path: &Path) -> Result<Self, CalendarStateError> {
        let mut event_bases = HashMap::new();

        if path.is_dir() {
            for entry in std::fs::read_dir(path)? {
                let entry = entry?;
                let file_path = entry.path();
                let Some(filename) = file_path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };

                if file_path.is_file() && is_hashed_filename(filename) {
                    // An unreadable base is no base — degrade to the legacy
                    // known-id entry rather than failing the whole load.
                    // Safe for the delete path: `known_event_ids`
                    // still holds the identity, so the item degrades to
                    // "seen, no base", never to "never seen".
                    let Ok(item) = CalendarItem::load(&file_path) else {
                        continue;
                    };
                    let instance_id = item.instance_id();
                    let expected_filename =
                        format!("{}.ics", hash_filename(&instance_id.to_string()));

                    if filename == expected_filename {
                        event_bases.insert(instance_id, item);
                    }
                }
            }
        }

        Ok(Self(event_bases))
    }
}

fn hash_filename(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn is_hashed_filename(filename: &str) -> bool {
    filename.strip_suffix(".ics").is_some_and(|stem| {
        stem.len() == 64
            && stem
                .bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
    })
}

#[cfg(test)]
mod tests {
    use super::{CalendarItem, EventBases, EventInstanceId, hash_filename, is_hashed_filename};
    use crate::test_utils::test_event;

    // Backward compatibility: an event id hashes the SAME string it always did,
    // so existing `bases/<sha256>.ics` files keep their names after
    // ComponentKind lands. The expected digests are independent literals
    // (`printf '<id>' | sha256sum`), not recomputed the way the code does it.
    #[test]
    fn event_base_filenames_are_unchanged_and_a_task_never_shares_one() {
        use crate::event::{EventUid, RecurrenceId};
        use chrono::NaiveDate;

        let uid = EventUid::new("abc123@hooli.com".to_string());
        let event = EventInstanceId::new(uid.clone(), None);
        assert_eq!(event.to_string(), "abc123@hooli.com");
        assert_eq!(
            hash_filename(&event.to_string()),
            "8518494aec55573e30ac20f14d44bda8ef783c0768d9f944822017c69d602a4d"
        );

        let recurring = EventInstanceId::new(
            uid.clone(),
            Some(RecurrenceId::from_event_time(crate::EventTime::Date(
                NaiveDate::from_ymd_opt(2026, 1, 9).unwrap(),
            ))),
        );
        assert_eq!(
            hash_filename(&recurring.to_string()),
            "6684dfc8c510581b06cc8630ef17ffaa063608e8974f67010c1471c74816ca81"
        );

        // Hazard 4: a task sharing the UID must not land on the event's file.
        let task = EventInstanceId::todo(uid, None);
        assert_eq!(
            hash_filename(&task.to_string()),
            "0b807d9f854656c80035519669e0e13b402a44c3a392c3f5d10faedd11d2eb6a"
        );
        assert_ne!(
            hash_filename(&task.to_string()),
            hash_filename(&event.to_string())
        );
    }

    #[test]
    fn filename_hash_is_fixed_length_and_filesystem_safe() {
        let filename = hash_filename("event@google.com__TZID=Europe/Stockholm:20260630T190000");

        assert_eq!(
            filename,
            "5dc4f5ac8fb72c87d9333e218f4b36d44f5fae5ad3cb888b788bbad2cbaf2148"
        );
        assert_eq!(filename.len(), 64);
    }

    #[test]
    fn write_skips_unchanged_bases_despite_dtstamp_drift() {
        let dir = tempfile::TempDir::new().unwrap();
        let event = CalendarItem::from(test_event());
        EventBases::write_from([&event], dir.path()).unwrap();

        let path = dir.path().join(format!(
            "{}.ics",
            hash_filename(&event.instance_id().to_string())
        ));

        // Simulate an earlier sync: same event, older DTSTAMP.
        let restamped: String = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| {
                if line.starts_with("DTSTAMP:") {
                    "DTSTAMP:20000101T000000Z\r\n".to_string()
                } else {
                    format!("{line}\r\n")
                }
            })
            .collect();
        std::fs::write(&path, &restamped).unwrap();

        EventBases::write_from([&event], dir.path()).unwrap();

        // Untouched: the old DTSTAMP survives.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), restamped);
    }

    #[test]
    fn load_ignores_non_hashed_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let event = test_event();
        std::fs::write(dir.path().join("legacy.ics"), event.to_ics_string()).unwrap();

        let loaded = EventBases::load(dir.path()).unwrap();

        assert_eq!(loaded.into_iter().count(), 0);
    }

    #[test]
    fn corrupt_base_is_skipped_rather_than_failing_the_load() {
        let dir = tempfile::TempDir::new().unwrap();
        let event = CalendarItem::from(test_event());
        EventBases::write_from([&event], dir.path()).unwrap();

        let garbled = format!("{}.ics", hash_filename("garbled"));
        let empty = format!("{}.ics", hash_filename("empty"));
        std::fs::write(dir.path().join(garbled), "not an ics file").unwrap();
        std::fs::write(dir.path().join(empty), "").unwrap();

        let loaded: Vec<_> = EventBases::load(dir.path()).unwrap().into_iter().collect();

        assert_eq!(loaded, vec![(event.instance_id(), event)]);
    }

    /// The rehash gate is a silent discarder by design: a base whose id
    /// string form has drifted from its filename degrades to "no base"
    /// rather than being trusted under the wrong key.
    #[test]
    fn load_discards_a_base_whose_filename_does_not_match_its_contents() {
        let dir = tempfile::TempDir::new().unwrap();

        // A perfectly valid event document, filed under another id's digest.
        std::fs::write(
            dir.path()
                .join("99af2871eebb2d3a36d4269410f91edc56db3d531c12f46800bf0b6d7e59c9ba.ics"),
            crate::test_utils::VEVENT_CONTROL_ICS,
        )
        .unwrap();

        let loaded: Vec<_> = EventBases::load(dir.path()).unwrap().into_iter().collect();

        assert_eq!(loaded, vec![]);
    }

    /// Backward compatibility, end to end: a base file written by an older
    /// caldir — an event document under `sha256(<uid>)` with no kind sigil —
    /// still loads. If this ever fails, every existing user's sync state
    /// silently reads as empty and the next sync becomes a full re-push.
    #[test]
    fn a_legacy_event_base_written_before_vtodo_support_still_loads() {
        let dir = tempfile::TempDir::new().unwrap();

        // `printf 'window-123@u-crawler' | sha256sum` — an independent digest,
        // written the way pre-VTODO caldir would have named the file.
        std::fs::write(
            dir.path()
                .join("4c6a2c75669698a6e1127c24dadde2bb9e48ffff12b55e5797cf5e78b31e3b54.ics"),
            crate::test_utils::VEVENT_CONTROL_ICS,
        )
        .unwrap();

        let loaded: Vec<_> = EventBases::load(dir.path()).unwrap().into_iter().collect();

        assert_eq!(loaded.len(), 1, "{loaded:?}");
        assert_eq!(
            loaded[0].0,
            EventInstanceId::new(
                crate::event::EventUid::new("window-123@u-crawler".to_string()),
                None
            )
        );
        assert!(matches!(loaded[0].1, CalendarItem::Event(_)));
    }

    /// T-23. Three things that can independently break, in one test:
    /// the task base round-trips (R43), the rehash check agrees with the
    /// kind-carrying id string form (the L1-06 + L3-01 contract), and an
    /// unparseable base is skipped rather than fatal (C5, INVIOLABLE).
    #[test]
    fn a_task_base_loads_and_rehashes_and_an_unparseable_base_is_skipped() {
        use crate::event::ComponentKind;
        use crate::test_utils::VTODO_ICS;

        let dir = tempfile::TempDir::new().unwrap();
        let task = CalendarItem::from_single_ics_str(VTODO_ICS).unwrap();

        EventBases::write_from([&task], dir.path()).unwrap();

        // Rehash: the on-disk name is the hash of the task's own id string.
        // The digest is an independent literal
        // (`printf 'VTODO:assignment-123@u-crawler' | sha256sum`), not a
        // recomputation the way the code does it.
        let expected_filename =
            "00f37d0ec1204a1be30bbc8cec82b000fc6e8b6405f1dd1dfda5c31367683e83.ics";
        assert!(dir.path().join(expected_filename).is_file());
        assert_eq!(
            format!("{}.ics", hash_filename(&task.instance_id().to_string())),
            expected_filename
        );

        // C5: unparseable bases under correctly-named hashed filenames.
        std::fs::write(
            dir.path().join(format!("{}.ics", hash_filename("garbled"))),
            "not an ics file",
        )
        .unwrap();
        std::fs::write(
            dir.path().join(format!("{}.ics", hash_filename("empty"))),
            "",
        )
        .unwrap();

        let loaded: Vec<_> = EventBases::load(dir.path()).unwrap().into_iter().collect();

        assert_eq!(loaded, vec![(task.instance_id(), task)]);
        assert_eq!(loaded[0].0.kind(), ComponentKind::Todo);
    }

    /// AC2: two kinds, one UID, two files. The store never conflates them,
    /// and the event keeps the filename it has always had.
    #[test]
    fn an_event_and_a_task_sharing_a_uid_get_two_distinct_base_files() {
        let dir = tempfile::TempDir::new().unwrap();

        let mut event = test_event();
        event.uid = crate::event::EventUid::new("shared@u-crawler".to_string());
        let mut todo = crate::todo::Todo::new("Shared");
        todo.uid = crate::event::EventUid::new("shared@u-crawler".to_string());

        let event = CalendarItem::from(event);
        let task = CalendarItem::from(todo);

        EventBases::write_from([&event, &task], dir.path()).unwrap();

        let files: std::collections::BTreeSet<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_str().unwrap().to_string())
            .collect();

        // Independent digests: `printf '<id>' | sha256sum`.
        assert_eq!(files.len(), 2, "{files:?}");
        assert!(
            files.contains("99af2871eebb2d3a36d4269410f91edc56db3d531c12f46800bf0b6d7e59c9ba.ics")
        );
        assert!(
            files.contains("e49f6d9c1cb361b8f11d241574edf8b6e097d19b08d488b2bfe4f66276acf6c7.ics")
        );

        let loaded: std::collections::HashMap<_, _> =
            EventBases::load(dir.path()).unwrap().into_iter().collect();

        assert_eq!(loaded.get(&event.instance_id()), Some(&event));
        assert_eq!(loaded.get(&task.instance_id()), Some(&task));
    }

    /// R18: the base decoder applies the `VTIMEZONE` exemption, exactly like
    /// the disk loader — a zoned task base is not "two components".
    #[test]
    fn a_base_with_a_vtimezone_sibling_decodes() {
        let dir = tempfile::TempDir::new().unwrap();
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VTIMEZONE\r\nTZID:Europe/Stockholm\r\nEND:VTIMEZONE\r\n\
BEGIN:VTODO\r\nUID:zoned@u-crawler\r\nDUE;TZID=Europe/Stockholm:20260815T235900\r\n\
END:VTODO\r\nEND:VCALENDAR\r\n";

        // Independent digest: `printf 'VTODO:zoned@u-crawler' | sha256sum`.
        std::fs::write(
            dir.path()
                .join("8be82ff09dc387c86b83580644536fa6814dd4ac7e975f9f6a9b703d598cf36e.ics"),
            ics,
        )
        .unwrap();

        let loaded: Vec<_> = EventBases::load(dir.path()).unwrap().into_iter().collect();

        assert_eq!(loaded.len(), 1, "{loaded:?}");
        assert_eq!(
            loaded[0].0,
            EventInstanceId::todo(
                crate::event::EventUid::new("zoned@u-crawler".to_string()),
                None
            )
        );
    }

    /// AC4: an arity failure and an unsupported component are errors the
    /// decoder raises and `load` swallows — never propagated, never fatal.
    #[test]
    fn an_arity_or_unsupported_base_is_skipped_not_propagated() {
        let dir = tempfile::TempDir::new().unwrap();
        let event = CalendarItem::from(test_event());
        EventBases::write_from([&event], dir.path()).unwrap();

        let two_components = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:a@caldir\r\nDTSTART:20260301T100000Z\r\nEND:VEVENT\r\n\
BEGIN:VTODO\r\nUID:b@caldir\r\nEND:VTODO\r\nEND:VCALENDAR\r\n";
        let unsupported = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VFREEBUSY\r\nUID:c@caldir\r\nEND:VFREEBUSY\r\nEND:VCALENDAR\r\n";

        std::fs::write(
            dir.path().join(format!("{}.ics", hash_filename("two"))),
            two_components,
        )
        .unwrap();
        std::fs::write(
            dir.path()
                .join(format!("{}.ics", hash_filename("unsupported"))),
            unsupported,
        )
        .unwrap();

        let loaded: Vec<_> = EventBases::load(dir.path()).unwrap().into_iter().collect();

        assert_eq!(loaded, vec![(event.instance_id(), event)]);
    }

    /// AC7: the parse-compare skip works for tasks too — a task base is not
    /// rewritten just because `to_ics_string` stamps a fresh `DTSTAMP`.
    #[test]
    fn write_skips_unchanged_task_bases_despite_dtstamp_drift() {
        let dir = tempfile::TempDir::new().unwrap();
        let task = CalendarItem::from_single_ics_str(crate::test_utils::VTODO_ICS).unwrap();
        EventBases::write_from([&task], dir.path()).unwrap();

        let path = dir.path().join(format!(
            "{}.ics",
            hash_filename(&task.instance_id().to_string())
        ));

        let restamped: String = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| {
                if line.starts_with("DTSTAMP:") {
                    "DTSTAMP:20000101T000000Z\r\n".to_string()
                } else {
                    format!("{line}\r\n")
                }
            })
            .collect();
        std::fs::write(&path, &restamped).unwrap();

        EventBases::write_from([&task], dir.path()).unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), restamped);
    }

    #[test]
    fn recognizes_only_lowercase_sha256_filenames() {
        assert!(is_hashed_filename(
            "5dc4f5ac8fb72c87d9333e218f4b36d44f5fae5ad3cb888b788bbad2cbaf2148.ics"
        ));
        assert!(!is_hashed_filename("legacy.ics"));
        assert!(!is_hashed_filename(
            "5DC4F5AC8FB72C87D9333E218F4B36D44F5FAE5AD3CB888B788BBAD2CBAF2148.ics"
        ));
    }
}
