//! Main-viewport coordination for the relation-network feature.
//!
//! Bridge commands and deferred-viewport actions are implemented on `IcaApp`
//! in the view module for now; this action type is the stable boundary used by
//! the child viewport to describe state transitions without owning the app.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RelationAction {
    Rebuild,
    LoadGroups(Option<usize>),
    Close,
}
