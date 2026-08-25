//! Live-server tier: gates that need a real CalDAV server, not a double.
//!
//! `#[cfg(test)]` only. See [`radicale`] for the container recipe, the gating
//! rules and the two findings this tier records (Q-3 and the R69 iCloud gap).

pub mod radicale;
