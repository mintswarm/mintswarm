//! The plane identifiers round-trip through JSON.

use std::fmt::Debug;

use mintswarm_domain::ids::{AccountNickname, DirectiveId, LeaseEpoch, ProjectId, RunnerId};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

fn round_trip<T>(value: &T) -> Value
where
    T: Serialize + DeserializeOwned + PartialEq + Debug,
{
    let text = serde_json::to_string(value).expect("serialises");
    let back: T = serde_json::from_str(&text).expect("deserialises");
    assert_eq!(&back, value);
    serde_json::from_str(&text).expect("is JSON")
}

#[test]
fn ids_round_trip() {
    assert_eq!(
        round_trip(&ProjectId("mintswarm".into())),
        json!("mintswarm")
    );
    assert_eq!(round_trip(&AccountNickname("work".into())), json!("work"));
    assert_eq!(round_trip(&RunnerId("runner-1".into())), json!("runner-1"));
}

#[test]
fn lease_epoch_and_directive_id_live_in_ids() {
    assert_eq!(round_trip(&LeaseEpoch(3)), json!(3));
    assert_eq!(round_trip(&DirectiveId("d-1".into())), json!("d-1"));
}
