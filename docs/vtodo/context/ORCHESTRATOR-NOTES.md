# Orchestrator cross-cutting notes (Stage 1)

Observations the lead drew across explorer docs. Each is a claim to be
CONFIRMED or REFUTED by synthesis against the underlying docs.

1. Blast radius: one unparseable .ics aborts the WHOLE calendar listing
   (`calendar.rs:114` `?`). A VTODO is a poison pill, not a skipped file.
   The u_crawler VEVENT `windows/` break too if a `deadlines/` VTODO exists
   in the same calendar dir. VERIFY whether windows/ and deadlines/ are
   separate calendars (separate dirs = separate calendars) — this changes
   the severity claim.
2. Two failure modes, not one:
   - local file  -> hard error ExpectedSingleEvent{found:0}
   - webcal feed -> SILENT DROP, zero items, no error
   Error model must cover both.
3. Single VEVENT filter in the workspace: `event.rs:101` `.events()`.
   One seam, not a scattered refactor.
4. Two parallel arity rules must move together:
   `CalendarEventError::ExpectedSingleEvent` (disk) and
   `EventError::UnexpectedEventCount` (RPC wire + sync bases).
5. Wire is opaque ICS already -> NO new RPC method needed.
   `Method` is a closed serde enum with no `#[serde(other)]`: new verbs are
   the ONE thing that cannot be added compatibly. Treat as hard constraint.
6. DECISION-11 (NEW, not in user's list): sync-window semantics for
   undated / DUE-only items. `occurs_in_range` = `start < to && end.unwrap_or(start) > from`
   is undefined without DTSTART. Failure mode: task outside window treated
   as a DELETE. Safe prior: undated items always in-window, never
   window-deleted. Must be tested, not assumed.
7. `Event` PartialEq is educe-tuned (ignores last_modified+sequence,
   order-independent x-props, attachments by uri). `==` MEANS "content
   changed?". Any Todo must replicate this exactly or sync misbehaves.
8. VALARM-in-VTODO is not byte-stable in icalendar 0.17.10 (fresh random
   UID each serialize). Combined with (7) structural equality => infinite
   sync churn. Strong argument to exclude VALARM from Todo MVP.
9. RELATED-TO collapses to last value => task hierarchies lossy =>
   subtasks are a non-goal.
10. iCloud has NO mapping layer; it delegates wholly to caldav ops.
    Fixing the 2 comp-filter literals likely lights up iCloud tasks free.
    Verify, don't assume.
11. Google/Outlook tasks need a DIFFERENT API + NEW OAuth scopes
    (outlook consent is Calendars.ReadWrite User.Read offline_access).
    => re-consent for every existing user => explicit non-goal.
12. CalDAV `find_map(Result::ok)` silently drops all but first parsed
    component (`caldav ops.rs:32-37`). Pre-existing bug, own ticket.
13. CLAUDE.md is STALE: says direction = mtime vs LAST-MODIFIED. Real
    algorithm is three-way against sync bases first
    (`calendar_diff.rs:153`), mtime only as fallback. Own ticket.
14. Constraints on test work: no tests/ dirs, no .ics fixture files, all
    inline #[cfg(test)]. test_utils.rs is crate-private (cli/providers
    cannot reuse). CI = TZ matrix UTC/Stockholm/New_York + -D warnings.
    Baseline 686 passed / 0 failed.
15. Toolchain hazard: pinned 1.97.0; PARALLEL cargo invocations race
    rustup and corrupt the toolchain. Serialize cargo across agents.
