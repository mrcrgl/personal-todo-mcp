//! Optimistic concurrency for stored documents, shared by files and tables.

/// What a write expects to find, so a stale write fails instead of
/// overwriting someone else's change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expected {
    Absent,
    Version(String),
}
