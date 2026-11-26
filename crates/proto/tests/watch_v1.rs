//! The `mintswarm.watch.v1` code: its messages carry the design's fields, the
//! tonic client and server are exported, a `WatchEvent` round-trips through
//! prost with a known or an unknown body, and the protos' comments state the
//! cursor and id rules.

#![cfg(test)]

use std::collections::HashMap;

use mintswarm_proto::watch::v1::{
    Actor, ActorClass, ActorType, AttributeMatch, Forwarding, GapReason, GetServerInfoRequest,
    GetServerInfoResponse, GitHubWebhook, ItemKind, ItemRef, ListWatchEventsRequest,
    ListWatchEventsResponse, Mark, MarkKind, Party, PartyMatch, Retention, StartAt, Subject,
    SubscribeRequest, SubscribeResponse, UntrustedText, Verdict, WatchEvent, WatchFilter,
    subscribe_request, watch_event, watch_service_client, watch_service_server,
};
use prost::Message;

const WATCH: &str = include_str!("../../../api/proto/mintswarm/watch/v1/watch.proto");
const GITHUB: &str = include_str!("../../../api/proto/mintswarm/watch/v1/github.proto");

fn untrusted(text: &str) -> UntrustedText {
    UntrustedText {
        text: text.to_owned(),
        truncated: false,
        original_length: u32::try_from(text.len()).expect("short text"),
    }
}

fn timestamp(seconds: i64) -> mintswarm_proto::prost_types::Timestamp {
    mintswarm_proto::prost_types::Timestamp { seconds, nanos: 0 }
}

/// An event with every envelope field set and no body.
fn envelope() -> WatchEvent {
    WatchEvent {
        cursor: "0193a1b2-0000-7000-8000-000000000001.0193a1b2-0000-7000-8000-000000000002"
            .to_owned(),
        id: "0193a1b2-0000-7000-8000-000000000002".to_owned(),
        at: Some(timestamp(1_800_000_000)),
        kind: "github.webhook".to_owned(),
        subject: Some(Subject {
            repository: "mintswarm/mintswarm".to_owned(),
            item_kind: ItemKind::PullRequest.into(),
            number: 484,
            url: "https://github.com/mintswarm/mintswarm/pull/484".to_owned(),
            job_id: String::new(),
            title: Some(untrusted("feat(api): mintswarm.watch.v1")),
        }),
        actor: Some(Actor {
            login: "maintainer".to_owned(),
            id: "42".to_owned(),
            r#type: ActorType::User.into(),
            class: ActorClass::Maintainer.into(),
        }),
        parties: vec![Party {
            login: "maintainer".to_owned(),
            role: "author".to_owned(),
        }],
        attributes: HashMap::from([
            ("event".to_owned(), "issue_comment".to_owned()),
            ("action".to_owned(), "created".to_owned()),
        ]),
        summary: "issue_comment created on mintswarm/mintswarm#484 by maintainer".to_owned(),
        forwarding: Some(Forwarding {
            verdict: Verdict::Pass.into(),
            rule: "maintainer".to_owned(),
            opt_in: String::new(),
        }),
        body: None,
    }
}

fn github_webhook() -> GitHubWebhook {
    GitHubWebhook {
        delivery: "72d3162e-cc78-11e3-81ab-4c9367dc0958".to_owned(),
        event: "issue_comment".to_owned(),
        action: "created".to_owned(),
        body: Some(untrusted("Looks good.")),
        detail: HashMap::from([("label".to_owned(), untrusted("mintswarm"))]),
        payload: b"{\"action\":\"created\"}".to_vec(),
    }
}

/// Names the generated tonic client, server and service trait from outside
/// the crate, so they are proven exported.
#[test]
fn client_and_server_are_exported() {
    let _ =
        std::mem::size_of::<watch_service_client::WatchServiceClient<tonic::transport::Channel>>();
    let server = watch_service_server::WatchServiceServer::new(Unimplemented);
    let _ = server.max_decoding_message_size(2 * 1024 * 1024);
}

/// Calls the `Subscribe` stub with a single request and binds its answer to a
/// response stream, so the generated client is proven server-streaming. Never
/// run: it only has to compile.
#[expect(
    dead_code,
    reason = "compile-only proof that the Subscribe client stub is server-streaming"
)]
async fn subscribe_is_server_streaming(
    client: &mut watch_service_client::WatchServiceClient<tonic::transport::Channel>,
) {
    let _: Result<tonic::Response<tonic::Streaming<SubscribeResponse>>, tonic::Status> =
        client.subscribe(SubscribeRequest::default()).await;
}

/// A server that answers nothing. It implements exactly the three RPCs, so a
/// missing or extra RPC fails to compile, and `subscribe` takes a single
/// request, not a stream.
struct Unimplemented;

#[tonic::async_trait]
impl watch_service_server::WatchService for Unimplemented {
    async fn get_server_info(
        &self,
        _: tonic::Request<GetServerInfoRequest>,
    ) -> Result<tonic::Response<GetServerInfoResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("get_server_info"))
    }

    type SubscribeStream = tonic::codegen::BoxStream<SubscribeResponse>;

    async fn subscribe(
        &self,
        _: tonic::Request<SubscribeRequest>,
    ) -> Result<tonic::Response<Self::SubscribeStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("subscribe"))
    }

    async fn list_watch_events(
        &self,
        _: tonic::Request<ListWatchEventsRequest>,
    ) -> Result<tonic::Response<ListWatchEventsResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("list_watch_events"))
    }
}

#[test]
fn watch_event_with_a_github_body_round_trips() {
    let event = WatchEvent {
        body: Some(watch_event::Body::GithubWebhook(github_webhook())),
        ..envelope()
    };
    let bytes = event.encode_to_vec();
    // Field 20, length-delimited: the varint of (20 << 3) | 2 = 162.
    assert!(
        bytes.windows(2).any(|pair| pair == [0xA2, 0x01]),
        "the body is encoded as field 20"
    );
    let back = WatchEvent::decode(bytes.as_slice()).expect("decodes");
    assert_eq!(back, event);
}

#[test]
fn body_has_only_the_github_webhook_case() {
    let body = watch_event::Body::GithubWebhook(github_webhook());
    match body {
        watch_event::Body::GithubWebhook(webhook) => assert_eq!(webhook.event, "issue_comment"),
    }
}

#[test]
fn github_webhook_has_exactly_the_design_fields() {
    let GitHubWebhook {
        delivery,
        event,
        action,
        body,
        detail,
        payload,
    } = github_webhook();
    let _: String = delivery;
    let _: String = event;
    let _: String = action;
    let _: Option<UntrustedText> = body;
    let _: HashMap<String, UntrustedText> = detail;
    let _: Vec<u8> = payload;
}

#[test]
fn filter_and_server_info_carry_the_design_fields() {
    let filter = WatchFilter {
        kinds: vec!["github.*".to_owned()],
        repositories: vec!["mintswarm/mintswarm".to_owned()],
        items: vec![ItemRef {
            repository: "mintswarm/mintswarm".to_owned(),
            number: 484,
        }],
        actors: vec!["maintainer".to_owned()],
        actor_classes: vec![ActorClass::Maintainer.into()],
        parties: vec![PartyMatch {
            login: "maintainer".to_owned(),
            roles: vec!["requested_reviewer".to_owned()],
        }],
        attributes: vec![AttributeMatch {
            key: "event".to_owned(),
            values: vec!["issue_comment".to_owned()],
        }],
        include_opt_in: vec!["bots".to_owned()],
        include_dropped: false,
    };
    let subscribe = SubscribeRequest {
        filter: Some(filter.clone()),
        start: Some(subscribe_request::Start::StartAt(StartAt::Oldest.into())),
        heartbeat_seconds: 30,
        include_payload: false,
    };
    let resumed = SubscribeRequest {
        start: Some(subscribe_request::Start::After(envelope().cursor)),
        ..subscribe.clone()
    };
    assert_ne!(subscribe, resumed);

    let gap = Mark {
        kind: MarkKind::Gap.into(),
        cursor: String::new(),
        gap_reason: GapReason::CursorExpired.into(),
        pruned_before: Some(timestamp(1_799_000_000)),
    };
    let page = ListWatchEventsResponse {
        events: vec![envelope()],
        next_cursor: envelope().cursor,
        head_cursor: envelope().cursor,
        gap: Some(gap),
    };
    let request = ListWatchEventsRequest {
        filter: Some(filter),
        after: String::new(),
        limit: 500,
        include_payload: false,
    };
    assert_eq!(page.events.len(), 1);
    assert_eq!(request.limit, 500);

    let info = GetServerInfoResponse {
        store_epoch: "0193a1b2-0000-7000-8000-000000000001".to_owned(),
        kinds: vec!["github.webhook".to_owned()],
        retention: Some(Retention {
            max_age: Some(mintswarm_proto::prost_types::Duration {
                seconds: 7 * 24 * 60 * 60,
                nanos: 0,
            }),
            max_events: 10_000,
            oldest_cursor: String::new(),
        }),
        protocol_minor: 0,
        server_version: "0.1.0".to_owned(),
    };
    assert!(envelope().cursor.starts_with(&info.store_epoch));
}

#[test]
fn every_enum_has_a_prefixed_zero_value() {
    let zeros = [
        (
            ItemKind::Unspecified as i32,
            ItemKind::Unspecified.as_str_name(),
        ),
        (
            ActorType::Unspecified as i32,
            ActorType::Unspecified.as_str_name(),
        ),
        (
            ActorClass::Unspecified as i32,
            ActorClass::Unspecified.as_str_name(),
        ),
        (
            Verdict::Unspecified as i32,
            Verdict::Unspecified.as_str_name(),
        ),
        (
            StartAt::Unspecified as i32,
            StartAt::Unspecified.as_str_name(),
        ),
        (
            MarkKind::Unspecified as i32,
            MarkKind::Unspecified.as_str_name(),
        ),
        (
            GapReason::Unspecified as i32,
            GapReason::Unspecified.as_str_name(),
        ),
    ];
    let expected = [
        "ITEM_KIND_UNSPECIFIED",
        "ACTOR_TYPE_UNSPECIFIED",
        "ACTOR_CLASS_UNSPECIFIED",
        "VERDICT_UNSPECIFIED",
        "START_AT_UNSPECIFIED",
        "MARK_KIND_UNSPECIFIED",
        "GAP_REASON_UNSPECIFIED",
    ];
    for ((value, name), want) in zeros.into_iter().zip(expected) {
        assert_eq!(value, 0, "{want}");
        assert_eq!(name, want);
    }
    // The package declares exactly these seven enums.
    let declared = WATCH
        .lines()
        .chain(GITHUB.lines())
        .filter(|line| line.trim_start().starts_with("enum "))
        .count();
    assert_eq!(declared, expected.len());
}

#[test]
fn gap_reason_has_the_three_reasons() {
    let names: Vec<&str> = (0..=3)
        .map(|i| GapReason::try_from(i).expect("a gap reason").as_str_name())
        .collect();
    assert_eq!(
        names,
        [
            "GAP_REASON_UNSPECIFIED",
            "GAP_REASON_CURSOR_EXPIRED",
            "GAP_REASON_STORE_CHANGED",
            "GAP_REASON_UNKNOWN",
        ]
    );
    assert!(GapReason::try_from(4).is_err());
}

#[test]
fn unknown_body_field_decodes_with_body_unset() {
    let event = envelope();
    let mut bytes = event.encode_to_vec();
    // Field 21, length-delimited: the varint of (21 << 3) | 2 = 170.
    bytes.extend_from_slice(&[0xAA, 0x01, 0x03, b'n', b'e', b'w']);
    let back = WatchEvent::decode(bytes.as_slice()).expect("decodes");
    assert_eq!(back.body, None);
    assert_eq!(back, event);
}

/// The text from the line whose trimmed form is `header` to its matching
/// closing brace.
fn block<'a>(src: &'a str, header: &str) -> &'a str {
    let mut starts = src
        .match_indices('\n')
        .map(|(i, _)| i + 1)
        .chain(std::iter::once(0))
        .filter(|&i| src[i..].lines().next().is_some_and(|l| l.trim() == header));
    let start = starts
        .next()
        .unwrap_or_else(|| panic!("no line `{header}`"));
    assert!(starts.next().is_none(), "`{header}` appears twice");
    let mut depth = 0_usize;
    for (offset, c) in src[start..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &src[start..=start + offset];
                }
            }
            _ => {}
        }
    }
    panic!("`{header}` is not closed");
}

/// The `//` comment directly above the one line whose trimmed form starts
/// with `line_start`, its lines stripped and joined with single spaces.
fn leading_comment(text: &str, line_start: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut found = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.trim_start().starts_with(line_start));
    let (index, _) = found
        .next()
        .unwrap_or_else(|| panic!("no line starts with `{line_start}`"));
    assert!(found.next().is_none(), "`{line_start}` appears twice");
    let comment: Vec<&str> = lines[..index]
        .iter()
        .rev()
        .map(|l| l.trim())
        .take_while(|l| l.starts_with("//"))
        .map(|l| l.trim_start_matches("//").trim())
        .collect();
    assert!(!comment.is_empty(), "`{line_start}` has no comment");
    comment
        .into_iter()
        .rev()
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn assert_says(comment: &str, phrase: &str) {
    assert!(
        comment.contains(phrase),
        "the comment does not say `{phrase}`: {comment}"
    );
}

#[test]
fn cursor_and_id_are_documented() {
    let event = block(WATCH, "message WatchEvent {");
    let cursor = leading_comment(event, "string cursor = 1;");
    assert_says(&cursor, "`<store_epoch>.<id>`");
    assert_says(&cursor, "opaque to clients");
    let id = leading_comment(event, "string id = 2;");
    assert_says(&id, "UUIDv7");
    assert_says(&id, "de-duplicate");
    assert_says(&id, "commit order within one store epoch");
}

#[test]
fn cursor_rules_are_documented() {
    let reasons = block(WATCH, "enum GapReason {");
    assert_says(
        &leading_comment(reasons, "GAP_REASON_STORE_CHANGED"),
        "cursor from another store epoch",
    );
    assert_says(
        &leading_comment(reasons, "GAP_REASON_CURSOR_EXPIRED"),
        "id is strictly below `pruned_through`",
    );
    let mark = leading_comment(WATCH, "message Mark {");
    assert_says(&mark, "at or above `pruned_through` resumes with no gap");
    assert_says(&mark, "even when its own event was pruned");
}

#[test]
fn clamped_limits_are_documented() {
    let subscribe = block(WATCH, "message SubscribeRequest {");
    let heartbeat = leading_comment(subscribe, "uint32 heartbeat_seconds = 4;");
    assert_says(&heartbeat, "zero means the default of 30");
    assert_says(&heartbeat, "clamped, not refused");
    assert_says(&heartbeat, "a value from 1 to 4 is raised to 5");
    let list = block(WATCH, "message ListWatchEventsRequest {");
    let limit = leading_comment(list, "uint32 limit = 3;");
    assert_says(&limit, "clamped, not refused");
    assert_says(&limit, "zero means the default of 100");
    assert_says(&limit, "a value above 500 is capped at 500");
}

#[test]
fn no_seq_in_the_package() {
    for src in [WATCH, GITHUB] {
        assert!(
            !src.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .any(|token| token == "seq"),
            "a proto of the package mentions `seq`"
        );
    }
}

#[test]
fn one_way_import() {
    assert!(
        !GITHUB.lines().any(|l| l.starts_with("import")),
        "github.proto imports nothing"
    );
    assert!(
        WATCH
            .lines()
            .any(|l| l == "import \"mintswarm/watch/v1/github.proto\";"),
        "watch.proto imports github.proto"
    );
    assert!(GITHUB.contains("message UntrustedText {"));
    assert!(!WATCH.contains("message UntrustedText {"));
}
