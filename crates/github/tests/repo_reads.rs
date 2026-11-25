//! Repository reads against a scripted local server answering with recorded responses in the
//! shape of GitHub's REST API, and the same cases against an in-memory fake host.
// Not redundant: clippy's `allow-unwrap-in-tests` covers a `tests/` crate only under `cfg(test)`.
#![cfg(test)]

mod support;

use std::collections::BTreeMap;
use std::future::Future;
use std::time::{Duration, Instant};

use mintswarm_domain::hosting::{RepoReadError, RepoReads};
use mintswarm_domain::ids::RevId;
use mintswarm_domain::tracker::{IssueType, Login};
use mintswarm_domain::vcs::RepoPath;
use mintswarm_github::{GitHubReads, GitHubRepoReads, Kinds, Permissions, Token};
use support::Responder;

const TOKEN: &str = "ghs_r3p0T0kenV4lue";
const REPOSITORY: &str = include_str!("fixtures/repo/repository.json");
const BRANCH: &str = include_str!("fixtures/repo/branch.json");
const CONFIG: &[u8] = include_bytes!("fixtures/repo/config.yaml");
const CONFIG_CONTENTS: &str = include_str!("fixtures/repo/config-contents.json");
const LARGE_CONTENTS: &str = include_str!("fixtures/repo/large-contents.json");
/// The head `branch.json` records.
const HEAD: &str = "4e1d2c3b5a6978877665544332211009f8e7d6c5";
/// The commit the files are read at.
const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const CONFIG_PATH: &str = ".mintswarm/config.yaml";
const MISSING_PATH: &str = ".mintswarm/missing.yaml";
const REPO_PATH: &str = "/repos/acme/widgets";
const NOT_FOUND: &str = r#"{"message":"Not Found","status":"404"}"#;
const LIMITED: &str = r#"{"message":"You have exceeded a secondary rate limit"}"#;

fn token() -> Token {
    Token::new(TOKEN.to_owned(), 0)
}

fn client(responder: &Responder) -> GitHubReads {
    GitHubReads::new(
        &responder.base,
        "acme",
        "widgets",
        Kinds::issue_types([(IssueType::Plan, "Plan".to_owned())]),
        Vec::new(),
        Login("mintswarm-dev[bot]".into()),
    )
    .unwrap()
}

fn path(text: &str) -> RepoPath {
    RepoPath(text.as_bytes().to_vec())
}

fn commit() -> RevId {
    RevId(COMMIT.to_owned())
}

fn contents_path(file: &str) -> String {
    format!("{REPO_PATH}/contents/{file}?ref={COMMIT}")
}

/// One behaviour every host's repository reads must show.
#[derive(Clone, Copy, Debug)]
enum Case {
    /// The default branch's head is the recorded commit.
    Head,
    /// The config file reads as its exact bytes.
    ConfigBytes,
    /// A path with no file reads as absent, not as an error.
    MissingPath,
    /// A rate limit with a one-second `retry-after` surfaces as a rate limit with that delay.
    RateLimited,
}

const CASES: [Case; 4] = [
    Case::Head,
    Case::ConfigBytes,
    Case::MissingPath,
    Case::RateLimited,
];

async fn check(host: &impl RepoReads, case: Case) {
    match case {
        Case::Head => {
            assert_eq!(
                host.default_branch_head().await.unwrap(),
                RevId(HEAD.to_owned())
            );
        }
        Case::ConfigBytes => {
            let bytes = host.read_file(&commit(), &path(CONFIG_PATH)).await.unwrap();
            assert_eq!(bytes.as_deref(), Some(CONFIG));
        }
        Case::MissingPath => {
            let bytes = host
                .read_file(&commit(), &path(MISSING_PATH))
                .await
                .unwrap();
            assert_eq!(bytes, None);
        }
        Case::RateLimited => {
            let error = host
                .read_file(&commit(), &path(CONFIG_PATH))
                .await
                .unwrap_err();
            assert!(error.is_rate_limited(), "{error:?}");
            assert_eq!(error.retry_after(), Some(Duration::from_secs(1)));
        }
    }
}

#[tokio::test]
async fn github_passes_the_shared_cases() {
    for case in CASES {
        let responder = Responder::start().await;
        match case {
            Case::Head => {
                responder.reply(200, REPOSITORY);
                responder.reply(200, BRANCH);
            }
            Case::ConfigBytes => responder.reply(200, CONFIG_CONTENTS),
            Case::MissingPath => responder.reply(404, NOT_FOUND),
            Case::RateLimited => responder.reply_with(403, &[("retry-after", "1")], LIMITED),
        }
        let reads = client(&responder);
        let token = token();
        check(&reads.repo_reads(&token), case).await;
    }
}

/// An in-memory host: a head, files by commit and path, and an optional rate limit that
/// refuses every read.
#[derive(Default)]
struct FakeHost {
    head: Option<RevId>,
    files: BTreeMap<(RevId, RepoPath), Vec<u8>>,
    rate_limit: Option<Duration>,
}

impl FakeHost {
    fn limited(&self) -> Result<(), RepoReadError> {
        match self.rate_limit {
            Some(delay) => Err(RepoReadError::RateLimited {
                retry_after: Some(delay),
            }),
            None => Ok(()),
        }
    }
}

impl RepoReads for FakeHost {
    fn default_branch_head(&self) -> impl Future<Output = Result<RevId, RepoReadError>> + Send {
        let head = self
            .limited()
            .and_then(|()| self.head.clone().ok_or(RepoReadError::Inaccessible));
        async move { head }
    }

    fn read_file(
        &self,
        rev: &RevId,
        path: &RepoPath,
    ) -> impl Future<Output = Result<Option<Vec<u8>>, RepoReadError>> + Send {
        let bytes = self
            .limited()
            .map(|()| self.files.get(&(rev.clone(), path.clone())).cloned());
        async move { bytes }
    }
}

#[tokio::test]
async fn fake_passes_the_shared_cases() {
    for case in CASES {
        let mut host = FakeHost {
            head: Some(RevId(HEAD.to_owned())),
            ..FakeHost::default()
        };
        host.files
            .insert((commit(), path(CONFIG_PATH)), CONFIG.to_vec());
        if matches!(case, Case::RateLimited) {
            host.rate_limit = Some(Duration::from_secs(1));
        }
        check(&host, case).await;
    }
}

#[tokio::test]
async fn the_recorded_contents_decode_to_the_file() {
    let responder = Responder::start().await;
    responder.reply(200, CONFIG_CONTENTS);
    let reads = client(&responder);
    let token = token();
    let bytes = reads
        .repo_reads(&token)
        .read_file(&commit(), &path(CONFIG_PATH))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bytes, CONFIG);
    assert!(
        !std::str::from_utf8(CONFIG).unwrap().is_ascii(),
        "the fixture has a non-ASCII character"
    );
}

#[tokio::test]
async fn the_head_reads_the_repository_then_its_branch() {
    let responder = Responder::start().await;
    responder.reply(200, REPOSITORY);
    responder.reply(200, BRANCH);
    let reads = client(&responder);
    let token = token();
    reads
        .repo_reads(&token)
        .default_branch_head()
        .await
        .unwrap();
    let requests = responder.requests();
    let paths: Vec<_> = requests.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(
        paths,
        [REPO_PATH.to_owned(), format!("{REPO_PATH}/branches/master")]
    );
    assert!(requests.iter().all(|r| r.method == "GET"));
}

#[tokio::test]
async fn a_slashed_branch_is_encoded_per_segment() {
    let responder = Responder::start().await;
    responder.reply(
        200,
        r#"{"name":"widgets","default_branch":"release/2026 q4"}"#,
    );
    responder.reply(200, BRANCH);
    let reads = client(&responder);
    let token = token();
    reads
        .repo_reads(&token)
        .default_branch_head()
        .await
        .unwrap();
    assert_eq!(
        responder.requests()[1].path,
        format!("{REPO_PATH}/branches/release/2026%20q4")
    );
}

#[tokio::test]
async fn a_missing_repository_or_branch_is_inaccessible() {
    for found in [0, 1] {
        let responder = Responder::start().await;
        if found == 1 {
            responder.reply(200, REPOSITORY);
        }
        responder.reply(404, NOT_FOUND);
        let reads = client(&responder);
        let token = token();
        let error = reads
            .repo_reads(&token)
            .default_branch_head()
            .await
            .unwrap_err();
        assert!(matches!(error, RepoReadError::Inaccessible), "{error:?}");
    }
}

#[tokio::test]
async fn a_head_that_is_not_a_commit_id_is_unexpected() {
    let responder = Responder::start().await;
    responder.reply(200, REPOSITORY);
    responder.reply(200, r#"{"name":"master","commit":{"sha":"ABC"}}"#);
    let reads = client(&responder);
    let token = token();
    let error = reads
        .repo_reads(&token)
        .default_branch_head()
        .await
        .unwrap_err();
    assert!(matches!(error, RepoReadError::Unexpected), "{error:?}");
}

#[tokio::test]
async fn a_file_is_read_at_the_commit_by_encoded_segments() {
    let responder = Responder::start().await;
    responder.reply(200, CONFIG_CONTENTS);
    responder.reply(404, NOT_FOUND);
    let reads = client(&responder);
    let token = token();
    let repo = reads.repo_reads(&token);
    repo.read_file(&commit(), &path(CONFIG_PATH)).await.unwrap();
    repo.read_file(&commit(), &path("docs/stage defs/plan+.md"))
        .await
        .unwrap();
    let paths: Vec<_> = responder.requests().into_iter().map(|r| r.path).collect();
    assert_eq!(
        paths,
        [
            contents_path(CONFIG_PATH),
            contents_path("docs/stage%20defs/plan%2B.md"),
        ]
    );
}

#[tokio::test]
async fn a_directory_a_symlink_and_a_submodule_read_as_absent() {
    let bodies = [
        r#"[{"type":"file","name":"config.yaml","path":".mintswarm/config.yaml"}]"#,
        r#"{"type":"symlink","target":"../elsewhere","size":12,"name":"link","path":"link"}"#,
        r#"{"type":"submodule","submodule_git_url":"git://example.invalid/sub.git","size":0,"name":"sub","path":"sub"}"#,
    ];
    for body in bodies {
        let responder = Responder::start().await;
        responder.reply(200, body);
        let reads = client(&responder);
        let token = token();
        let bytes = reads
            .repo_reads(&token)
            .read_file(&commit(), &path(".mintswarm"))
            .await
            .unwrap();
        assert_eq!(bytes, None, "{body}");
    }
}

#[tokio::test]
async fn a_file_over_the_inline_limit_is_too_large() {
    let responder = Responder::start().await;
    responder.reply(200, LARGE_CONTENTS);
    let reads = client(&responder);
    let token = token();
    let error = reads
        .repo_reads(&token)
        .read_file(&commit(), &path("assets/big.bin"))
        .await
        .unwrap_err();
    assert!(matches!(error, RepoReadError::TooLarge), "{error:?}");
}

#[tokio::test]
async fn contents_of_another_size_or_encoding_are_unexpected() {
    let bodies = [
        r#"{"type":"file","encoding":"base64","size":5,"content":"YWJj\n"}"#,
        r#"{"type":"file","encoding":"utf-8","size":3,"content":"abc"}"#,
        r#"{"type":"file","encoding":"base64","size":3,"content":"!!!!"}"#,
    ];
    for body in bodies {
        let responder = Responder::start().await;
        responder.reply(200, body);
        let reads = client(&responder);
        let token = token();
        let error = reads
            .repo_reads(&token)
            .read_file(&commit(), &path("a.txt"))
            .await
            .unwrap_err();
        assert!(
            matches!(error, RepoReadError::Unexpected),
            "{body}: {error:?}"
        );
    }
}

#[tokio::test]
async fn an_unnameable_path_is_refused_before_any_request() {
    let responder = Responder::start().await;
    let reads = client(&responder);
    let token = token();
    let repo = reads.repo_reads(&token);
    for bad in [
        &b""[..],
        b"/.mintswarm/config.yaml",
        b".mintswarm/",
        b".mintswarm//config.yaml",
        b"./config.yaml",
        b".mintswarm/../config.yaml",
        b"\xff\xfe",
    ] {
        let error = repo
            .read_file(&commit(), &RepoPath(bad.to_vec()))
            .await
            .unwrap_err();
        assert!(
            matches!(error, RepoReadError::InvalidPath),
            "{bad:?}: {error:?}"
        );
    }
    assert!(responder.requests().is_empty());
}

#[tokio::test]
async fn a_rev_that_is_not_a_full_commit_id_is_refused_before_any_request() {
    let responder = Responder::start().await;
    let reads = client(&responder);
    let token = token();
    let repo = reads.repo_reads(&token);
    for bad in [
        "",
        "master",
        "v1.0",
        &COMMIT[..7],
        COMMIT.to_uppercase().as_str(),
        format!("{COMMIT}0").as_str(),
    ] {
        let error = repo
            .read_file(&RevId(bad.to_owned()), &path(CONFIG_PATH))
            .await
            .unwrap_err();
        assert!(
            matches!(error, RepoReadError::InvalidRev),
            "{bad:?}: {error:?}"
        );
    }
    assert!(responder.requests().is_empty());
}

#[tokio::test]
async fn a_refused_credential_or_permission_is_inaccessible() {
    let refusals = [
        (401, r#"{"message":"Bad credentials","status":"401"}"#),
        (
            403,
            r#"{"message":"Resource not accessible by integration","status":"403"}"#,
        ),
    ];
    for (status, body) in refusals {
        for head in [false, true] {
            let responder = Responder::start().await;
            responder.reply(status, body);
            let reads = client(&responder);
            let token = token();
            let repo = reads.repo_reads(&token);
            let error = if head {
                repo.default_branch_head().await.unwrap_err()
            } else {
                repo.read_file(&commit(), &path(CONFIG_PATH))
                    .await
                    .unwrap_err()
            };
            assert!(
                matches!(error, RepoReadError::Inaccessible),
                "{status}, head {head}: {error:?}"
            );
            assert!(!error.is_rate_limited());
        }
    }
}

#[tokio::test]
async fn a_rate_limit_pauses_the_next_read() {
    let responder = Responder::start().await;
    responder.reply_with(403, &[("retry-after", "1")], LIMITED);
    responder.reply(200, CONFIG_CONTENTS);
    let reads = client(&responder);
    let token = token();
    let repo = reads.repo_reads(&token);
    let started = Instant::now();
    let error = repo
        .read_file(&commit(), &path(CONFIG_PATH))
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            RepoReadError::RateLimited {
                retry_after: Some(delay)
            } if delay == Duration::from_secs(1)
        ),
        "{error:?}"
    );
    assert_eq!(responder.requests().len(), 1);
    let bytes = repo.read_file(&commit(), &path(CONFIG_PATH)).await.unwrap();
    assert_eq!(bytes.as_deref(), Some(CONFIG));
    // The pause starts at the first reply, after `started`, so the second read can only have
    // reached the responder once a second has passed since `started`.
    assert!(
        started.elapsed() >= Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(responder.requests().len(), 2);
}

#[test]
fn the_repository_reads_ask_for_contents_and_metadata_only() {
    assert_eq!(
        GitHubRepoReads::permissions(),
        Permissions::read(["contents", "metadata"])
    );
}
