//! The fact catalogue: every fact mintswarm records, its envelope, its attributes and its ids.
//!
//! The types here are the one definition of the fact contract; `api/schemas/fact.schema.json`
//! is generated from them by `cargo xtask schemas`.
//!
//! - [`Fact`] is the serde form a writer hands to the journal: `kind`, `item`, `time` (or
//!   `start` and `end`), `concerns`, `links` (spans only) and `attributes`. It is tagged by
//!   `kind`; [`FactKind`] lists the kinds with their OTLP signal, name, attachment and natural key.
//! - [`kinds`] holds one attribute type per kind, and [`catalogue`] the attribute groups shared
//!   by several kinds and their value sets. Field serde names are the attribute keys.
//! - [`ids`] holds the item key and the trace, span and fact id derivations.
//! - [`envelope`] is the single source of the envelope keys, which carry in an OTLP record the
//!   envelope parts its ids only hash.
//!
//! Keys are frozen once written: new keys and values only add, and [`CATALOGUE_VERSION`]
//! increases with each addition from the first written fact on.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, DeserializeOwned};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::tracker::IssueType;

/// The type of a flat attribute field: `T` when required, `Option<T>` when optional.
macro_rules! attr_ty {
    (required, $t:ty) => { $t };
    (optional, $t:ty) => { Option<$t> };
}

/// Writes one flat attribute field into a map; an absent optional field is left out.
macro_rules! attr_write {
    (required, $map:ident, $key:literal, $value:expr) => {
        serde::ser::SerializeMap::serialize_entry($map, $key, $value)?;
    };
    (optional, $map:ident, $key:literal, $value:expr) => {
        if let Some(value) = $value {
            serde::ser::SerializeMap::serialize_entry($map, $key, value)?;
        }
    };
}

/// Removes one flat attribute field from a map and decodes it.
macro_rules! attr_take {
    (required, $map:ident, $key:literal, $t:ty) => {
        match $map.remove($key) {
            Some(value) => $crate::facts::decode::<$t, E>($key, value)?,
            None => return Err(E::missing_field($key)),
        }
    };
    (optional, $map:ident, $key:literal, $t:ty) => {
        match $map.remove($key) {
            Some(value) => Some($crate::facts::decode::<$t, E>($key, value)?),
            None => None,
        }
    };
}

/// Whether a flat attribute field is required.
macro_rules! attr_required {
    (required) => {
        true
    };
    (optional) => {
        false
    };
}

/// Declares a struct of flat attributes: its groups, held as typed fields, and its own fields,
/// each `required` or `optional` with its attribute key. It implements [`FlatAttributes`], and
/// `Serialize`, `Deserialize` and `JsonSchema` as one flat map: the groups' entries in group
/// order, then the own fields. Deserialising rejects any key no field claims.
macro_rules! attributes {
    (
        $(#[$meta:meta])*
        pub struct $name:ident {
            groups {
                $( $(#[$gmeta:meta])* $group:ident : $gty:ty, )*
            }
            fields {
                $( $(#[$fmeta:meta])* $presence:ident $field:ident : $fty:ty = $key:literal, )*
            }
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq)]
        pub struct $name {
            $( $(#[$gmeta])* pub $group: $gty, )*
            $( $(#[$fmeta])* pub $field: attr_ty!($presence, $fty), )*
        }

        impl $crate::facts::FlatAttributes for $name {
            const KEYS: &'static [&'static str] = &[$($key),*];

            fn write<M: serde::ser::SerializeMap>(&self, map: &mut M) -> Result<(), M::Error> {
                $( $crate::facts::FlatAttributes::write(&self.$group, map)?; )*
                $( attr_write!($presence, map, $key, &self.$field); )*
                Ok(())
            }

            fn take<E: serde::de::Error>(
                map: &mut serde_json::Map<String, serde_json::Value>,
            ) -> Result<Self, E> {
                Ok($name {
                    $( $group: <$gty as $crate::facts::FlatAttributes>::take(map)?, )*
                    $( $field: attr_take!($presence, map, $key, $fty), )*
                })
            }

            fn extend_schema(
                generator: &mut schemars::SchemaGenerator,
                schema: &mut schemars::Schema,
            ) {
                $( <$gty as $crate::facts::FlatAttributes>::extend_schema(generator, schema); )*
                $(
                    $crate::facts::property::<$fty>(
                        generator,
                        schema,
                        $key,
                        attr_required!($presence),
                    );
                )*
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                $crate::facts::serialize_flat(self, serializer)
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                $crate::facts::deserialize_flat(deserializer)
            }
        }

        impl schemars::JsonSchema for $name {
            fn inline_schema() -> bool {
                true
            }

            fn schema_name() -> std::borrow::Cow<'static, str> {
                stringify!($name).into()
            }

            fn schema_id() -> std::borrow::Cow<'static, str> {
                concat!(module_path!(), "::", stringify!($name)).into()
            }

            fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
                $crate::facts::flat_schema::<Self>(generator)
            }
        }
    };
}

pub mod catalogue;
pub mod envelope;
pub mod ids;
pub mod kinds;

use ids::{FactId, ItemKey, SpanId, SpanKind, TraceId};

/// The catalogue's version: the scope version of every fact, raised with each addition from the
/// first written fact on.
pub const CATALOGUE_VERSION: u32 = 1;

/// The OTLP instrumentation scope name of every fact.
pub const SCOPE_NAME: &str = "mintswarm.facts";

/// The scope's `schema_url`: the pinned semantic-conventions version, the one place it is named.
pub const SCHEMA_URL: &str = "https://opentelemetry.io/schemas/1.40.0";

/// A struct whose fields are flat attributes, some of them held in nested groups.
pub trait FlatAttributes: Sized {
    /// The attribute keys of the struct's own fields, groups excluded, in field order.
    const KEYS: &'static [&'static str];

    /// Writes every present attribute into `map`: each group's in group order, then the own.
    fn write<M: serde::ser::SerializeMap>(&self, map: &mut M) -> Result<(), M::Error>;

    /// Removes the struct's keys, its groups' included, from `map` and decodes them.
    fn take<E: de::Error>(map: &mut Map<String, Value>) -> Result<Self, E>;

    /// Adds the struct's properties and required keys, its groups' included, to `schema`.
    fn extend_schema(generator: &mut SchemaGenerator, schema: &mut Schema);
}

pub(crate) fn decode<T: DeserializeOwned, E: de::Error>(key: &str, value: Value) -> Result<T, E> {
    serde_json::from_value(value).map_err(|e| E::custom(format_args!("`{key}`: {e}")))
}

pub(crate) fn property<T: JsonSchema>(
    generator: &mut SchemaGenerator,
    schema: &mut Schema,
    key: &str,
    required: bool,
) {
    let sub = generator.subschema_for::<T>().to_value();
    let object = schema.ensure_object();
    if let Some(properties) = object
        .entry("properties")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
    {
        properties.insert(key.to_owned(), sub);
    }
    if required
        && let Some(list) = object
            .entry("required")
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
    {
        list.push(Value::from(key));
    }
}

pub(crate) fn serialize_flat<T: FlatAttributes, S: serde::Serializer>(
    value: &T,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeMap;
    let mut map = serializer.serialize_map(None)?;
    value.write(&mut map)?;
    map.end()
}

pub(crate) fn deserialize_flat<'de, T: FlatAttributes, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<T, D::Error> {
    let mut map = Map::<String, Value>::deserialize(deserializer)?;
    let value = T::take(&mut map)?;
    match map.keys().next() {
        Some(key) => Err(de::Error::custom(format_args!("unknown field `{key}`"))),
        None => Ok(value),
    }
}

pub(crate) fn flat_schema<T: FlatAttributes>(generator: &mut SchemaGenerator) -> Schema {
    let mut schema = json_schema!({ "type": "object", "additionalProperties": false });
    T::extend_schema(generator, &mut schema);
    schema
}

/// The item a fact is about: the five `mintswarm.item.*` attributes every record carries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "ItemFields", into = "ItemFields")]
pub struct Item {
    key: ItemKey,
    kind: IssueType,
    components: Vec<String>,
}

impl Item {
    /// The item `key`, of `kind`, with its components in the issue's order.
    pub fn new(key: ItemKey, kind: IssueType, components: Vec<String>) -> Self {
        Item {
            key,
            kind,
            components,
        }
    }

    /// The item's key, from which its ids derive.
    pub fn key(&self) -> &ItemKey {
        &self.key
    }

    /// The item's kind.
    pub fn kind(&self) -> IssueType {
        self.kind
    }

    /// The item's components, in the issue's order.
    pub fn components(&self) -> &[String] {
        &self.components
    }
}

/// The item's attributes as written.
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ItemFields {
    /// The tracker host, for example `github.com`.
    #[serde(rename = "mintswarm.item.tracker")]
    #[schemars(regex(pattern = r"^[^/#\s]+$"))]
    tracker: String,
    /// The repository as `owner/repo`.
    #[serde(rename = "mintswarm.item.repository")]
    #[schemars(regex(pattern = r"^[^/#\s]+/[^/#\s]+$"))]
    repository: String,
    #[serde(rename = "mintswarm.item.number")]
    #[schemars(range(min = 1))]
    number: u64,
    #[serde(rename = "mintswarm.item.kind")]
    kind: IssueType,
    #[serde(rename = "mintswarm.item.components")]
    components: Vec<String>,
}

impl TryFrom<ItemFields> for Item {
    type Error = ids::ItemKeyError;

    fn try_from(fields: ItemFields) -> Result<Self, Self::Error> {
        let shape = || ids::ItemKeyError::Shape(fields.repository.clone());
        let (owner, repository) = fields.repository.split_once('/').ok_or_else(shape)?;
        Ok(Item {
            key: ItemKey::new(&fields.tracker, owner, repository, fields.number)?,
            kind: fields.kind,
            components: fields.components,
        })
    }
}

impl From<Item> for ItemFields {
    fn from(item: Item) -> Self {
        ItemFields {
            tracker: item.key.host().to_owned(),
            repository: format!("{}/{}", item.key.owner(), item.key.repository()),
            number: item.key.number(),
            kind: item.kind,
            components: item.components,
        }
    }
}

/// A span of an item, below its root: its kind and its `local` parts (design §4.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpanRef {
    pub span: SpanKind,
    /// The parts after the item key and the kind in the span id derivation.
    pub local: Vec<String>,
}

impl SpanRef {
    /// The span's id within `key`'s trace.
    pub fn span_id(&self, key: &ItemKey) -> SpanId {
        ids::span_id(key, self.span, &self.local)
    }
}

/// Which span a span fact is, and its parent; the span kind is the fact kind's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpanConcerns {
    /// The span's `local` parts.
    pub local: Vec<String>,
    /// The parent span; absent means the item's root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<SpanRef>,
}

/// The span a link points to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkTarget {
    pub item: ItemKey,
    /// The span within the item; absent means the item's root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concerns: Option<SpanRef>,
}

attributes! {
    /// An OTLP span link: a stage run's trigger or a child root's parent.
    pub struct Link {
        groups {
            linked: catalogue::Linked,
        }
        fields {
            required target: LinkTarget = "target",
            /// The record the link is about, for a human trigger.
            optional fact: FactId = "mintswarm.fact.id",
        }
    }
}

/// A fact recorded as an OTLP log record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LogFact<A> {
    pub item: Item,
    /// Unix nanoseconds.
    pub time: u64,
    /// The span the record attaches to; absent means the item's root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concerns: Option<SpanRef>,
    pub attributes: A,
}

/// A fact recorded as an OTLP span below the item's root.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpanFact<A> {
    pub item: Item,
    /// Unix nanoseconds.
    pub start: u64,
    /// Unix nanoseconds.
    pub end: u64,
    pub concerns: SpanConcerns,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Link>,
    pub attributes: A,
}

/// The item's root span.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RootFact<A> {
    pub item: Item,
    /// Unix nanoseconds.
    pub start: u64,
    /// Unix nanoseconds.
    pub end: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Link>,
    pub attributes: A,
}

/// The OTLP signal a fact is recorded as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    Span,
    Log,
}

impl Signal {
    /// The signal as `x-otlp` spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Signal::Span => "span",
            Signal::Log => "log",
        }
    }
}

/// What a fact attaches to: the item's root or a span of a kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attachment {
    Root,
    Span(SpanKind),
}

impl Attachment {
    /// The attachment as `x-otlp` spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Attachment::Root => "root",
            Attachment::Span(kind) => kind.as_str(),
        }
    }
}

/// A fact kind's OTLP encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Otlp {
    pub signal: Signal,
    /// The span name template or the log's event name.
    pub name: &'static str,
    /// For a span the span it is, for a log the spans it may attach to.
    pub attaches_to: &'static [Attachment],
    /// For a log, the ordered parts of the natural key its fact id derives from: attribute
    /// keys, `concerns` (the concerned span id in hex) or `time` (decimal Unix nanoseconds).
    /// A part `a|b` is the first of its alternatives that is present.
    pub natural_key: &'static [&'static str],
}

/// The envelope operations [`Fact`] dispatches to.
trait Envelope {
    fn item(&self) -> &Item;
    fn span_id(&self, kind: FactKind) -> SpanId;
    fn fact_id(&self, kind: FactKind) -> Option<FactId>;

    /// Checks the envelope against the kind's encoding.
    fn check(&self, _kind: FactKind) -> Result<(), String> {
        Ok(())
    }
}

impl<A> Envelope for RootFact<A> {
    fn item(&self) -> &Item {
        &self.item
    }

    fn span_id(&self, _: FactKind) -> SpanId {
        ids::root_span_id(self.item.key())
    }

    fn fact_id(&self, _: FactKind) -> Option<FactId> {
        None
    }
}

impl<A> Envelope for SpanFact<A> {
    fn item(&self) -> &Item {
        &self.item
    }

    fn span_id(&self, kind: FactKind) -> SpanId {
        match kind.otlp().attaches_to.first() {
            Some(Attachment::Span(span)) => {
                ids::span_id(self.item.key(), *span, &self.concerns.local)
            }
            _ => ids::root_span_id(self.item.key()),
        }
    }

    fn fact_id(&self, _: FactKind) -> Option<FactId> {
        None
    }
}

impl<A: Serialize> Envelope for LogFact<A> {
    fn item(&self) -> &Item {
        &self.item
    }

    fn span_id(&self, _: FactKind) -> SpanId {
        match &self.concerns {
            Some(span) => span.span_id(self.item.key()),
            None => ids::root_span_id(self.item.key()),
        }
    }

    fn fact_id(&self, kind: FactKind) -> Option<FactId> {
        let attributes = serde_json::to_value(&self.attributes)
            .expect("catalogue attributes hold only strings, numbers, booleans and arrays");
        let render = |part: &str| match part {
            "concerns" => Some(self.span_id(kind).to_string()),
            "time" => Some(self.time.to_string()),
            key => match attributes.get(key) {
                None => None,
                Some(Value::String(text)) => Some(text.clone()),
                Some(other) => Some(other.to_string()),
            },
        };
        let parts: Vec<String> = kind
            .otlp()
            .natural_key
            .iter()
            .map(|part| part.split('|').find_map(render).unwrap_or_default())
            .collect();
        Some(ids::fact_id(self.item.key(), kind.otlp().name, &parts))
    }

    fn check(&self, kind: FactKind) -> Result<(), String> {
        let allowed = kind.otlp().attaches_to;
        match &self.concerns {
            None if allowed.contains(&Attachment::Root) => Ok(()),
            None => Err(format!(
                "a `{}` fact names the span it concerns in `concerns`",
                kind.tag()
            )),
            Some(span) if allowed.contains(&Attachment::Span(span.span)) => Ok(()),
            Some(span) => Err(format!(
                "a `{}` fact cannot concern a `{}` span",
                kind.tag(),
                span.span.as_str()
            )),
        }
    }
}

/// Declares the kind table once: the [`Fact`] variants with their tags, [`FactKind`] and each
/// kind's [`Otlp`] encoding.
macro_rules! fact_kinds {
    (
        $(
            $(#[$doc:meta])*
            $variant:ident : $envelope:ident = $tag:literal,
            $signal:ident $name:literal,
            attaches [$($attachment:expr),+],
            key [$($part:literal),*];
        )+
    ) => {
        /// A fact, as a writer hands it to the journal, tagged by `kind`.
        ///
        /// Deserialising checks a log's `concerns` against the kind's
        /// [`attaches_to`](Otlp::attaches_to).
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        #[serde(tag = "kind", try_from = "Unchecked")]
        pub enum Fact {
            $(
                $(#[$doc])*
                #[serde(rename = $tag)]
                $variant($envelope<kinds::$variant>),
            )+
        }

        /// A fact as read, before its envelope is checked against its kind.
        #[derive(Deserialize)]
        #[serde(tag = "kind")]
        enum Unchecked {
            $(
                #[serde(rename = $tag)]
                $variant($envelope<kinds::$variant>),
            )+
        }

        impl TryFrom<Unchecked> for Fact {
            type Error = String;

            fn try_from(fact: Unchecked) -> Result<Self, Self::Error> {
                let fact = match fact {
                    $( Unchecked::$variant(fact) => Fact::$variant(fact), )+
                };
                fact.envelope().check(fact.kind())?;
                Ok(fact)
            }
        }

        /// The kind of a fact.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum FactKind {
            $( $(#[$doc])* $variant, )+
        }

        impl FactKind {
            /// Every kind, in the order of the kind table.
            pub const ALL: &'static [FactKind] = &[$(FactKind::$variant),+];

            /// The kind's `kind` tag.
            pub fn tag(self) -> &'static str {
                match self {
                    $( FactKind::$variant => $tag, )+
                }
            }

            /// The kind's OTLP encoding.
            pub fn otlp(self) -> Otlp {
                match self {
                    $(
                        FactKind::$variant => Otlp {
                            signal: Signal::$signal,
                            name: $name,
                            attaches_to: &[$($attachment),+],
                            natural_key: &[$($part),*],
                        },
                    )+
                }
            }

            /// The attribute keys of the kind's own fields, its groups' excluded.
            pub fn own_keys(self) -> &'static [&'static str] {
                match self {
                    $(
                        FactKind::$variant => {
                            <kinds::$variant as FlatAttributes>::KEYS
                        }
                    )+
                }
            }
        }

        impl Fact {
            /// The fact's kind.
            pub fn kind(&self) -> FactKind {
                match self {
                    $( Fact::$variant(_) => FactKind::$variant, )+
                }
            }

            fn envelope(&self) -> &dyn Envelope {
                match self {
                    $( Fact::$variant(fact) => fact, )+
                }
            }
        }

        impl JsonSchema for Fact {
            fn schema_name() -> Cow<'static, str> {
                "Fact".into()
            }

            fn json_schema(generator: &mut SchemaGenerator) -> Schema {
                let branches = vec![
                    $( branch::<$envelope<kinds::$variant>>(generator, FactKind::$variant), )+
                ];
                json_schema!({ "oneOf": branches })
            }
        }
    };
}

/// One `oneOf` branch of [`Fact`]'s schema: the envelope's, with the `kind` tag added first
/// and, for a log, `concerns` narrowed to what the kind attaches to.
fn branch<T: JsonSchema>(generator: &mut SchemaGenerator, kind: FactKind) -> Value {
    let tag = kind.tag();
    let concerns = match kind.otlp().signal {
        Signal::Log => Some(log_concerns(generator, kind)),
        Signal::Span => None,
    };
    let mut schema = T::json_schema(generator);
    let object = schema.ensure_object();
    if let Some(concerns) = concerns {
        apply_concerns(object, concerns);
    }
    if let Some(properties) = object
        .entry("properties")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
    {
        properties.insert(
            "kind".into(),
            serde_json::json!({ "type": "string", "const": tag }),
        );
    }
    if let Some(required) = object
        .entry("required")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
    {
        required.insert(0, Value::from("kind"));
    }
    schema.to_value()
}

/// A log's `concerns` schema for `kind`: `None` when it attaches to the root only, else the
/// [`SpanRef`] schema with `span` limited to the kinds it attaches to, and whether it is required.
fn log_concerns(generator: &mut SchemaGenerator, kind: FactKind) -> Option<(Value, bool)> {
    let allowed = kind.otlp().attaches_to;
    let spans: Vec<Value> = allowed
        .iter()
        .filter_map(|attachment| match attachment {
            Attachment::Span(span) => Some(Value::from(span.as_str())),
            Attachment::Root => None,
        })
        .collect();
    if spans.is_empty() {
        return None;
    }
    let mut schema = SpanRef::json_schema(generator).to_value();
    schema["properties"]["span"] = serde_json::json!({ "type": "string", "enum": spans });
    Some((schema, !allowed.contains(&Attachment::Root)))
}

/// Replaces a log branch's `concerns` property with `concerns`, or removes it.
fn apply_concerns(object: &mut Map<String, Value>, concerns: Option<(Value, bool)>) {
    let Some(Value::Object(properties)) = object.get_mut("properties") else {
        return;
    };
    match concerns {
        None => {
            properties.remove("concerns");
        }
        Some((schema, required)) => {
            properties.insert("concerns".into(), schema);
            if required
                && let Some(list) = object
                    .entry("required")
                    .or_insert_with(|| Value::Array(Vec::new()))
                    .as_array_mut()
            {
                list.push(Value::from("concerns"));
            }
        }
    }
}

use Attachment::{Root, Span};
use SpanKind::{Phase, StageRun, ToolCall, Turn, Wait};

fact_kinds! {
    /// The item's root span: from issue creation to landing.
    ItemRoot: RootFact = "item_root",
        Span "mintswarm.item",
        attaches [Root],
        key [];
    /// A stage run has started.
    StageRunStarted: LogFact = "stage_run_started",
        Log "mintswarm.stage_run.started",
        attaches [Span(StageRun)],
        key ["mintswarm.run.id"];
    /// A stage run, below the root; its links are its triggers.
    StageRun: SpanFact = "stage_run",
        Span "invoke_agent {mintswarm.stage}",
        attaches [Span(StageRun)],
        key [];
    /// One model turn of a run.
    Turn: SpanFact = "turn",
        Span "chat {gen_ai.request.model}",
        attaches [Span(Turn)],
        key [];
    /// One tool call of a run.
    ToolCall: SpanFact = "tool_call",
        Span "execute_tool {gen_ai.tool.name}",
        attaches [Span(ToolCall)],
        key [];
    /// A named phase of a run.
    Phase: SpanFact = "phase",
        Span "mintswarm.phase {mintswarm.phase}",
        attaches [Span(Phase)],
        key [];
    /// A wait has started.
    WaitStarted: LogFact = "wait_started",
        Log "mintswarm.wait.started",
        attaches [Span(Wait)],
        key ["concerns"];
    /// A wait, below the root.
    Wait: SpanFact = "wait",
        Span "mintswarm.wait {mintswarm.wait.kind}",
        attaches [Span(Wait)],
        key [];
    /// A judge scored a run's output.
    ConfidenceScored: LogFact = "confidence_scored",
        Log "mintswarm.confidence.scored",
        attaches [Span(StageRun)],
        key ["concerns", "mintswarm.judge"];
    /// A decision was requested.
    DecisionRequested: LogFact = "decision_requested",
        Log "mintswarm.decision.requested",
        attaches [Span(StageRun)],
        key ["concerns", "mintswarm.decision.kind", "mintswarm.decision.url"];
    /// A decision was taken.
    DecisionTaken: LogFact = "decision_taken",
        Log "mintswarm.decision.taken",
        attaches [Span(StageRun)],
        key ["concerns", "mintswarm.decision.kind", "mintswarm.decision.url"];
    /// A person intervened in a run or an item.
    Intervention: LogFact = "intervention",
        Log "mintswarm.intervention",
        attaches [Span(StageRun), Root],
        key ["concerns", "mintswarm.intervention.kind", "mintswarm.intervention.url|time"];
    /// The item was picked up.
    PickedUp: LogFact = "picked_up",
        Log "mintswarm.item.picked_up",
        attaches [Root],
        key [];
    /// The item's state changed.
    StateChanged: LogFact = "state_changed",
        Log "mintswarm.state.changed",
        attaches [Root],
        key ["time", "mintswarm.state.from", "mintswarm.state.to"];
    /// A link from the item was added or removed.
    LinkChanged: LogFact = "link_changed",
        Log "mintswarm.link.changed",
        attaches [Root],
        key ["time", "mintswarm.link.kind", "mintswarm.link.target", "mintswarm.link.op"];
    /// A PR for the item was opened.
    PrOpened: LogFact = "pr_opened",
        Log "mintswarm.pr.opened",
        attaches [Root],
        key ["mintswarm.pr.number"];
    /// A PR for the item was merged.
    PrMerged: LogFact = "pr_merged",
        Log "mintswarm.pr.merged",
        attaches [Root],
        key ["mintswarm.pr.number"];
    /// A PR for the item was reverted.
    PrReverted: LogFact = "pr_reverted",
        Log "mintswarm.pr.reverted",
        attaches [Root],
        key ["mintswarm.revert.pr"];
    /// The item was linked as a regression of another.
    RegressionLinked: LogFact = "regression_linked",
        Log "mintswarm.regression.linked",
        attaches [Root],
        key ["mintswarm.regression.of"];
}

impl Fact {
    /// The item the fact is about.
    pub fn item(&self) -> &Item {
        self.envelope().item()
    }

    /// The item's trace id.
    pub fn trace_id(&self) -> TraceId {
        ids::trace_id(self.item().key())
    }

    /// The span the record is, for a span, or attaches to, for a log.
    pub fn span_id(&self) -> SpanId {
        self.envelope().span_id(self.kind())
    }

    /// The fact id of a log record, from its kind's natural key; `None` for spans.
    pub fn fact_id(&self) -> Option<FactId> {
        self.envelope().fact_id(self.kind())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use schemars::generate::SchemaSettings;
    use serde_json::json;

    use super::catalogue::*;
    use super::*;
    use crate::tracker::IssueType;

    fn generator() -> SchemaGenerator {
        SchemaGenerator::new(SchemaSettings::default().with(|s| s.inline_subschemas = true))
    }

    fn property_names<T: JsonSchema>() -> Vec<String> {
        let schema = T::json_schema(&mut generator()).to_value();
        schema["properties"]
            .as_object()
            .map(|p| p.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn item() -> Item {
        Item::new(
            "github.com/mintswarm/mintswarm#412".parse().unwrap(),
            IssueType::Implementation,
            vec!["domain".into()],
        )
    }

    fn live() -> Provenance {
        Provenance {
            kind: ProvenanceKind::Live,
            source: ProvenanceSource::Session,
            gaps: None,
        }
    }

    fn run() -> Run {
        Run {
            stage: Stage::Implement,
            harness: Harness::Claude,
            effort: None,
            round: 1,
            id: "agent-1".into(),
            trigger: Trigger::Pipeline,
        }
    }

    fn stage_run_span() -> SpanRef {
        SpanRef {
            span: SpanKind::StageRun,
            local: vec!["agent-1".into()],
        }
    }

    fn log_fact() -> Fact {
        Fact::StageRunStarted(LogFact {
            item: item(),
            time: 1_764_151_200_000_000_000,
            concerns: Some(stage_run_span()),
            attributes: kinds::StageRunStarted {
                provenance: live(),
                run: run(),
            },
        })
    }

    fn span_fact() -> Fact {
        Fact::Phase(SpanFact {
            item: item(),
            start: 1,
            end: 2,
            concerns: SpanConcerns {
                local: vec!["0123456789abcdef".into(), "build".into(), "1".into()],
                parent: Some(stage_run_span()),
            },
            links: vec![Link {
                linked: Linked {
                    kind: LinkKind::Trigger,
                },
                target: LinkTarget {
                    item: "github.com/mintswarm/mintswarm#411".parse().unwrap(),
                    concerns: None,
                },
                fact: Some(FactId([7; 16])),
            }],
            attributes: kinds::Phase {
                provenance: live(),
                name: "build".into(),
            },
        })
    }

    fn root_fact() -> Fact {
        Fact::ItemRoot(RootFact {
            item: item(),
            start: 1,
            end: 2,
            links: Vec::new(),
            attributes: kinds::ItemRoot {
                provenance: live(),
                outcome: ItemOutcome::Merged,
                landed_by: Some(530),
            },
        })
    }

    fn keys(value: &Value) -> Vec<&str> {
        value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect()
    }

    fn group_keys() -> Vec<&'static [&'static str]> {
        vec![
            Provenance::KEYS,
            Run::KEYS,
            Model::KEYS,
            Tokens::KEYS,
            StageResult::KEYS,
            catalogue::Wait::KEYS,
            Actor::KEYS,
            Decision::KEYS,
            Linked::KEYS,
            Pr::KEYS,
        ]
    }

    /// Every attribute key declared by a field: the groups', the kinds' own, the link's and the
    /// item's.
    fn declared_keys() -> Vec<String> {
        let mut out: Vec<String> = group_keys()
            .into_iter()
            .flatten()
            .map(|k| (*k).to_owned())
            .collect();
        for kind in FactKind::ALL {
            out.extend(kind.own_keys().iter().map(|k| (*k).to_owned()));
        }
        out.extend(Link::KEYS.iter().map(|k| (*k).to_owned()));
        out.extend(property_names::<Item>());
        out
    }

    const DESIGN_MINTSWARM_KEYS: &[&str] = &[
        "mintswarm.provenance",
        "mintswarm.provenance.source",
        "mintswarm.provenance.gaps",
        "mintswarm.item.tracker",
        "mintswarm.item.repository",
        "mintswarm.item.number",
        "mintswarm.item.kind",
        "mintswarm.item.components",
        "mintswarm.item.outcome",
        "mintswarm.item.landed_by",
        "mintswarm.stage",
        "mintswarm.harness",
        "mintswarm.effort",
        "mintswarm.round",
        "mintswarm.run.id",
        "mintswarm.run.queued_at",
        "mintswarm.trigger",
        "mintswarm.outcome.status",
        "mintswarm.result.findings.blocker",
        "mintswarm.result.findings.major",
        "mintswarm.result.findings.minor",
        "mintswarm.result.findings.nit",
        "mintswarm.result.gate_verdict",
        "mintswarm.result.lines_added",
        "mintswarm.result.lines_removed",
        "mintswarm.result.questions",
        "mintswarm.result.must_decide",
        "mintswarm.result.assumptions",
        "mintswarm.activity",
        "mintswarm.activity.source",
        "mintswarm.tool.program",
        "mintswarm.mcp.server",
        "mintswarm.phase",
        "mintswarm.wait.kind",
        "mintswarm.judge",
        "mintswarm.confidence.value",
        "mintswarm.confidence.band",
        "mintswarm.confidence.threshold",
        "mintswarm.confidence.hard_triggers",
        "mintswarm.decision.kind",
        "mintswarm.decision.verdict",
        "mintswarm.decision.required",
        "mintswarm.decision.where",
        "mintswarm.decision.url",
        "mintswarm.decision.answers.recommended",
        "mintswarm.decision.answers.other_option",
        "mintswarm.decision.answers.free_text",
        "mintswarm.decision.must_decide",
        "mintswarm.decision.assumptions.confirmed",
        "mintswarm.decision.assumptions.overruled",
        "mintswarm.decision.assumptions.silent",
        "mintswarm.decision.confidence_fact",
        "mintswarm.actor.kind",
        "mintswarm.actor.id",
        "mintswarm.intervention.kind",
        "mintswarm.steer.kind",
        "mintswarm.steer.via",
        "mintswarm.intervention.url",
        "mintswarm.state.from",
        "mintswarm.state.to",
        "mintswarm.link.kind",
        "mintswarm.link.target",
        "mintswarm.link.op",
        "mintswarm.pr.number",
        "mintswarm.pr.authored_by",
        "mintswarm.revert.pr",
        "mintswarm.regression.of",
        "mintswarm.fact.id",
    ];

    const DESIGN_GEN_AI_KEYS: &[&str] = &[
        "gen_ai.operation.name",
        "gen_ai.agent.name",
        "gen_ai.provider.name",
        "gen_ai.request.model",
        "gen_ai.response.model",
        "gen_ai.response.id",
        "gen_ai.tool.name",
        "gen_ai.tool.call.id",
        "gen_ai.usage.input_tokens",
        "gen_ai.usage.output_tokens",
        "gen_ai.usage.cache_read.input_tokens",
        "gen_ai.usage.cache_creation.input_tokens",
    ];

    #[test]
    fn catalogue_version_is_one() {
        assert_eq!(CATALOGUE_VERSION, 1_u32);
    }

    #[test]
    fn one_type_per_kind() {
        let tags: Vec<&str> = FactKind::ALL.iter().map(|k| k.tag()).collect();
        assert_eq!(
            tags,
            [
                "item_root",
                "stage_run_started",
                "stage_run",
                "turn",
                "tool_call",
                "phase",
                "wait_started",
                "wait",
                "confidence_scored",
                "decision_requested",
                "decision_taken",
                "intervention",
                "picked_up",
                "state_changed",
                "link_changed",
                "pr_opened",
                "pr_merged",
                "pr_reverted",
                "regression_linked",
            ]
        );
        for fact in [log_fact(), span_fact(), root_fact()] {
            let value = serde_json::to_value(&fact).unwrap();
            assert_eq!(value["kind"], json!(fact.kind().tag()));
            let back: Fact = serde_json::from_value(value).unwrap();
            assert_eq!(back, fact);
        }
    }

    #[test]
    fn mintswarm_keys_declared_once() {
        let declared = declared_keys();
        let mintswarm: Vec<&str> = declared
            .iter()
            .map(String::as_str)
            .filter(|k| k.starts_with("mintswarm."))
            .collect();
        let unique: BTreeSet<&str> = mintswarm.iter().copied().collect();
        assert_eq!(unique.len(), mintswarm.len(), "a key is declared twice");
        let design: BTreeSet<&str> = DESIGN_MINTSWARM_KEYS.iter().copied().collect();
        assert_eq!(design.len(), DESIGN_MINTSWARM_KEYS.len());
        assert_eq!(unique, design);
    }

    #[test]
    fn declared_keys_cover_every_kind_schema() {
        let declared: BTreeSet<String> = declared_keys().into_iter().collect();
        let mut generator = generator();
        let schema = Fact::json_schema(&mut generator).to_value();
        for branch in schema["oneOf"].as_array().unwrap() {
            for key in keys(&branch["properties"]["attributes"]["properties"]) {
                assert!(declared.contains(key), "{key}");
            }
        }
    }

    #[test]
    fn semconv_keys_are_fields() {
        let declared: BTreeSet<String> = declared_keys().into_iter().collect();
        for key in DESIGN_GEN_AI_KEYS {
            assert!(declared.contains(*key), "{key}");
        }
        let gen_ai: BTreeSet<&str> = declared
            .iter()
            .map(String::as_str)
            .filter(|k| k.starts_with("gen_ai."))
            .collect();
        assert_eq!(gen_ai, DESIGN_GEN_AI_KEYS.iter().copied().collect());
        assert_eq!(
            property_names::<HostUtilisation>(),
            [
                "system.cpu.utilization",
                "system.filesystem.utilization",
                "system.memory.utilization",
            ]
        );
        assert!(SCHEMA_URL.starts_with("https://opentelemetry.io/schemas/"));
    }

    fn spellings<T: Serialize>(values: &[T]) -> Vec<String> {
        values
            .iter()
            .map(|v| {
                serde_json::to_value(v)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn value_spellings_match_design() {
        let cases: Vec<(Vec<String>, &[&str])> = vec![
            (spellings(ProvenanceKind::ALL), &["live", "reconstructed"]),
            (
                spellings(ProvenanceSource::ALL),
                &[
                    "session",
                    "tool",
                    "runner",
                    "plane",
                    "transcript",
                    "github",
                    "job_record",
                ],
            ),
            (
                spellings(&[
                    IssueType::Design,
                    IssueType::Plan,
                    IssueType::Implementation,
                ]),
                &["design", "plan", "implementation"],
            ),
            (
                spellings(ItemOutcome::ALL),
                &["merged", "decomposed", "closed"],
            ),
            (
                spellings(Stage::ALL),
                &[
                    "analyse",
                    "design",
                    "decompose",
                    "assess-decomposition",
                    "plan",
                    "assess",
                    "implement",
                    "gates",
                    "review",
                    "address",
                    "merge",
                    "landing",
                ],
            ),
            (spellings(Harness::ALL), &["claude", "codex"]),
            (
                spellings(Trigger::ALL),
                &["pipeline", "human", "factory", "retry"],
            ),
            (
                spellings(RunOutcome::ALL),
                &["complete", "needs_input", "failed", "unknown"],
            ),
            (
                spellings(ActivitySource::ALL),
                &["default", "project", "none", "unparsed"],
            ),
            (
                spellings(WaitKind::ALL),
                &["blocked", "to_start", "runner", "human"],
            ),
            (spellings(Judge::ALL), &["plan", "landing", "decomposition"]),
            (
                spellings(DecisionKind::ALL),
                &[
                    "design_approval",
                    "plan_approval",
                    "decomposition_approval",
                    "landing_approval",
                    "question_round",
                    "round_cap",
                ],
            ),
            (
                spellings(DecisionVerdict::ALL),
                &[
                    "approved",
                    "changes_requested",
                    "rejected",
                    "answered",
                    "continue",
                    "accept",
                ],
            ),
            (spellings(DecisionWhere::ALL), &["session", "github"]),
            (
                spellings(ActorKind::ALL),
                &["agent", "human", "judge", "rule", "delegated"],
            ),
            (
                spellings(InterventionKind::ALL),
                &["assumption_overruled", "steer", "human_review"],
            ),
            (
                spellings(LinkKind::ALL),
                &["blocked_by", "sub_issue", "trigger", "parent"],
            ),
            (spellings(LinkOp::ALL), &["added", "removed"]),
            (spellings(PrAuthor::ALL), &["factory", "human"]),
            (
                spellings(&[
                    serde_json::to_value(InvokeAgent::InvokeAgent).unwrap(),
                    serde_json::to_value(Chat::Chat).unwrap(),
                    serde_json::to_value(ExecuteTool::ExecuteTool).unwrap(),
                ]),
                &["invoke_agent", "chat", "execute_tool"],
            ),
        ];
        for (actual, expected) in cases {
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn run_outcome_extends_outcome_status() {
        let schema = RunOutcome::json_schema(&mut generator()).to_value();
        assert_eq!(
            schema["enum"],
            json!(["complete", "needs_input", "failed", "unknown"])
        );
        for outcome in RunOutcome::ALL {
            let value = serde_json::to_value(outcome).unwrap();
            assert_eq!(
                serde_json::from_value::<RunOutcome>(value).unwrap(),
                *outcome
            );
        }
        assert_eq!(
            serde_json::to_value(RunOutcome::Known(OutcomeStatus::NeedsInput)).unwrap(),
            serde_json::to_value(OutcomeStatus::NeedsInput).unwrap()
        );
        assert!(serde_json::from_value::<RunOutcome>(json!("skipped")).is_err());
    }

    #[test]
    fn envelope_fields() {
        let log = serde_json::to_value(log_fact()).unwrap();
        assert_eq!(
            keys(&log),
            ["attributes", "concerns", "item", "kind", "time"]
        );
        let span = serde_json::to_value(span_fact()).unwrap();
        assert_eq!(
            keys(&span),
            [
                "attributes",
                "concerns",
                "end",
                "item",
                "kind",
                "links",
                "start"
            ]
        );
        assert_eq!(
            keys(&span["links"][0]),
            ["mintswarm.fact.id", "mintswarm.link.kind", "target"]
        );
        let root = serde_json::to_value(root_fact()).unwrap();
        assert_eq!(keys(&root), ["attributes", "end", "item", "kind", "start"]);
        assert_eq!(
            keys(&root["item"]),
            [
                "mintswarm.item.components",
                "mintswarm.item.kind",
                "mintswarm.item.number",
                "mintswarm.item.repository",
                "mintswarm.item.tracker",
            ]
        );
        assert_eq!(
            root["item"]["mintswarm.item.repository"],
            json!("mintswarm/mintswarm")
        );
    }

    #[test]
    fn rejects_unknown_and_out_of_set() {
        let fails = |value: Value| serde_json::from_value::<Fact>(value).is_err();
        let span = serde_json::to_value(span_fact()).unwrap();
        assert!(serde_json::from_value::<Fact>(span.clone()).is_ok());
        let with = |pointer: &str, key: &str| {
            let mut value = span.clone();
            let target = if pointer.is_empty() {
                &mut value
            } else {
                value.pointer_mut(pointer).unwrap()
            };
            target[key] = json!(1);
            fails(value)
        };
        assert!(with("", "extra"));
        assert!(with("/item", "extra"));
        assert!(with("/concerns", "extra"));
        assert!(with("/concerns/parent", "extra"));
        assert!(with("/links/0", "extra"));
        assert!(with("/links/0/target", "extra"));
        assert!(with("/attributes", "extra"));

        let mut unknown_kind = span.clone();
        unknown_kind["kind"] = json!("deploy");
        assert!(fails(unknown_kind));

        let tokens = json!({
            "gen_ai.usage.input_tokens": 1,
            "gen_ai.usage.output_tokens": 1,
            "gen_ai.usage.cache_read.input_tokens": 1,
            "gen_ai.usage.cache_creation.input_tokens": 1,
        });
        assert!(serde_json::from_value::<Tokens>(tokens.clone()).is_ok());
        let mut extra = tokens;
        extra["gen_ai.usage.reasoning_tokens"] = json!(1);
        assert!(serde_json::from_value::<Tokens>(extra).is_err());

        let mut stage = serde_json::to_value(log_fact()).unwrap();
        stage["attributes"]["mintswarm.stage"] = json!("deploy");
        assert!(fails(stage));

        let started = serde_json::to_value(log_fact()).unwrap();
        let mut run = json!({
            "kind": "stage_run",
            "item": started["item"],
            "start": 1,
            "end": 2,
            "concerns": { "local": ["agent-1"] },
            "attributes": started["attributes"],
        });
        let attributes = run["attributes"].as_object_mut().unwrap();
        for (key, value) in [
            ("gen_ai.provider.name", json!("anthropic")),
            ("gen_ai.request.model", json!("claude-opus-4")),
            ("gen_ai.operation.name", json!("invoke_agent")),
            ("gen_ai.agent.name", json!("msw-implement")),
            ("mintswarm.outcome.status", json!("complete")),
        ] {
            attributes.insert(key.into(), value);
        }
        assert!(serde_json::from_value::<Fact>(run.clone()).is_ok());
        let mut skipped = run.clone();
        skipped["attributes"]["mintswarm.outcome.status"] = json!("skipped");
        assert!(fails(skipped));
        let mut operation = run;
        operation["attributes"]["gen_ai.operation.name"] = json!("chat");
        assert!(fails(operation));
    }

    #[test]
    fn ids_follow_the_envelope() {
        let key = item().key().clone();
        let log = log_fact();
        assert_eq!(log.trace_id(), ids::trace_id(&key));
        assert_eq!(
            log.span_id(),
            ids::span_id(&key, SpanKind::StageRun, &["agent-1"])
        );
        assert_eq!(
            log.fact_id(),
            Some(ids::fact_id(
                &key,
                "mintswarm.stage_run.started",
                &["agent-1"]
            ))
        );
        let span = span_fact();
        assert_eq!(
            span.span_id(),
            ids::span_id(&key, SpanKind::Phase, &["0123456789abcdef", "build", "1"])
        );
        assert_eq!(span.fact_id(), None);
        assert_eq!(root_fact().span_id(), ids::root_span_id(&key));
    }

    fn picked_up(concerns: Option<SpanRef>) -> Value {
        serde_json::to_value(Fact::PickedUp(LogFact {
            item: item(),
            time: 1,
            concerns,
            attributes: kinds::PickedUp { provenance: live() },
        }))
        .unwrap()
    }

    fn intervention(time: u64, url: Option<&str>) -> Fact {
        Fact::Intervention(LogFact {
            item: item(),
            time,
            concerns: None,
            attributes: kinds::Intervention {
                provenance: live(),
                actor: Actor {
                    kind: ActorKind::Human,
                    id: None,
                },
                kind: InterventionKind::AssumptionOverruled,
                steer_kind: None,
                steer_via: None,
                url: url.map(str::to_owned),
            },
        })
    }

    #[test]
    fn concerns_follow_attaches_to() {
        let parses = |value: Value| serde_json::from_value::<Fact>(value).is_ok();
        let turn = SpanRef {
            span: SpanKind::Turn,
            local: vec!["0123456789abcdef".into(), "msg-1".into()],
        };
        assert!(parses(picked_up(None)));
        assert!(!parses(picked_up(Some(stage_run_span()))));

        let started = serde_json::to_value(log_fact()).unwrap();
        assert!(parses(started.clone()));
        let mut without = started.clone();
        without.as_object_mut().unwrap().remove("concerns");
        assert!(!parses(without));
        let mut on_turn = started;
        on_turn["concerns"] = serde_json::to_value(&turn).unwrap();
        assert!(!parses(on_turn));

        let root = serde_json::to_value(intervention(1, None)).unwrap();
        assert!(parses(root.clone()));
        let mut on_run = root.clone();
        on_run["concerns"] = serde_json::to_value(stage_run_span()).unwrap();
        assert!(parses(on_run));
        let mut on_turn = root;
        on_turn["concerns"] = serde_json::to_value(&turn).unwrap();
        assert!(!parses(on_turn));
    }

    #[test]
    fn schema_concerns_follow_attaches_to() {
        let schema = Fact::json_schema(&mut generator()).to_value();
        let branch = |tag: &str| {
            schema["oneOf"]
                .as_array()
                .unwrap()
                .iter()
                .find(|b| b["properties"]["kind"]["const"] == tag)
                .unwrap()
                .clone()
        };
        let required = |branch: &Value| branch["required"].as_array().unwrap().clone();
        for kind in FactKind::ALL {
            let otlp = kind.otlp();
            let branch = branch(kind.tag());
            let concerns = &branch["properties"]["concerns"];
            match otlp.signal {
                Signal::Span => continue,
                Signal::Log if otlp.attaches_to == [Root] => {
                    assert!(concerns.is_null(), "{kind:?}");
                }
                Signal::Log => {
                    let spans: Vec<&str> = otlp
                        .attaches_to
                        .iter()
                        .filter(|a| **a != Root)
                        .map(|a| a.as_str())
                        .collect();
                    assert_eq!(concerns["properties"]["span"]["enum"], json!(spans));
                    assert_eq!(
                        required(&branch).contains(&json!("concerns")),
                        !otlp.attaches_to.contains(&Root),
                        "{kind:?}"
                    );
                }
            }
        }
        assert!(branch("picked_up")["properties"]["concerns"].is_null());
        assert!(required(&branch("confidence_scored")).contains(&json!("concerns")));
        assert!(!required(&branch("intervention")).contains(&json!("concerns")));
    }

    #[test]
    fn url_or_time_tells_interventions_apart() {
        assert_ne!(
            intervention(1, None).fact_id(),
            intervention(2, None).fact_id()
        );
        let url = "https://github.com/mintswarm/mintswarm/pull/531#issuecomment-2";
        assert_eq!(
            intervention(1, Some(url)).fact_id(),
            intervention(2, Some(url)).fact_id()
        );
        assert_eq!(
            intervention(1, Some(url)).fact_id(),
            Some(ids::fact_id(
                item().key(),
                "mintswarm.intervention",
                &[
                    ids::root_span_id(item().key()).to_string().as_str(),
                    "assumption_overruled",
                    url,
                ]
            ))
        );
        assert_eq!(
            intervention(7, None).fact_id(),
            Some(ids::fact_id(
                item().key(),
                "mintswarm.intervention",
                &[
                    ids::root_span_id(item().key()).to_string().as_str(),
                    "assumption_overruled",
                    "7",
                ]
            ))
        );
    }

    #[test]
    fn natural_keys_name_declared_parts() {
        let declared: BTreeSet<String> = declared_keys().into_iter().collect();
        for kind in FactKind::ALL {
            let otlp = kind.otlp();
            match otlp.signal {
                Signal::Span => assert!(otlp.natural_key.is_empty(), "{kind:?}"),
                Signal::Log => {
                    for part in otlp.natural_key.iter().flat_map(|p| p.split('|')) {
                        assert!(
                            matches!(part, "concerns" | "time") || declared.contains(part),
                            "{kind:?}: {part}"
                        );
                    }
                }
            }
        }
    }
}
