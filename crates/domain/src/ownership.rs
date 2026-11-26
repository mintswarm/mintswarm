//! Who owns a work item, and how ownership changes hands.
//!
//! A work item gets its owner when it is created, ingested from the tracker or inherited from
//! a parent; a person may claim an unowned item, its owner may hand it over, another person
//! may take it over, and it becomes unowned when its owner's account is removed. Every change
//! is an [`OwnershipEvent`] naming the previous owner, the new owner, the actor, the time and
//! the client.
//!
//! Ownership says whom an item waits on; it gives no right to approve anything, and this
//! module offers no approval check.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::{UserId, WorkItemId};

/// Who asked for a change of owner.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    /// A person.
    User(UserId),
    /// The control plane itself, for changes no person sends: ingest, inheritance by a split
    /// or release on account removal.
    Plane,
}

/// Where the change was asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Client {
    /// The web UI.
    Ui,
    /// The command line.
    Cli,
    /// A coding-agent session.
    Session,
    /// The tracker.
    Tracker,
}

/// A requested change of owner.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnershipRequest {
    /// A new item, owned by its creator.
    Create {
        /// The creator.
        owner: UserId,
    },
    /// An item read from the tracker, owned by its author.
    Ingest {
        /// The author.
        author: UserId,
    },
    /// An item made from a parent, such as by a split, owned as the parent is.
    Inherit {
        /// The parent's owner.
        parent_owner: UserId,
    },
    /// The actor takes an unowned item.
    Claim,
    /// The owner gives the item to another person.
    HandOver {
        /// The new owner.
        to: UserId,
    },
    /// The actor takes an item someone else owns.
    TakeOver,
    /// The owner's account is removed, and the item becomes unowned.
    Release {
        /// The removed account.
        removed: UserId,
    },
}

/// The kind of an ownership change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnershipEventKind {
    /// After [`OwnershipRequest::Create`].
    Created,
    /// After [`OwnershipRequest::Ingest`].
    Ingested,
    /// After [`OwnershipRequest::Inherit`].
    Inherited,
    /// After [`OwnershipRequest::Claim`].
    Claimed,
    /// After [`OwnershipRequest::HandOver`].
    HandedOver,
    /// After [`OwnershipRequest::TakeOver`].
    TakenOver,
    /// After [`OwnershipRequest::Release`].
    Released,
}

/// A change of a work item's owner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnershipEvent {
    /// The work item.
    pub item: WorkItemId,
    /// What happened.
    pub kind: OwnershipEventKind,
    /// The owner before, if any.
    pub previous: Option<UserId>,
    /// The owner after, if any.
    pub new: Option<UserId>,
    /// Who asked for it.
    pub actor: Actor,
    /// When, in RFC 3339, as the caller gives it.
    pub at: String,
    /// Where it was asked for.
    pub client: Client,
}

/// Why a change of owner was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    /// Create, ingest, inherit or claim on an item that already has an owner.
    AlreadyOwned,
    /// Hand-over, take-over or release on an item with no owner.
    NotOwned,
    /// Hand-over by someone who is not the owner.
    NotOwner,
    /// Hand-over to the current owner, or take-over by the owner.
    SameOwner,
    /// Release for an account that does not own the item.
    RemovedIsNotOwner,
    /// Claim, hand-over or take-over by the control plane rather than a person.
    ActorNotUser,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::AlreadyOwned => "the item already has an owner",
            Self::NotOwned => "the item has no owner",
            Self::NotOwner => "only the owner can hand the item over",
            Self::SameOwner => "the item already belongs to that person",
            Self::RemovedIsNotOwner => "the removed account does not own the item",
            Self::ActorNotUser => "only a person can claim, hand over or take over an item",
        })
    }
}

impl std::error::Error for Refusal {}

/// Applies REQUEST by ACTOR from CLIENT at AT to ITEM, whose owner is CURRENT, and returns the
/// event that records it.
pub fn transition(
    current: Option<&UserId>,
    item: WorkItemId,
    request: OwnershipRequest,
    actor: Actor,
    client: Client,
    at: String,
) -> Result<OwnershipEvent, Refusal> {
    use OwnershipEventKind as Kind;
    let person = match &actor {
        Actor::User(id) => Some(id),
        Actor::Plane => None,
    };
    let (kind, new) = match request {
        OwnershipRequest::Create { owner } => (Kind::Created, unowned(current, owner)?),
        OwnershipRequest::Ingest { author } => (Kind::Ingested, unowned(current, author)?),
        OwnershipRequest::Inherit { parent_owner } => {
            (Kind::Inherited, unowned(current, parent_owner)?)
        }
        OwnershipRequest::Claim => {
            let by = person.ok_or(Refusal::ActorNotUser)?;
            (Kind::Claimed, unowned(current, by.clone())?)
        }
        OwnershipRequest::HandOver { to } => {
            let by = person.ok_or(Refusal::ActorNotUser)?;
            let owner = current.ok_or(Refusal::NotOwned)?;
            if by != owner {
                return Err(Refusal::NotOwner);
            }
            if &to == owner {
                return Err(Refusal::SameOwner);
            }
            (Kind::HandedOver, Some(to))
        }
        OwnershipRequest::TakeOver => {
            let by = person.ok_or(Refusal::ActorNotUser)?;
            let owner = current.ok_or(Refusal::NotOwned)?;
            if by == owner {
                return Err(Refusal::SameOwner);
            }
            (Kind::TakenOver, Some(by.clone()))
        }
        OwnershipRequest::Release { removed } => {
            let owner = current.ok_or(Refusal::NotOwned)?;
            if &removed != owner {
                return Err(Refusal::RemovedIsNotOwner);
            }
            (Kind::Released, None)
        }
    };
    Ok(OwnershipEvent {
        item,
        kind,
        previous: current.cloned(),
        new,
        actor,
        at,
        client,
    })
}

/// NEW as the owner of an item that has none.
fn unowned(current: Option<&UserId>, new: UserId) -> Result<Option<UserId>, Refusal> {
    match current {
        Some(_) => Err(Refusal::AlreadyOwned),
        None => Ok(Some(new)),
    }
}
