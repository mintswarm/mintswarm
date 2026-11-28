//! The envelope keys: the parts of a [`Fact`](super::Fact)'s envelope that its OTLP ids only
//! hash, carried as attributes so that an OTLP record decodes back to the exact fact.
//!
//! This module is the single source of these keys. They are not fields of any attribute type,
//! so a writer never sets them in a fact's `attributes`; the OTLP codec writes them from the
//! envelope (`concerns`, a span's parent, a link's `target`) and reads them back. Other writers
//! of OTLP records take this list and are checked against the golden lines of the OTLP codec
//! in `crates/proto`.
//!
//! The last part of each key is the [`SpanRef`](super::SpanRef) field it carries: `span` (a
//! [`SpanKind`](super::ids::SpanKind)) or `local` (the span id derivation's local parts). A key
//! is absent when its envelope part is absent. Log records also carry the existing
//! [`FACT_ID`] key, the record's own fact id.

/// On a span: the span's own `local` parts (`concerns.local`), a string array.
pub const SPAN_LOCAL: &str = "mintswarm.span.local";

/// On a span: the parent span's kind (`concerns.parent.span`), a span kind string; absent when
/// the parent is the item's root.
pub const SPAN_PARENT_SPAN: &str = "mintswarm.span.parent.span";

/// On a span: the parent span's `local` parts (`concerns.parent.local`), a string array; absent
/// when the parent is the item's root.
pub const SPAN_PARENT_LOCAL: &str = "mintswarm.span.parent.local";

/// On a log record: the kind of the span it concerns (`concerns.span`), a span kind string;
/// absent when the record attaches to the item's root.
pub const CONCERNS_SPAN: &str = "mintswarm.concerns.span";

/// On a log record: the `local` parts of the span it concerns (`concerns.local`), a string
/// array; absent when the record attaches to the item's root.
pub const CONCERNS_LOCAL: &str = "mintswarm.concerns.local";

/// On a link: the target's item (`target.item`), the rendered item key.
pub const LINK_TO_ITEM: &str = "mintswarm.link.to.item";

/// On a link: the kind of the target span (`target.concerns.span`), a span kind string; absent
/// when the link points to the target item's root.
///
/// It names which span the link points to, and is unrelated to `mintswarm.link.kind`, which says
/// what the link means (`trigger`, `parent`).
pub const LINK_TO_SPAN: &str = "mintswarm.link.to.span";

/// On a link: the target span's `local` parts (`target.concerns.local`), a string array; absent
/// when the link points to the target item's root.
pub const LINK_TO_LOCAL: &str = "mintswarm.link.to.local";

/// Every envelope key, in the order a record or link carries them.
pub const KEYS: &[&str] = &[
    SPAN_LOCAL,
    SPAN_PARENT_SPAN,
    SPAN_PARENT_LOCAL,
    CONCERNS_SPAN,
    CONCERNS_LOCAL,
    LINK_TO_ITEM,
    LINK_TO_SPAN,
    LINK_TO_LOCAL,
];

/// The fact id key: a link attribute of the catalogue, also carried by every log record as the
/// record's own fact id in hex. It is not an envelope key, since it is derived, not stored.
pub const FACT_ID: &str = "mintswarm.fact.id";

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use schemars::generate::SchemaSettings;
    use schemars::{JsonSchema, SchemaGenerator};
    use serde_json::Value;

    use super::*;
    use crate::facts::{Fact, FlatAttributes, Link};

    fn object_keys(value: &Value, out: &mut BTreeSet<String>) {
        if let Some(properties) = value.as_object() {
            out.extend(properties.keys().cloned());
        }
    }

    /// Every attribute key the generated `Fact` schema names: each kind's attributes, the
    /// item's and the links'.
    fn attribute_keys() -> BTreeSet<String> {
        let mut generator =
            SchemaGenerator::new(SchemaSettings::default().with(|s| s.inline_subschemas = true));
        let schema = Fact::json_schema(&mut generator).to_value();
        let mut keys = BTreeSet::new();
        for branch in schema["oneOf"].as_array().unwrap() {
            let properties = &branch["properties"];
            object_keys(&properties["attributes"]["properties"], &mut keys);
            object_keys(&properties["item"]["properties"], &mut keys);
            object_keys(&properties["links"]["items"]["properties"], &mut keys);
        }
        keys
    }

    #[test]
    fn schema_holds_item_link_and_kind_keys() {
        let keys = attribute_keys();
        for key in [
            "mintswarm.item.tracker",
            "mintswarm.link.kind",
            "mintswarm.fact.id",
            "mintswarm.provenance",
        ] {
            assert!(keys.contains(key), "{key}");
        }
    }

    #[test]
    fn envelope_keys_are_no_attribute_keys_or_namespaces() {
        let below = |outer: &str, inner: &str| inner.starts_with(&format!("{outer}."));
        for attribute in attribute_keys() {
            for key in KEYS {
                assert_ne!(*key, attribute);
                assert!(
                    !below(key, &attribute),
                    "{key} is a namespace of {attribute}"
                );
                assert!(
                    !below(&attribute, key),
                    "{attribute} is a namespace of {key}"
                );
            }
        }
    }

    #[test]
    fn envelope_keys_are_unique_and_namespaced() {
        let unique: BTreeSet<&str> = KEYS.iter().copied().collect();
        assert_eq!(unique.len(), KEYS.len());
        assert!(KEYS.iter().all(|k| k.starts_with("mintswarm.")));
        assert!(!KEYS.contains(&FACT_ID));
    }

    #[test]
    fn fact_id_is_the_link_attribute() {
        assert!(Link::KEYS.contains(&FACT_ID));
    }
}
