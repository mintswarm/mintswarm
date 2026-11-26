//! Ownership transitions of work items and the events they record.

#![cfg(test)]

use mintswarm_domain::ids::{UserId, WorkItemId};
use mintswarm_domain::ownership::{
    Actor, Client, OwnershipEvent, OwnershipEventKind as Kind, OwnershipRequest as Request,
    Refusal, transition,
};

const AT: &str = "2026-11-26T10:00:00Z";

fn user(id: &str) -> UserId {
    UserId(id.to_owned())
}

fn by(id: &str) -> Actor {
    Actor::User(user(id))
}

fn apply(current: Option<&str>, request: Request, actor: Actor) -> Result<OwnershipEvent, Refusal> {
    let current = current.map(user);
    transition(
        current.as_ref(),
        WorkItemId("item-1".to_owned()),
        request,
        actor,
        Client::Ui,
        AT.to_owned(),
    )
}

fn event(kind: Kind, previous: Option<&str>, new: Option<&str>, actor: Actor) -> OwnershipEvent {
    OwnershipEvent {
        item: WorkItemId("item-1".to_owned()),
        kind,
        previous: previous.map(user),
        new: new.map(user),
        actor,
        at: AT.to_owned(),
        client: Client::Ui,
    }
}

#[test]
fn claiming_an_unowned_item_makes_the_actor_its_owner() {
    assert_eq!(
        apply(None, Request::Claim, by("ann")),
        Ok(event(Kind::Claimed, None, Some("ann"), by("ann")))
    );
}

#[test]
fn the_owner_hands_the_item_over() {
    let request = Request::HandOver { to: user("bob") };
    assert_eq!(
        apply(Some("ann"), request, by("ann")),
        Ok(event(Kind::HandedOver, Some("ann"), Some("bob"), by("ann")))
    );
}

#[test]
fn another_person_takes_the_item_over() {
    assert_eq!(
        apply(Some("ann"), Request::TakeOver, by("bob")),
        Ok(event(Kind::TakenOver, Some("ann"), Some("bob"), by("bob")))
    );
}

#[test]
fn removing_the_owner_releases_the_item() {
    let request = Request::Release {
        removed: user("ann"),
    };
    assert_eq!(
        apply(Some("ann"), request, Actor::Plane),
        Ok(event(Kind::Released, Some("ann"), None, Actor::Plane))
    );
}

#[test]
fn the_client_is_recorded() {
    let event = transition(
        None,
        WorkItemId("item-2".to_owned()),
        Request::Claim,
        by("ann"),
        Client::Session,
        AT.to_owned(),
    )
    .unwrap();
    assert_eq!(event.client, Client::Session);
    assert_eq!(event.item, WorkItemId("item-2".to_owned()));
}

#[test]
fn creation_ingest_and_inheritance_have_no_previous_owner() {
    let cases = [
        (
            Request::Create { owner: user("ann") },
            Kind::Created,
            by("ann"),
        ),
        (
            Request::Ingest {
                author: user("ann"),
            },
            Kind::Ingested,
            Actor::Plane,
        ),
        (
            Request::Inherit {
                parent_owner: user("ann"),
            },
            Kind::Inherited,
            Actor::Plane,
        ),
    ];
    for (request, kind, actor) in cases {
        assert_eq!(
            apply(None, request, actor.clone()),
            Ok(event(kind, None, Some("ann"), actor))
        );
    }
}

#[test]
fn an_owned_item_cannot_be_given_a_first_owner_or_claimed() {
    for request in [
        Request::Create { owner: user("bob") },
        Request::Ingest {
            author: user("bob"),
        },
        Request::Inherit {
            parent_owner: user("bob"),
        },
        Request::Claim,
    ] {
        assert_eq!(
            apply(Some("ann"), request, by("bob")),
            Err(Refusal::AlreadyOwned)
        );
    }
}

#[test]
fn an_unowned_item_cannot_be_handed_over_taken_over_or_released() {
    for request in [
        Request::HandOver { to: user("bob") },
        Request::TakeOver,
        Request::Release {
            removed: user("ann"),
        },
    ] {
        assert_eq!(apply(None, request, by("ann")), Err(Refusal::NotOwned));
    }
}

#[test]
fn only_the_owner_hands_over() {
    let request = Request::HandOver { to: user("cid") };
    assert_eq!(
        apply(Some("ann"), request, by("bob")),
        Err(Refusal::NotOwner)
    );
}

#[test]
fn handing_over_to_the_owner_or_taking_over_ones_own_item_is_refused() {
    let request = Request::HandOver { to: user("ann") };
    assert_eq!(
        apply(Some("ann"), request, by("ann")),
        Err(Refusal::SameOwner)
    );
    assert_eq!(
        apply(Some("ann"), Request::TakeOver, by("ann")),
        Err(Refusal::SameOwner)
    );
}

#[test]
fn release_names_the_owner() {
    let request = Request::Release {
        removed: user("bob"),
    };
    assert_eq!(
        apply(Some("ann"), request, Actor::Plane),
        Err(Refusal::RemovedIsNotOwner)
    );
}

#[test]
fn the_plane_cannot_claim_hand_over_or_take_over() {
    assert_eq!(
        apply(None, Request::Claim, Actor::Plane),
        Err(Refusal::ActorNotUser)
    );
    for request in [Request::HandOver { to: user("bob") }, Request::TakeOver] {
        assert_eq!(
            apply(Some("ann"), request, Actor::Plane),
            Err(Refusal::ActorNotUser)
        );
    }
}

#[test]
fn the_event_serialises_in_snake_case() {
    let event = apply(
        Some("ann"),
        Request::HandOver { to: user("bob") },
        by("ann"),
    )
    .unwrap();
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "item": "item-1",
            "kind": "handed_over",
            "previous": "ann",
            "new": "bob",
            "actor": {"user": "ann"},
            "at": AT,
            "client": "ui",
        })
    );
    let back: OwnershipEvent = serde_json::from_value(json).unwrap();
    assert_eq!(back, event);
    assert_eq!(
        serde_json::to_value(Actor::Plane).unwrap(),
        serde_json::json!("plane")
    );
    assert_eq!(
        serde_json::to_value(Refusal::RemovedIsNotOwner).unwrap(),
        serde_json::json!("removed_is_not_owner")
    );
}
