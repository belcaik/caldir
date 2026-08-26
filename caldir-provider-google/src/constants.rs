pub const PROVIDER_NAME: &str = "google";
pub const PROVIDER_EVENT_ID_PROPERTY: &str = "X-GOOGLE-EVENT-ID";
pub const PROVIDER_COLOR_ID_PROPERTY: &str = "X-GOOGLE-COLOR-ID";

pub const CALENDAR_LIST_SCOPE: &str =
    "https://www.googleapis.com/auth/calendar.calendarlist.readonly";
pub const CALENDAR_EVENTS_SCOPE: &str = "https://www.googleapis.com/auth/calendar.events";
/// Full read/write Tasks scope (spec §10: "read/write requires the Google
/// Tasks OAuth scope" — one scope covers both; `tasks.readonly` would force
/// a second reauthorization cycle once writes ship in Wave 5).
pub const TASKS_SCOPE: &str = "https://www.googleapis.com/auth/tasks";

pub const REQUESTED_SCOPES: &[&str] = &[CALENDAR_LIST_SCOPE, CALENDAR_EVENTS_SCOPE, TASKS_SCOPE];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requested_scopes_include_both_calendar_scopes_and_tasks() {
        assert!(REQUESTED_SCOPES.contains(&CALENDAR_LIST_SCOPE));
        assert!(REQUESTED_SCOPES.contains(&CALENDAR_EVENTS_SCOPE));
        assert!(REQUESTED_SCOPES.contains(&TASKS_SCOPE));
    }
}
