use std::fmt;

/// VTODO `STATUS`. Distinct from the event `Status`: RFC 5545 §3.8.1.11 defines
/// a different value set for tasks, and merging the two would put impossible
/// variants in front of every event match site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    // NEEDS-ACTION is the RFC 5545 default for a task, so absence and an
    // explicit NEEDS-ACTION must round-trip the same way.
    #[default]
    NeedsAction,
    InProcess,
    Completed,
    Cancelled,
}

impl fmt::Display for TodoStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::NeedsAction => "needs_action",
            Self::InProcess => "in_process",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        };
        write!(f, "{}", s)
    }
}

impl TodoStatus {
    pub fn as_ics_str(&self) -> &'static str {
        match self {
            Self::NeedsAction => "NEEDS-ACTION",
            Self::InProcess => "IN-PROCESS",
            Self::Completed => "COMPLETED",
            Self::Cancelled => "CANCELLED",
        }
    }

    pub fn from_ics_str(s: &str) -> Option<Self> {
        match s {
            "NEEDS-ACTION" => Some(Self::NeedsAction),
            "IN-PROCESS" => Some(Self::InProcess),
            "COMPLETED" => Some(Self::Completed),
            "CANCELLED" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_four_task_statuses_round_trip() {
        for (ics, status) in [
            ("NEEDS-ACTION", TodoStatus::NeedsAction),
            ("IN-PROCESS", TodoStatus::InProcess),
            ("COMPLETED", TodoStatus::Completed),
            ("CANCELLED", TodoStatus::Cancelled),
        ] {
            assert_eq!(TodoStatus::from_ics_str(ics), Some(status));
            assert_eq!(status.as_ics_str(), ics);
        }

        assert_eq!(TodoStatus::default(), TodoStatus::NeedsAction);
        assert_eq!(TodoStatus::default().as_ics_str(), "NEEDS-ACTION");
    }
}
