use caldir_core::{Caldir, CaldirError, Connection, EventChange};

/// The remote provider's slug, for a message that names it. `None` for a
/// local-only calendar — there is no provider to name.
pub fn provider_slug(connection: &Connection) -> Option<String> {
    connection
        .local()
        .config()
        .and_then(|c| c.remote_config())
        .map(|r| r.provider_slug().to_string())
}

/// Return the caldir's connections, optionally narrowed to specific calendar slugs.
/// An empty `calendar_slugs` slice returns all connections.
pub fn connections(
    caldir: &Caldir,
    calendar_slugs: &[String],
) -> Vec<Result<Connection, CaldirError>> {
    let all = caldir.connections();

    if calendar_slugs.is_empty() {
        return all;
    }

    all.into_iter()
        .filter(|conn| {
            conn.as_ref()
                .ok()
                .and_then(|c| c.local().slug())
                .map(|s| calendar_slugs.iter().any(|x| x == s))
                .unwrap_or(false)
        })
        .collect()
}

/// Count `(created, updated, deleted)` over a sequence of event changes.
pub fn count_changes<'a, I>(changes: I) -> (usize, usize, usize)
where
    I: IntoIterator<Item = &'a EventChange>,
{
    changes
        .into_iter()
        .fold((0, 0, 0), |(c, u, d), change| match change {
            EventChange::Create(_) => (c + 1, u, d),
            EventChange::Update { .. } => (c, u + 1, d),
            EventChange::Delete(_) => (c, u, d + 1),
        })
}
