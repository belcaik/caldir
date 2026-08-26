pub const PROVIDER_NAME: &str = "google";
pub const PROVIDER_EVENT_ID_PROPERTY: &str = "X-GOOGLE-EVENT-ID";
pub const PROVIDER_COLOR_ID_PROPERTY: &str = "X-GOOGLE-COLOR-ID";
/// Mirrors `PROVIDER_EVENT_ID_PROPERTY` for the Tasks path: the Google task
/// id is provider metadata, never the VTODO `UID` (spec §12).
pub const PROVIDER_TASK_ID_PROPERTY: &str = "X-GOOGLE-TASK-ID";

pub const CALENDAR_LIST_SCOPE: &str =
    "https://www.googleapis.com/auth/calendar.calendarlist.readonly";
pub const CALENDAR_EVENTS_SCOPE: &str = "https://www.googleapis.com/auth/calendar.events";
/// Authorizes `calendars.insert` (secondary calendar creation) without the
/// full `calendar` scope.
pub const CALENDAR_CALENDARS_SCOPE: &str = "https://www.googleapis.com/auth/calendar.calendars";
/// Full read/write Tasks scope (spec §10: "read/write requires the Google
/// Tasks OAuth scope" — one scope covers both; `tasks.readonly` would force
/// a second reauthorization cycle once writes ship in Wave 5).
pub const TASKS_SCOPE: &str = "https://www.googleapis.com/auth/tasks";

pub const REQUESTED_SCOPES: &[&str] = &[
    CALENDAR_LIST_SCOPE,
    CALENDAR_EVENTS_SCOPE,
    CALENDAR_CALENDARS_SCOPE,
    TASKS_SCOPE,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requested_scopes_are_exactly_the_calendar_and_tasks_scopes() {
        assert_eq!(
            REQUESTED_SCOPES,
            &[
                CALENDAR_LIST_SCOPE,
                CALENDAR_EVENTS_SCOPE,
                CALENDAR_CALENDARS_SCOPE,
                TASKS_SCOPE
            ]
        );
    }
}
