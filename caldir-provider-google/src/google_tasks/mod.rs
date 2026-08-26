//! Hand-rolled Google Tasks REST client (P4-01 spike decision: reqwest, no
//! new crate). Every Tasks-specific type stays private to this module —
//! nothing here may cross into caldir-core or the shared RPC (P4-01
//! Containment).

mod client;

pub use client::list_task_lists;
