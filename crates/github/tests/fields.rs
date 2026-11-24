//! Field and label reads and writes, and the field catalogue, against a scripted local server
//! answering with hand-written fixtures in the shape of GitHub's issue-fields API.
// Not redundant: clippy's `allow-unwrap-in-tests` covers a `tests/` crate only under `cfg(test)`.
#![cfg(test)]

mod support;

use mintswarm_domain::field::{
    FieldCatalogue, FieldError, FieldFinding, FieldRead, FieldRef, FieldSource, FieldValue,
};
use mintswarm_domain::ids::IssueNumber;
use mintswarm_github::{FieldClient, GitHubFieldCatalogue, Token};
use support::{Request, Responder};

const TOKEN: &str = "ghs_f13ldT0kenV4lue";
const VALUES: &str = include_str!("fixtures/fields/issue-field-values.json");
const RECREATED: &str = include_str!("fixtures/fields/field-recreated.json");
const OPTION_MISSING: &str = include_str!("fixtures/fields/option-missing.json");
const LABELS_CONFLICT: &str = include_str!("fixtures/fields/issue-labels-conflict.json");
const ORG_FIELD: &str = include_str!("fixtures/fields/org-field.json");
const VALUES_PATH: &str = "/repos/acme/widgets/issues/7/issue-field-values";
const ISSUE: IssueNumber = IssueNumber(7);

fn token() -> Token {
    Token::new(TOKEN.to_owned(), 0)
}

fn names(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn size() -> FieldRef {
    FieldRef {
        source: FieldSource::Field {
            name: "Effort".into(),
            field_id: 101,
        },
        options: names(&["XS", "S", "M", "L"]),
    }
}

fn priority() -> FieldRef {
    FieldRef {
        source: FieldSource::Field {
            name: "Priority".into(),
            field_id: 202,
        },
        options: names(&["P0", "P1", "P2"]),
    }
}

fn effort_labels() -> FieldRef {
    FieldRef {
        source: FieldSource::Labels {
            label: "effort:{option}".into(),
        },
        options: names(&["S", "M", "L"]),
    }
}

fn client(responder: &Responder) -> FieldClient {
    FieldClient::new(&responder.base, "acme", "widgets").unwrap()
}

fn value(field_id: Option<u64>, option_id: Option<u64>, name: &str) -> FieldValue {
    FieldValue {
        field_id,
        option_id,
        name: name.into(),
    }
}

fn body(request: &Request) -> serde_json::Value {
    serde_json::from_str(&request.body).unwrap()
}

/// No request shows the token outside its `authorization` header.
fn assert_token_only_in_authorization(requests: &[Request]) {
    for request in requests {
        assert_eq!(
            request.header("authorization"),
            Some(format!("Bearer {TOKEN}").as_str())
        );
        assert!(!request.path.contains(TOKEN));
        assert!(!request.body.contains(TOKEN));
        for (name, value) in &request.headers {
            assert!(name == "authorization" || !value.contains(TOKEN), "{name}");
        }
    }
}

#[tokio::test]
async fn reads_an_effort_value() {
    let responder = Responder::start().await;
    responder.reply(200, VALUES);
    let client = client(&responder);
    let read = client.pass(&token()).read_field(ISSUE, &size()).await;
    assert_eq!(
        read,
        Ok(FieldRead::Value(value(Some(101), Some(1003), "M")))
    );
    let requests = responder.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path, VALUES_PATH);
    assert_eq!(
        requests[0].header("accept"),
        Some("application/vnd.github+json")
    );
    assert_token_only_in_authorization(&requests);
}

#[tokio::test]
async fn a_same_named_field_with_another_id_is_recreated() {
    let responder = Responder::start().await;
    responder.reply(200, RECREATED);
    let client = client(&responder);
    let error = client
        .pass(&token())
        .read_field(ISSUE, &size())
        .await
        .unwrap_err();
    assert_eq!(error, FieldError::Field(FieldFinding::FieldRecreated));
    assert!(error.should_resolve_again());
}

#[tokio::test]
async fn an_effort_value_the_map_does_not_list_is_unmapped() {
    let responder = Responder::start().await;
    responder.reply(200, VALUES);
    let mut field = size();
    field.options = names(&["Low", "High"]);
    let read = client(&responder)
        .pass(&token())
        .read_field(ISSUE, &field)
        .await;
    assert_eq!(read, Err(FieldError::Field(FieldFinding::OptionUnmapped)));
}

#[tokio::test]
async fn an_issue_without_the_field_is_unset() {
    let responder = Responder::start().await;
    responder.reply(200, "[]");
    let read = client(&responder)
        .pass(&token())
        .read_field(ISSUE, &size())
        .await;
    assert_eq!(read, Ok(FieldRead::Unset));
}

#[tokio::test]
async fn size_and_priority_share_one_request() {
    let responder = Responder::start().await;
    responder.reply(200, VALUES);
    let client = client(&responder);
    let pass = client.pass(&token());
    assert_eq!(
        pass.read_field(ISSUE, &size()).await,
        Ok(FieldRead::Value(value(Some(101), Some(1003), "M")))
    );
    assert_eq!(
        pass.read_field(ISSUE, &priority()).await,
        Ok(FieldRead::Value(value(Some(202), Some(2002), "P1")))
    );
    assert_eq!(responder.requests().len(), 1);
}

#[tokio::test]
async fn a_second_pass_revalidates_with_the_etag() {
    let responder = Responder::start().await;
    responder.reply_with(200, &[("ETag", "\"v1\"")], VALUES);
    responder.reply_with(304, &[("ETag", "\"v1\"")], "");
    let client = client(&responder);
    client
        .pass(&token())
        .read_field(ISSUE, &size())
        .await
        .unwrap();
    let read = client.pass(&token()).read_field(ISSUE, &priority()).await;
    assert_eq!(
        read,
        Ok(FieldRead::Value(value(Some(202), Some(2002), "P1")))
    );
    let requests = responder.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].header("if-none-match"), None);
    assert_eq!(requests[1].header("if-none-match"), Some("\"v1\""));
}

#[tokio::test]
async fn writes_an_option_by_name() {
    let responder = Responder::start().await;
    responder.reply(200, VALUES);
    let client = client(&responder);
    client
        .pass(&token())
        .set_field(ISSUE, &size(), Some("M"))
        .await
        .unwrap();
    let requests = responder.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, VALUES_PATH);
    assert_eq!(
        body(&requests[0]),
        serde_json::json!({"issue_field_values": [{"field_id": 101, "value": "M"}]})
    );
    assert_token_only_in_authorization(&requests);
}

#[tokio::test]
async fn a_write_answered_with_another_value_is_rejected() {
    let responder = Responder::start().await;
    responder.reply(200, VALUES);
    let error = client(&responder)
        .pass(&token())
        .set_field(ISSUE, &size(), Some("L"))
        .await
        .unwrap_err();
    assert_eq!(error, FieldError::Field(FieldFinding::WriteRejected));
    assert!(error.should_resolve_again());
}

#[tokio::test]
async fn a_write_answered_under_another_field_id_is_recreated() {
    let responder = Responder::start().await;
    responder.reply(200, RECREATED);
    let error = client(&responder)
        .pass(&token())
        .set_field(ISSUE, &size(), Some("M"))
        .await
        .unwrap_err();
    assert_eq!(error, FieldError::Field(FieldFinding::FieldRecreated));
    assert!(error.should_resolve_again());
}

#[tokio::test]
async fn an_unmapped_option_is_refused_before_any_request() {
    let responder = Responder::start().await;
    let error = client(&responder)
        .pass(&token())
        .set_field(ISSUE, &size(), Some("XXL"))
        .await
        .unwrap_err();
    assert_eq!(error, FieldError::Field(FieldFinding::OptionUnmapped));
    assert!(responder.requests().is_empty());
}

#[tokio::test]
async fn an_unknown_option_on_write_is_option_missing() {
    let responder = Responder::start().await;
    responder.reply(422, OPTION_MISSING);
    let error = client(&responder)
        .pass(&token())
        .set_field(ISSUE, &size(), Some("L"))
        .await
        .unwrap_err();
    assert_eq!(error, FieldError::Field(FieldFinding::OptionMissing));
    assert!(!error.to_string().contains(TOKEN));
}

#[tokio::test]
async fn a_forbidden_write_without_rate_limit_headers_is_rejected() {
    let responder = Responder::start().await;
    responder.reply(
        403,
        r#"{"message":"Resource not accessible by integration"}"#,
    );
    let error = client(&responder)
        .pass(&token())
        .set_field(ISSUE, &size(), Some("M"))
        .await
        .unwrap_err();
    assert_eq!(error, FieldError::Field(FieldFinding::WriteRejected));
    assert!(error.should_resolve_again());
}

#[tokio::test]
async fn bad_gateway_and_rate_limits_are_transport() {
    let responder = Responder::start().await;
    responder.reply(502, "<html>Bad Gateway</html>");
    responder.reply_with(
        403,
        &[("x-ratelimit-remaining", "0")],
        r#"{"message":"API rate limit exceeded"}"#,
    );
    responder.reply_with(429, &[("retry-after", "60")], "{}");
    responder.reply(502, "");
    let client = client(&responder);
    for _ in 0..3 {
        let error = client
            .pass(&token())
            .set_field(ISSUE, &size(), Some("M"))
            .await
            .unwrap_err();
        assert!(matches!(error, FieldError::Transport(_)), "{error}");
        assert!(!error.should_resolve_again());
        assert!(!error.to_string().contains(TOKEN));
    }
    let error = client
        .pass(&token())
        .read_field(ISSUE, &size())
        .await
        .unwrap_err();
    assert!(matches!(error, FieldError::Transport(_)), "{error}");
}

#[tokio::test]
async fn an_unrelated_not_found_is_transport() {
    let responder = Responder::start().await;
    responder.reply(404, r#"{"message":"Not Found"}"#);
    let error = client(&responder)
        .pass(&token())
        .read_field(ISSUE, &size())
        .await
        .unwrap_err();
    assert_eq!(
        error,
        FieldError::Transport(format!("GET {VALUES_PATH}: 404 Not Found"))
    );
}

#[tokio::test]
async fn a_clear_deletes_the_field_value() {
    let responder = Responder::start().await;
    responder.reply(204, "");
    client(&responder)
        .pass(&token())
        .set_field(ISSUE, &size(), None)
        .await
        .unwrap();
    let requests = responder.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "DELETE");
    assert_eq!(requests[0].path, format!("{VALUES_PATH}/101"));
}

#[tokio::test]
async fn a_write_makes_the_next_read_fetch_again() {
    let responder = Responder::start().await;
    responder.reply_with(200, &[("ETag", "\"v1\"")], VALUES);
    responder.reply(204, "");
    responder.reply(200, "[]");
    let client = client(&responder);
    let pass = client.pass(&token());
    pass.read_field(ISSUE, &size()).await.unwrap();
    pass.set_field(ISSUE, &size(), None).await.unwrap();
    assert_eq!(pass.read_field(ISSUE, &size()).await, Ok(FieldRead::Unset));
    let requests = responder.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[2].header("if-none-match"), None);
}

#[tokio::test]
async fn two_effort_labels_conflict() {
    let responder = Responder::start().await;
    responder.reply(200, LABELS_CONFLICT);
    let read = client(&responder)
        .pass(&token())
        .read_field(ISSUE, &effort_labels())
        .await;
    assert_eq!(
        read,
        Ok(FieldRead::Conflict(vec![
            value(None, None, "S"),
            value(None, None, "L"),
        ]))
    );
    let requests = responder.requests();
    assert_eq!(requests[0].path, "/repos/acme/widgets/issues/7");
}

#[tokio::test]
async fn one_effort_label_reads_as_its_option() {
    let responder = Responder::start().await;
    responder.reply(200, r#"{"labels":[{"name":"Effort:m"},{"name":"bug"}]}"#);
    responder.reply(200, r#"{"labels":[{"name":"effort:XXL"}]}"#);
    let client = client(&responder);
    let read = client
        .pass(&token())
        .read_field(ISSUE, &effort_labels())
        .await;
    assert_eq!(read, Ok(FieldRead::Value(value(None, None, "M"))));
    let read = client
        .pass(&token())
        .read_field(ISSUE, &effort_labels())
        .await;
    assert_eq!(read, Err(FieldError::Field(FieldFinding::OptionUnmapped)));
}

#[tokio::test]
async fn a_label_swap_sends_one_put_keeping_the_other_labels() {
    let responder = Responder::start().await;
    responder.reply(200, LABELS_CONFLICT);
    responder.reply(200, "[]");
    client(&responder)
        .pass(&token())
        .set_field(ISSUE, &effort_labels(), Some("M"))
        .await
        .unwrap();
    let requests = responder.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].method, "PUT");
    assert_eq!(requests[1].path, "/repos/acme/widgets/issues/7/labels");
    assert_eq!(
        body(&requests[1]),
        serde_json::json!({"labels": ["mintswarm", "effort:M"]})
    );
    assert_token_only_in_authorization(&requests);
}

#[tokio::test]
async fn catalogue_sends_its_token_and_lists_every_option() {
    let responder = Responder::start().await;
    responder.reply(200, ORG_FIELD);
    let catalogue = GitHubFieldCatalogue::new(&responder.base, "acme", "widgets", token()).unwrap();
    let options = catalogue.options(&size()).await.unwrap();
    assert_eq!(
        options,
        vec![
            value(Some(101), Some(1001), "XS"),
            value(Some(101), Some(1002), "S"),
            value(Some(101), Some(1003), "M"),
            value(Some(101), Some(1004), "L"),
            value(Some(101), Some(1005), "Huge"),
        ]
    );
    let requests = responder.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path, "/orgs/acme/issue-fields/101");
    assert_token_only_in_authorization(&requests);
}

#[tokio::test]
async fn catalogue_lists_matching_labels_page_by_page() {
    let responder = Responder::start().await;
    // A full first page, its last label an effort label, then a short second page.
    let first: Vec<serde_json::Value> = (0..99)
        .map(|n| serde_json::json!({"name": format!("other-{n}")}))
        .chain([serde_json::json!({"name": "effort:S"})])
        .collect();
    responder.reply(200, serde_json::Value::from(first).to_string());
    responder.reply(200, r#"[{"name":"Effort:XL"},{"name":"bug"}]"#);
    let catalogue = GitHubFieldCatalogue::new(&responder.base, "acme", "widgets", token()).unwrap();
    let options = catalogue.options(&effort_labels()).await.unwrap();
    assert_eq!(
        options,
        vec![value(None, None, "S"), value(None, None, "XL")]
    );
    let paths: Vec<String> = responder.requests().into_iter().map(|r| r.path).collect();
    assert_eq!(
        paths,
        [
            "/repos/acme/widgets/labels?per_page=100&page=1",
            "/repos/acme/widgets/labels?per_page=100&page=2",
        ]
    );
}

#[tokio::test]
async fn invalid_repository_names_are_refused() {
    assert!(FieldClient::new("http://127.0.0.1:1", "acme", "../x").is_err());
    assert!(GitHubFieldCatalogue::new("http://127.0.0.1:1", "", "widgets", token()).is_err());
}
