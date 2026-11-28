//! The mapping between a [`Fact`] and its export request; the rules are in the parent
//! module's documentation.

use std::fmt;

use mintswarm_domain::facts::catalogue::{OutcomeStatus, RunOutcome};
use mintswarm_domain::facts::ids::{self, ItemKey, SpanId};
use mintswarm_domain::facts::{
    CATALOGUE_VERSION, Fact, FactKind, Item, Link, LogFact, RootFact, SCHEMA_URL, SCOPE_NAME,
    Signal, SpanFact, SpanRef, envelope,
};
use mintswarm_domain::tracker::IssueType;
use serde::Serialize;
use serde::ser::{self, Impossible};
use serde_json::{Map, Number, Value};

use super::{Error, Export, Origin};
use crate::opentelemetry::proto::collector::logs::v1::ExportLogsServiceRequest;
use crate::opentelemetry::proto::collector::trace::v1::ExportTraceServiceRequest;
use crate::opentelemetry::proto::common::v1::{
    AnyValue, ArrayValue, InstrumentationScope, KeyValue, any_value,
};
use crate::opentelemetry::proto::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use crate::opentelemetry::proto::resource::v1::Resource;
use crate::opentelemetry::proto::trace::v1::{
    ResourceSpans, ScopeSpans, Span, Status, span, status,
};

/// The serde name of a link's `target` field, which OTLP carries as the link's ids and the
/// `mintswarm.link.to.*` keys.
const TARGET: &str = "target";

/// An attribute key and its serde value, in the order the fact serialises them.
type Entries = Vec<(String, Value)>;

// ---------------------------------------------------------------------------------------------
// Capturing attributes in field order.

/// Why a value is not a map of attributes.
#[derive(Debug)]
struct CaptureError(String);

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CaptureError {}

impl ser::Error for CaptureError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        CaptureError(msg.to_string())
    }
}

/// A serializer that accepts only a map or a struct, and records its entries in order.
struct Capture;

/// The entries of a map or struct being captured.
struct CaptureMap {
    entries: Entries,
    key: Option<String>,
}

fn not_a_map<T>() -> Result<T, CaptureError> {
    Err(CaptureError("attributes serialise as a map".into()))
}

fn to_value<T: Serialize + ?Sized>(value: &T) -> Result<Value, CaptureError> {
    serde_json::to_value(value).map_err(|e| CaptureError(e.to_string()))
}

macro_rules! refuse {
    ($($name:ident($($arg:ty),*);)*) => {
        $(
            fn $name(self, $(_: $arg),*) -> Result<Self::Ok, Self::Error> {
                not_a_map()
            }
        )*
    };
}

impl ser::Serializer for Capture {
    type Ok = Entries;
    type Error = CaptureError;
    type SerializeSeq = Impossible<Entries, CaptureError>;
    type SerializeTuple = Impossible<Entries, CaptureError>;
    type SerializeTupleStruct = Impossible<Entries, CaptureError>;
    type SerializeTupleVariant = Impossible<Entries, CaptureError>;
    type SerializeMap = CaptureMap;
    type SerializeStruct = CaptureMap;
    type SerializeStructVariant = Impossible<Entries, CaptureError>;

    refuse! {
        serialize_bool(bool);
        serialize_i8(i8);
        serialize_i16(i16);
        serialize_i32(i32);
        serialize_i64(i64);
        serialize_u8(u8);
        serialize_u16(u16);
        serialize_u32(u32);
        serialize_u64(u64);
        serialize_f32(f32);
        serialize_f64(f64);
        serialize_char(char);
        serialize_str(&str);
        serialize_bytes(&[u8]);
        serialize_none();
        serialize_unit();
        serialize_unit_struct(&'static str);
        serialize_unit_variant(&'static str, u32, &'static str);
    }

    fn serialize_some<T: Serialize + ?Sized>(self, _: &T) -> Result<Entries, CaptureError> {
        not_a_map()
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: &T,
    ) -> Result<Entries, CaptureError> {
        not_a_map()
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: &T,
    ) -> Result<Entries, CaptureError> {
        not_a_map()
    }

    fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq, CaptureError> {
        not_a_map()
    }

    fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple, CaptureError> {
        not_a_map()
    }

    fn serialize_tuple_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleStruct, CaptureError> {
        not_a_map()
    }

    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleVariant, CaptureError> {
        not_a_map()
    }

    fn serialize_map(self, _: Option<usize>) -> Result<CaptureMap, CaptureError> {
        Ok(CaptureMap {
            entries: Vec::new(),
            key: None,
        })
    }

    fn serialize_struct(self, _: &'static str, _: usize) -> Result<CaptureMap, CaptureError> {
        self.serialize_map(None)
    }

    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStructVariant, CaptureError> {
        not_a_map()
    }
}

impl ser::SerializeMap for CaptureMap {
    type Ok = Entries;
    type Error = CaptureError;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), CaptureError> {
        match to_value(key)? {
            Value::String(key) => {
                self.key = Some(key);
                Ok(())
            }
            _ => Err(CaptureError("an attribute key is not a string".into())),
        }
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), CaptureError> {
        let key = self
            .key
            .take()
            .ok_or_else(|| CaptureError("a value without a key".into()))?;
        self.entries.push((key, to_value(value)?));
        Ok(())
    }

    fn end(self) -> Result<Entries, CaptureError> {
        Ok(self.entries)
    }
}

impl ser::SerializeStruct for CaptureMap {
    type Ok = Entries;
    type Error = CaptureError;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), CaptureError> {
        self.entries.push((key.to_owned(), to_value(value)?));
        Ok(())
    }

    fn end(self) -> Result<Entries, CaptureError> {
        Ok(self.entries)
    }
}

/// The entries of `value`, which must serialise as a map or a struct, in order.
fn entries<T: Serialize>(value: &T) -> Result<Entries, Error> {
    value
        .serialize(Capture)
        .map_err(|e| Error::Value(e.to_string()))
}

/// The item attribute keys, in the order [`Item`] writes them.
fn item_keys() -> Result<Vec<String>, Error> {
    let key =
        ItemKey::new("host", "owner", "repository", 1).map_err(|e| Error::Fact(e.to_string()))?;
    let item = Item::new(key, IssueType::Design, Vec::new());
    Ok(entries(&item)?.into_iter().map(|(key, _)| key).collect())
}

// ---------------------------------------------------------------------------------------------
// Values.

/// The OTLP value of an attribute's serde value.
fn any_value(key: &str, value: &Value) -> Result<AnyValue, Error> {
    use any_value::Value as V;
    let value = match value {
        Value::String(text) => V::StringValue(text.clone()),
        Value::Bool(flag) => V::BoolValue(*flag),
        Value::Number(number) => match (number.as_i64(), number.is_u64()) {
            (Some(int), _) => V::IntValue(int),
            (None, true) => {
                return Err(Error::Value(format!("`{key}`: {number} is above i64::MAX")));
            }
            (None, false) => V::DoubleValue(
                number
                    .as_f64()
                    .ok_or_else(|| Error::Value(format!("`{key}`: {number}")))?,
            ),
        },
        Value::Array(items) => V::ArrayValue(ArrayValue {
            values: items
                .iter()
                .map(|item| any_value(key, item))
                .collect::<Result<_, _>>()?,
        }),
        Value::Null => return Err(Error::Value(format!("`{key}` is null"))),
        Value::Object(_) => return Err(Error::Value(format!("`{key}` is a nested object"))),
    };
    Ok(AnyValue { value: Some(value) })
}

/// The serde value of an OTLP attribute value.
fn serde_value(key: &str, value: &AnyValue) -> Result<Value, Error> {
    use any_value::Value as V;
    match &value.value {
        Some(V::StringValue(text)) => Ok(Value::String(text.clone())),
        Some(V::BoolValue(flag)) => Ok(Value::Bool(*flag)),
        Some(V::IntValue(int)) => Ok(Value::from(*int)),
        Some(V::DoubleValue(double)) => Number::from_f64(*double)
            .map(Value::Number)
            .ok_or_else(|| Error::Value(format!("`{key}`: {double} is not finite"))),
        Some(V::ArrayValue(array)) => array
            .values
            .iter()
            .map(|item| serde_value(key, item))
            .collect::<Result<_, _>>()
            .map(Value::Array),
        Some(_) => Err(Error::Value(format!("`{key}` has no attribute decoding"))),
        None => Err(Error::Value(format!("`{key}` has no value"))),
    }
}

fn key_value(key: &str, value: &Value) -> Result<KeyValue, Error> {
    Ok(KeyValue {
        key: key.to_owned(),
        value: Some(any_value(key, value)?),
        ..KeyValue::default()
    })
}

fn key_values(entries: &[(String, Value)]) -> Result<Vec<KeyValue>, Error> {
    entries
        .iter()
        .map(|(key, value)| key_value(key, value))
        .collect()
}

/// The `(key, serde value)` pairs of OTLP attributes, in order.
fn read_entries(attributes: &[KeyValue]) -> Result<Entries, Error> {
    attributes
        .iter()
        .map(|kv| {
            let value = kv
                .value
                .as_ref()
                .ok_or_else(|| Error::Value(format!("`{}` has no value", kv.key)))?;
            Ok((kv.key.clone(), serde_value(&kv.key, value)?))
        })
        .collect()
}

fn strings(parts: &[String]) -> Value {
    Value::Array(parts.iter().cloned().map(Value::String).collect())
}

/// Pushes `span`'s kind and local parts under `kind_key` and `local_key`.
fn push_span_ref(out: &mut Entries, kind_key: &str, local_key: &str, span: &SpanRef) {
    out.push((kind_key.to_owned(), Value::from(span.span.as_str())));
    out.push((local_key.to_owned(), strings(&span.local)));
}

// ---------------------------------------------------------------------------------------------
// Encoding.

fn resource(origin: &Origin) -> Resource {
    let attribute = |key: &str, value: &str| KeyValue {
        key: key.to_owned(),
        value: Some(AnyValue {
            value: Some(any_value::Value::StringValue(value.to_owned())),
        }),
        ..KeyValue::default()
    };
    Resource {
        attributes: vec![
            attribute("service.name", &origin.service_name),
            attribute("service.version", &origin.service_version),
            attribute("service.instance.id", &origin.instance_id),
            attribute("host.name", &origin.host_name),
        ],
        ..Resource::default()
    }
}

fn scope() -> InstrumentationScope {
    InstrumentationScope {
        name: SCOPE_NAME.to_owned(),
        version: CATALOGUE_VERSION.to_string(),
        ..InstrumentationScope::default()
    }
}

fn traces(span: Span, origin: &Origin) -> Export {
    Export::Traces(ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(resource(origin)),
            scope_spans: vec![ScopeSpans {
                scope: Some(scope()),
                spans: vec![span],
                schema_url: SCHEMA_URL.to_owned(),
            }],
            ..ResourceSpans::default()
        }],
    })
}

fn logs(record: LogRecord, origin: &Origin) -> Export {
    Export::Logs(ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(resource(origin)),
            scope_logs: vec![ScopeLogs {
                scope: Some(scope()),
                log_records: vec![record],
                schema_url: SCHEMA_URL.to_owned(),
            }],
            ..ResourceLogs::default()
        }],
    })
}

/// The kind's span name template with `{key}` filled from `attributes`.
fn render(template: &str, attributes: &[(String, Value)]) -> Option<String> {
    let Some((head, rest)) = template.split_once('{') else {
        return Some(template.to_owned());
    };
    let (key, tail) = rest.split_once('}')?;
    let value = attributes.iter().find(|(k, _)| k == key).map(|(_, v)| v)?;
    let text = match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    Some(format!("{head}{text}{tail}"))
}

/// The span status of `fact`: set only for a stage run's known outcome.
fn status_of(fact: &Fact) -> Option<Status> {
    let Fact::StageRun(run) = fact else {
        return None;
    };
    let code = match run.attributes.outcome {
        RunOutcome::Known(OutcomeStatus::Complete | OutcomeStatus::NeedsInput) => {
            status::StatusCode::Ok
        }
        RunOutcome::Known(OutcomeStatus::Failed) => status::StatusCode::Error,
        RunOutcome::Unknown => return None,
    };
    Some(Status {
        code: code.into(),
        ..Status::default()
    })
}

fn span_id_of(key: &ItemKey, span: Option<&SpanRef>) -> SpanId {
    span.map_or_else(|| ids::root_span_id(key), |span| span.span_id(key))
}

fn link(link: &Link) -> Result<span::Link, Error> {
    let target = &link.target;
    let mut attributes = Entries::new();
    for (key, value) in entries(link)? {
        if key == TARGET {
            attributes.push((
                envelope::LINK_TO_ITEM.to_owned(),
                Value::String(target.item.to_string()),
            ));
            if let Some(span) = &target.concerns {
                push_span_ref(
                    &mut attributes,
                    envelope::LINK_TO_SPAN,
                    envelope::LINK_TO_LOCAL,
                    span,
                );
            }
        } else {
            attributes.push((key, value));
        }
    }
    Ok(span::Link {
        trace_id: ids::trace_id(&target.item).0.to_vec(),
        span_id: span_id_of(&target.item, target.concerns.as_ref())
            .0
            .to_vec(),
        attributes: key_values(&attributes)?,
        ..span::Link::default()
    })
}

/// A span's record: its envelope entries go after the item's and before the kind's.
fn span_record<A: Serialize>(
    fact: &Fact,
    item: &Item,
    envelope_entries: Entries,
    parent: Option<SpanId>,
    links: &[Link],
    (start, end): (u64, u64),
    attributes: &A,
) -> Result<Span, Error> {
    let kind = entries(attributes)?;
    let name = render(fact.kind().otlp().name, &kind)
        .ok_or_else(|| Error::Value("the span name's attribute is absent".into()))?;
    let mut all = entries(item)?;
    all.extend(envelope_entries);
    all.extend(kind);
    Ok(Span {
        trace_id: fact.trace_id().0.to_vec(),
        span_id: fact.span_id().0.to_vec(),
        parent_span_id: parent.map(|id| id.0.to_vec()).unwrap_or_default(),
        name,
        kind: span::SpanKind::Internal.into(),
        start_time_unix_nano: start,
        end_time_unix_nano: end,
        attributes: key_values(&all)?,
        links: links.iter().map(link).collect::<Result<_, _>>()?,
        status: status_of(fact),
        ..Span::default()
    })
}

fn encode_log<A: Serialize>(
    fact: &Fact,
    log: &LogFact<A>,
    origin: &Origin,
) -> Result<Export, Error> {
    let mut all = entries(&log.item)?;
    if let Some(span) = &log.concerns {
        push_span_ref(
            &mut all,
            envelope::CONCERNS_SPAN,
            envelope::CONCERNS_LOCAL,
            span,
        );
    }
    let fact_id = fact
        .fact_id()
        .ok_or_else(|| Error::Fact("a log fact has a fact id".into()))?;
    all.push((
        envelope::FACT_ID.to_owned(),
        Value::String(fact_id.to_string()),
    ));
    all.extend(entries(&log.attributes)?);
    let record = LogRecord {
        time_unix_nano: log.time,
        event_name: fact.kind().otlp().name.to_owned(),
        trace_id: fact.trace_id().0.to_vec(),
        span_id: fact.span_id().0.to_vec(),
        attributes: key_values(&all)?,
        ..LogRecord::default()
    };
    Ok(logs(record, origin))
}

fn encode_span<A: Serialize>(
    fact: &Fact,
    span: &SpanFact<A>,
    origin: &Origin,
) -> Result<Export, Error> {
    let key = span.item.key();
    let parent = span.concerns.parent.as_ref();
    let mut envelope_entries = vec![(
        envelope::SPAN_LOCAL.to_owned(),
        strings(&span.concerns.local),
    )];
    if let Some(parent) = parent {
        push_span_ref(
            &mut envelope_entries,
            envelope::SPAN_PARENT_SPAN,
            envelope::SPAN_PARENT_LOCAL,
            parent,
        );
    }
    let record = span_record(
        fact,
        &span.item,
        envelope_entries,
        Some(span_id_of(key, parent)),
        &span.links,
        (span.start, span.end),
        &span.attributes,
    )?;
    Ok(traces(record, origin))
}

fn encode_root<A: Serialize>(
    fact: &Fact,
    root: &RootFact<A>,
    origin: &Origin,
) -> Result<Export, Error> {
    let record = span_record(
        fact,
        &root.item,
        Entries::new(),
        None,
        &root.links,
        (root.start, root.end),
        &root.attributes,
    )?;
    Ok(traces(record, origin))
}

pub(super) fn encode(fact: &Fact, origin: &Origin) -> Result<Export, Error> {
    match fact {
        Fact::ItemRoot(f) => encode_root(fact, f, origin),
        Fact::StageRunStarted(f) => encode_log(fact, f, origin),
        Fact::StageRun(f) => encode_span(fact, f, origin),
        Fact::Turn(f) => encode_span(fact, f, origin),
        Fact::ToolCall(f) => encode_span(fact, f, origin),
        Fact::Phase(f) => encode_span(fact, f, origin),
        Fact::WaitStarted(f) => encode_log(fact, f, origin),
        Fact::Wait(f) => encode_span(fact, f, origin),
        Fact::ConfidenceScored(f) => encode_log(fact, f, origin),
        Fact::DecisionRequested(f) => encode_log(fact, f, origin),
        Fact::DecisionTaken(f) => encode_log(fact, f, origin),
        Fact::Intervention(f) => encode_log(fact, f, origin),
        Fact::PickedUp(f) => encode_log(fact, f, origin),
        Fact::StateChanged(f) => encode_log(fact, f, origin),
        Fact::LinkChanged(f) => encode_log(fact, f, origin),
        Fact::PrOpened(f) => encode_log(fact, f, origin),
        Fact::PrMerged(f) => encode_log(fact, f, origin),
        Fact::PrReverted(f) => encode_log(fact, f, origin),
        Fact::RegressionLinked(f) => encode_log(fact, f, origin),
    }
}

// ---------------------------------------------------------------------------------------------
// Decoding.

/// Exactly one element of `items`, named `what` in the refusal.
fn one<'a, T>(items: &'a [T], what: &str) -> Result<&'a T, Error> {
    match items {
        [item] => Ok(item),
        _ => Err(Error::Shape(format!(
            "{} {what} where exactly one is expected",
            items.len()
        ))),
    }
}

fn check_scope(scope: Option<&InstrumentationScope>) -> Result<(), Error> {
    let scope = scope.ok_or_else(|| Error::Shape("no scope".into()))?;
    if scope.name != SCOPE_NAME {
        return Err(Error::Shape(format!("scope `{}`", scope.name)));
    }
    if scope.version != CATALOGUE_VERSION.to_string() {
        return Err(Error::Shape(format!("scope version `{}`", scope.version)));
    }
    Ok(())
}

/// A record's attributes, split into the parts of the serde form.
struct Split {
    item: Map<String, Value>,
    envelope: Map<String, Value>,
    fact_id: Option<Value>,
    rest: Entries,
}

fn split(attributes: &[KeyValue]) -> Result<Split, Error> {
    let item_keys = item_keys()?;
    let mut out = Split {
        item: Map::new(),
        envelope: Map::new(),
        fact_id: None,
        rest: Entries::new(),
    };
    for (key, value) in read_entries(attributes)? {
        if item_keys.contains(&key) {
            out.item.insert(key, value);
        } else if envelope::KEYS.contains(&key.as_str()) {
            out.envelope.insert(key, value);
        } else if key == envelope::FACT_ID && out.fact_id.is_none() {
            out.fact_id = Some(value);
        } else {
            out.rest.push((key, value));
        }
    }
    Ok(out)
}

/// The `SpanRef` serde form of `kind_key` and `local_key` in `map`, both or neither.
fn take_span_ref(
    map: &mut Map<String, Value>,
    kind_key: &str,
    local_key: &str,
) -> Result<Option<Value>, Error> {
    match (map.remove(kind_key), map.remove(local_key)) {
        (None, None) => Ok(None),
        (Some(span), Some(local)) => {
            let mut object = Map::new();
            object.insert("span".into(), span);
            object.insert("local".into(), local);
            Ok(Some(Value::Object(object)))
        }
        _ => Err(Error::Fact(format!(
            "`{kind_key}` and `{local_key}` come together"
        ))),
    }
}

fn object(entries: Entries) -> Map<String, Value> {
    entries.into_iter().collect()
}

/// The serde form of an OTLP link.
fn read_link(link: &span::Link) -> Result<Value, Error> {
    let mut target = Map::new();
    let mut attributes = Map::new();
    let mut envelope_part = Map::new();
    for (key, value) in read_entries(&link.attributes)? {
        if envelope::KEYS.contains(&key.as_str()) {
            envelope_part.insert(key, value);
        } else {
            attributes.insert(key, value);
        }
    }
    let item = envelope_part
        .remove(envelope::LINK_TO_ITEM)
        .ok_or_else(|| Error::Fact(format!("a link has no `{}`", envelope::LINK_TO_ITEM)))?;
    target.insert("item".into(), item);
    if let Some(concerns) = take_span_ref(
        &mut envelope_part,
        envelope::LINK_TO_SPAN,
        envelope::LINK_TO_LOCAL,
    )? {
        target.insert("concerns".into(), concerns);
    }
    if let Some(key) = envelope_part.keys().next() {
        return Err(Error::Fact(format!("`{key}` is not a link key")));
    }
    attributes.insert(TARGET.into(), Value::Object(target));
    Ok(Value::Object(attributes))
}

fn fact_from(form: Map<String, Value>) -> Result<Fact, Error> {
    serde_json::from_value(Value::Object(form)).map_err(|e| Error::Fact(e.to_string()))
}

fn read_span(span: &Span) -> Result<Fact, Error> {
    let Split {
        item,
        mut envelope,
        fact_id,
        rest,
    } = split(&span.attributes)?;
    if fact_id.is_some() {
        return Err(Error::Fact(format!(
            "a span carries no `{}`",
            envelope::FACT_ID
        )));
    }
    let kinds: Vec<FactKind> = FactKind::ALL
        .iter()
        .copied()
        .filter(|kind| kind.otlp().signal == Signal::Span)
        .filter(|kind| render(kind.otlp().name, &rest).as_deref() == Some(span.name.as_str()))
        .collect();
    let kind = match kinds.as_slice() {
        [kind] => *kind,
        _ => {
            return Err(Error::Fact(format!(
                "no single kind for span `{}`",
                span.name
            )));
        }
    };
    let mut form = Map::new();
    form.insert("kind".into(), Value::from(kind.tag()));
    form.insert("item".into(), Value::Object(item));
    form.insert("start".into(), Value::from(span.start_time_unix_nano));
    form.insert("end".into(), Value::from(span.end_time_unix_nano));
    if let Some(local) = envelope.remove(envelope::SPAN_LOCAL) {
        let mut concerns = Map::new();
        concerns.insert("local".into(), local);
        if let Some(parent) = take_span_ref(
            &mut envelope,
            envelope::SPAN_PARENT_SPAN,
            envelope::SPAN_PARENT_LOCAL,
        )? {
            concerns.insert("parent".into(), parent);
        }
        form.insert("concerns".into(), Value::Object(concerns));
    }
    if let Some(key) = envelope.keys().next() {
        return Err(Error::Fact(format!("`{key}` is not a span key")));
    }
    if !span.links.is_empty() {
        let links = span.links.iter().map(read_link).collect::<Result<_, _>>()?;
        form.insert("links".into(), Value::Array(links));
    }
    form.insert("attributes".into(), Value::Object(object(rest)));
    fact_from(form)
}

fn read_log(record: &LogRecord) -> Result<(Fact, Option<Value>), Error> {
    let Split {
        item,
        mut envelope,
        fact_id,
        rest,
    } = split(&record.attributes)?;
    let kind = FactKind::ALL
        .iter()
        .copied()
        .find(|kind| kind.otlp().signal == Signal::Log && kind.otlp().name == record.event_name)
        .ok_or_else(|| Error::Fact(format!("unknown event `{}`", record.event_name)))?;
    let mut form = Map::new();
    form.insert("kind".into(), Value::from(kind.tag()));
    form.insert("item".into(), Value::Object(item));
    form.insert("time".into(), Value::from(record.time_unix_nano));
    if let Some(concerns) = take_span_ref(
        &mut envelope,
        envelope::CONCERNS_SPAN,
        envelope::CONCERNS_LOCAL,
    )? {
        form.insert("concerns".into(), concerns);
    }
    if let Some(key) = envelope.keys().next() {
        return Err(Error::Fact(format!("`{key}` is not a log key")));
    }
    form.insert("attributes".into(), Value::Object(object(rest)));
    Ok((fact_from(form)?, fact_id))
}

/// Refuses `actual` unless it equals the id `expected` derives.
fn same_id(what: &str, actual: &[u8], expected: &[u8]) -> Result<(), Error> {
    if actual == expected {
        Ok(())
    } else {
        Err(Error::Ids(format!("`{what}`")))
    }
}

/// Checks the ids of a decoded span against those `expected`, its re-encoding, derives.
fn check_span_ids(actual: &Span, expected: &Span) -> Result<(), Error> {
    same_id("traceId", &actual.trace_id, &expected.trace_id)?;
    same_id("spanId", &actual.span_id, &expected.span_id)?;
    same_id(
        "parentSpanId",
        &actual.parent_span_id,
        &expected.parent_span_id,
    )?;
    if actual.links.len() != expected.links.len() {
        return Err(Error::Ids("links".into()));
    }
    for (actual, expected) in actual.links.iter().zip(&expected.links) {
        same_id("links.traceId", &actual.trace_id, &expected.trace_id)?;
        same_id("links.spanId", &actual.span_id, &expected.span_id)?;
    }
    Ok(())
}

pub(super) fn decode(export: &Export) -> Result<Fact, Error> {
    let origin = Origin::default();
    match export {
        Export::Traces(request) => {
            let resource = one(&request.resource_spans, "resources")?;
            let scope = one(&resource.scope_spans, "scopes")?;
            check_scope(scope.scope.as_ref())?;
            let span = one(&scope.spans, "records")?;
            if span
                .status
                .as_ref()
                .is_some_and(|s| *s == Status::default())
            {
                return Err(Error::Shape("an empty status".into()));
            }
            let fact = read_span(span)?;
            let Export::Traces(mut expected) = encode(&fact, &origin)? else {
                return Err(Error::Differs);
            };
            if let Some(again) = expected
                .resource_spans
                .first()
                .and_then(|r| r.scope_spans.first())
                .and_then(|s| s.spans.first())
            {
                check_span_ids(span, again)?;
            }
            if let Some(first) = expected.resource_spans.first_mut() {
                first.resource.clone_from(&resource.resource);
            }
            if expected == *request {
                Ok(fact)
            } else {
                Err(Error::Differs)
            }
        }
        Export::Logs(request) => {
            let resource = one(&request.resource_logs, "resources")?;
            let scope = one(&resource.scope_logs, "scopes")?;
            check_scope(scope.scope.as_ref())?;
            let record = one(&scope.log_records, "records")?;
            let (fact, fact_id) = read_log(record)?;
            let derived = fact.fact_id().map(|id| Value::String(id.to_string()));
            if fact_id != derived {
                return Err(Error::Ids(format!("`{}`", envelope::FACT_ID)));
            }
            same_id("traceId", &record.trace_id, &fact.trace_id().0)?;
            same_id("spanId", &record.span_id, &fact.span_id().0)?;
            let Export::Logs(mut expected) = encode(&fact, &origin)? else {
                return Err(Error::Differs);
            };
            if let Some(first) = expected.resource_logs.first_mut() {
                first.resource.clone_from(&resource.resource);
            }
            if expected == *request {
                Ok(fact)
            } else {
                Err(Error::Differs)
            }
        }
    }
}
