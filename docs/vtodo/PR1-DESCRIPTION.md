# Degrade gracefully on unreadable `.ics` files (and don't delete what you can't read)

## Problem

One malformed `.ics` file anywhere in a calendar directory blanked the user's
entire agenda.

`Calendar::events()` loaded each `.ics` with a `?`, so the first file that
failed to parse aborted the whole listing and returned `Err`. The CLI's
cross-calendar aggregation propagated that error further still.

### Blast radius, three levels

1. **The file** — the malformed event is unreadable. Expected and fine.
2. **The calendar** — every *sibling* event in the same directory disappeared
   with it, because the listing returned `Err` instead of the events that
   parsed perfectly well.
3. **The agenda** — `caldir events`, `caldir today` and `caldir week` aggregate
   across calendars. One bad file in one calendar produced an empty agenda
   everywhere. The user's calendar looked *empty*, with no indication why.

The failure mode is the worst kind: silent, total, and maximally
disproportionate to the cause. A hand-authored file with a typo — the very
workflow caldir's plaintext model invites — took down the whole view.

## The fix

`Calendar::events()` now returns a `CalendarListing` instead of
`Result<Vec<CalendarEvent>>`-shaped all-or-nothing:

```rust
pub struct CalendarListing {
    events:  Vec<CalendarEvent>,   // the files that loaded
    skipped: Vec<SkippedEvent>,    // every file that did not, with its error
}
```

Per-file failures no longer abort the listing. Crucially, they are also **never
silently swallowed**: each skipped file is recorded with its path and its
error, and `caldir doctor` reports one warning per skipped file — the same
message the fatal path used to produce. Degrading softly is not the same as
degrading quietly.

## Why the delete-suppression guard is inseparable from this

Skipping a file changes what "absent locally" *means*, and caldir's sync
infers deletions from exactly that.

The sync diff decides a delete like this: item present remotely, absent from
the local listing, and present in `known_event_ids` (so it has synced before)
⇒ the user deleted it ⇒ emit a `Delete`.

Add graceful degradation on its own and that inference becomes catastrophic. A
file that merely failed to *parse* is now absent from the listing while still
being present in `known_event_ids`. The diff reads a local parse error as a
user's intent to delete — and pushes the delete to Google/iCloud/Outlook. The
old behaviour at least failed loudly and destroyed nothing; the "softer"
version would quietly delete the user's remote data.

So this PR ships the guard in the same change:

> **An incomplete listing suppresses ALL deletes for that calendar, in both
> directions.** No absence in a listing that is known to be short is evidence
> of a user deletion.

Both directions matters. The incoming direction (delete the local file because
the remote dropped it) is just as unsafe: the "local file" the diff wants to
reconcile may be sitting right there on disk, merely unparseable.

The guard is enforced by construction, not by convention:

- `CalendarDiff::compute_from_listing` takes the `CalendarListing` and is the
  **sole production entry point** for building a diff. You cannot construct a
  diff without supplying the completeness signal.
- The delete signal is a `Deletes { Allowed, Suppressed }` enum threaded to
  *both* delete sites in the diff, not a boolean checked in one place.
- `CalendarDiff::compute` (the "trust local absence" variant) is
  `#[cfg(test)]`-only, so the unguarded path cannot be reached from production
  code.

Creates, updates and out-of-window handling are unchanged — only deletes are
suppressed, and only for a calendar whose listing is known short.

## Tests

- `an_unreadable_file_is_skipped_and_reported_not_fatal` — a directory holding
  one good `.ics` and one garbage file lists the good event, reports the
  listing as incomplete, and names the skipped file's path. Previously this
  returned `Err` and yielded nothing.
- `unreadable_local_files_suppress_all_deletes` — with an unreadable file
  present, no `Delete` is emitted in either direction, while creates still
  flow.
- `complete_listing_still_emits_deletes_in_both_directions` — the opposite
  polarity: a complete listing deletes exactly as it did before. The guard
  narrows nothing when the listing is trustworthy.
- `diff_emits_no_deletes_when_a_local_file_is_unreadable` — the same property
  asserted at the sync seam (`Connection`), not just at the diff.
- `a_malformed_vevent_is_still_a_warning_after_component_skipping` — a file
  that *has* a VEVENT but whose component is broken is still audible in
  `doctor`. Guards against per-file degradation quietly downgrading to silence.
- `treats_unreadable_events_as_warnings` — the pre-existing doctor warning is
  preserved verbatim through the new path.

## Compatibility

- **Public API**: `Calendar::events()` returns `CalendarListing` rather than a
  plain `Vec<CalendarEvent>`. `CalendarListing` and `SkippedEvent` are exported
  from `caldir-core`. Existing call sites migrate with `.into_events()` (take
  the events, ignore completeness) or `.events()` (borrow). All in-tree callers
  are updated.
- **On-disk formats**: unchanged. No change to `.ics` files, `config.toml`, or
  `state/known_event_ids`.
- **Provider protocol**: unchanged. No provider needs modification.
- **Behavioural change users will notice**: a calendar containing an unreadable
  file now syncs its readable events (previously it failed outright), and
  suppresses deletes until the bad file is fixed or removed. `caldir doctor`
  names the offending file.
