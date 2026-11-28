//! Repository reads against a scripted local server answering with recorded responses in the
//! shape of GitHub's REST API, and the same cases against an in-memory fake host.
// Not redundant: clippy's `allow-unwrap-in-tests` covers a `tests/` crate only under `cfg(test)`.
#![cfg(test)]

mod support;

use std::collections::BTreeMap;
use std::future::Future;
use std::time::{Duration, Instant};

use mintswarm_domain::hosting::{CODEOWNERS_PATHS, CodeOwnersFile, RepoReadError, RepoReads};
use mintswarm_domain::ids::RevId;
use mintswarm_domain::setup::ProjectSetup;
use mintswarm_domain::tracker::{IssueType, Login};
use mintswarm_domain::vcs::{BranchName, RepoPath};
use mintswarm_github::{GitHubReads, GitHubRepoReads, Kinds, Permissions, RetryPolicy, Token};
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
    // A single attempt per read, so each read takes one scripted reply.
    .with_retry(RetryPolicy {
        attempts: 1,
        ..RetryPolicy::default()
    })
}

fn path(text: &str) -> RepoPath {
    RepoPath(text.as_bytes().to_vec())
}

/// The work branch the head reads ask for: not the fixtures' default branch, `master`.
const WORK: &str = "develop";

fn work() -> BranchName {
    BranchName(WORK.to_owned())
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
    /// The work branch's head is the recorded commit.
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
                host.branch_head(&work()).await.unwrap(),
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
            Case::Head => responder.reply(200, BRANCH),
            Case::ConfigBytes => responder.reply(200, CONFIG_CONTENTS),
            Case::MissingPath => responder.reply(404, NOT_FOUND),
            Case::RateLimited => responder.reply_with(403, &[("retry-after", "1")], LIMITED),
        }
        let reads = client(&responder);
        let token = token();
        check(&reads.repo_reads(&token), case).await;
    }
}

/// An in-memory host: heads by branch, a default branch, files by commit and path, and an
/// optional rate limit that refuses every read.
#[derive(Default)]
struct FakeHost {
    heads: BTreeMap<String, RevId>,
    default: Option<BranchName>,
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
    fn branch_head(
        &self,
        branch: &BranchName,
    ) -> impl Future<Output = Result<RevId, RepoReadError>> + Send {
        let head = self.limited().and_then(|()| {
            if !BranchName::is_valid(&branch.0) {
                return Err(RepoReadError::InvalidBranch);
            }
            self.heads
                .get(&branch.0)
                .cloned()
                .ok_or(RepoReadError::Inaccessible)
        });
        async move { head }
    }

    fn default_branch(&self) -> impl Future<Output = Result<BranchName, RepoReadError>> + Send {
        let name = self
            .limited()
            .and_then(|()| self.default.clone().ok_or(RepoReadError::Inaccessible));
        async move { name }
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
            default: Some(BranchName("master".to_owned())),
            ..FakeHost::default()
        };
        host.heads.insert(WORK.to_owned(), RevId(HEAD.to_owned()));
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
async fn the_head_reads_only_the_named_branch() {
    let responder = Responder::start().await;
    responder.reply(200, BRANCH);
    responder.reply(200, CONFIG_CONTENTS);
    let reads = client(&responder);
    let token = token();
    let repo = reads.repo_reads(&token);
    let head = repo.branch_head(&work()).await.unwrap();
    let bytes = repo.read_file(&head, &path(CONFIG_PATH)).await.unwrap();
    assert_eq!(bytes.as_deref(), Some(CONFIG));
    let requests = responder.requests();
    let paths: Vec<_> = requests.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            format!("{REPO_PATH}/branches/{WORK}"),
            format!("{REPO_PATH}/contents/{CONFIG_PATH}?ref={HEAD}"),
        ]
    );
    assert!(requests.iter().all(|r| r.method == "GET"));
}

#[tokio::test]
async fn a_slashed_branch_is_encoded_per_segment() {
    let responder = Responder::start().await;
    responder.reply(200, BRANCH);
    let reads = client(&responder);
    let token = token();
    reads
        .repo_reads(&token)
        .branch_head(&BranchName("release/2026+q4".to_owned()))
        .await
        .unwrap();
    assert_eq!(
        responder.requests()[0].path,
        format!("{REPO_PATH}/branches/release/2026%2Bq4")
    );
}

#[tokio::test]
async fn an_invalid_branch_is_refused_before_any_request() {
    let responder = Responder::start().await;
    let reads = client(&responder);
    let token = token();
    let repo = reads.repo_reads(&token);
    for bad in ["", "a..b", "a b", "-x", "HEAD", "/a"] {
        let error = repo
            .branch_head(&BranchName(bad.to_owned()))
            .await
            .unwrap_err();
        assert!(
            matches!(error, RepoReadError::InvalidBranch),
            "{bad:?}: {error:?}"
        );
    }
    assert!(responder.requests().is_empty());
}

#[tokio::test]
async fn the_default_branch_is_the_repository_s() {
    let responder = Responder::start().await;
    responder.reply(200, REPOSITORY);
    let reads = client(&responder);
    let token = token();
    let name = reads.repo_reads(&token).default_branch().await.unwrap();
    assert_eq!(name, BranchName("master".to_owned()));
    assert_eq!(responder.requests()[0].path, REPO_PATH);
}

#[tokio::test]
async fn setup_defaults_the_work_branch_to_the_default_branch() {
    let responder = Responder::start().await;
    responder.reply(200, r#"{"name":"widgets","default_branch":"develop"}"#);
    let reads = client(&responder);
    let token = token();
    let setup = ProjectSetup::from_default_branch(&reads.repo_reads(&token))
        .await
        .unwrap();
    assert_eq!(setup.work_branch, work());
}

#[tokio::test]
async fn an_empty_or_invalid_default_branch_is_unexpected() {
    for body in [
        r#"{"name":"widgets","default_branch":""}"#,
        r#"{"name":"widgets","default_branch":"a b"}"#,
        r#"{"name":"widgets","default_branch":7}"#,
        r#"{"name":"widgets"}"#,
    ] {
        let responder = Responder::start().await;
        responder.reply(200, body);
        let reads = client(&responder);
        let token = token();
        let error = reads.repo_reads(&token).default_branch().await.unwrap_err();
        assert!(
            matches!(error, RepoReadError::Unexpected),
            "{body}: {error:?}"
        );
    }
}

#[tokio::test]
async fn a_missing_repository_or_branch_is_inaccessible() {
    for head in [false, true] {
        let responder = Responder::start().await;
        responder.reply(404, NOT_FOUND);
        let reads = client(&responder);
        let token = token();
        let repo = reads.repo_reads(&token);
        let error = if head {
            repo.branch_head(&work()).await.unwrap_err()
        } else {
            repo.default_branch().await.unwrap_err()
        };
        assert!(matches!(error, RepoReadError::Inaccessible), "{error:?}");
    }
}

#[tokio::test]
async fn a_head_that_is_not_a_commit_id_is_unexpected() {
    let responder = Responder::start().await;
    responder.reply(200, r#"{"name":"develop","commit":{"sha":"ABC"}}"#);
    let reads = client(&responder);
    let token = token();
    let error = reads
        .repo_reads(&token)
        .branch_head(&work())
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
                repo.branch_head(&work()).await.unwrap_err()
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
async fn a_server_error_left_after_the_retries_is_transport() {
    let responder = Responder::start().await;
    responder.reply(502, "<html>Bad Gateway</html>");
    let reads = client(&responder);
    let token = token();
    let error = reads
        .repo_reads(&token)
        .read_file(&commit(), &path(CONFIG_PATH))
        .await
        .unwrap_err();
    assert!(matches!(error, RepoReadError::Transport(_)), "{error:?}");
    assert!(!error.to_string().contains(TOKEN));
    assert_eq!(responder.requests().len(), 1);

    // With more attempts, a server error followed by the file reads the file.
    let responder = Responder::start().await;
    responder.reply(500, "");
    responder.reply(200, CONFIG_CONTENTS);
    let reads = client(&responder).with_retry(RetryPolicy {
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(1),
        ..RetryPolicy::default()
    });
    let bytes = reads
        .repo_reads(&token)
        .read_file(&commit(), &path(CONFIG_PATH))
        .await
        .unwrap();
    assert_eq!(bytes.as_deref(), Some(CONFIG));
    assert_eq!(responder.requests().len(), 2);
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

/// A contents answer for a file holding `bytes`, in the shape `config-contents.json` records.
fn contents_json(name: &str, bytes: &[u8]) -> String {
    serde_json::json!({
        "type": "file",
        "encoding": "base64",
        "size": bytes.len(),
        "name": name.rsplit('/').next(),
        "path": name,
        "content": base64(bytes),
        "sha": "3f1c2a9e8b7d6c5f4e3a2b1c0d9e8f7a6b5c4d3e",
    })
    .to_string()
}

/// `bytes` in standard base64 with padding.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let word = chunk.iter().enumerate().fold(0u32, |word, (i, byte)| {
            word | (u32::from(*byte) << (16 - 8 * i))
        });
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((word >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

const CODEOWNERS: &str = "* @acme/maintainers\n/crates/github/ @acme/github-owners\n";

#[tokio::test]
async fn read_text_returns_text_at_a_commit() {
    let responder = Responder::start().await;
    responder.reply(200, CONFIG_CONTENTS);
    let reads = client(&responder);
    let token = token();
    let text = reads
        .repo_reads(&token)
        .read_text(&commit(), &path(CONFIG_PATH))
        .await
        .unwrap();
    assert_eq!(text.as_deref().map(str::as_bytes), Some(CONFIG));
    assert_eq!(
        responder.requests()[0].path,
        contents_path(".mintswarm/config.yaml")
    );
}

#[tokio::test]
async fn a_missing_text_file_is_none() {
    let responder = Responder::start().await;
    responder.reply(404, NOT_FOUND);
    let reads = client(&responder);
    let token = token();
    let text = reads
        .repo_reads(&token)
        .read_text(&commit(), &path(MISSING_PATH))
        .await
        .unwrap();
    assert_eq!(text, None);
}

#[tokio::test]
async fn non_utf8_is_not_text() {
    let responder = Responder::start().await;
    responder.reply(200, contents_json("assets/logo.bin", b"\x89PNG\xff\xfe"));
    let reads = client(&responder);
    let token = token();
    let error = reads
        .repo_reads(&token)
        .read_text(&commit(), &path("assets/logo.bin"))
        .await
        .unwrap_err();
    assert!(matches!(error, RepoReadError::NotText), "{error:?}");
}

#[tokio::test]
async fn codeowners_is_the_first_of_three_paths() {
    let responder = Responder::start().await;
    responder.reply(404, NOT_FOUND);
    responder.reply(200, contents_json("CODEOWNERS", CODEOWNERS.as_bytes()));
    let reads = client(&responder);
    let token = token();
    let found = reads
        .repo_reads(&token)
        .codeowners(&commit())
        .await
        .unwrap();
    assert_eq!(
        found,
        Some(CodeOwnersFile {
            path: path("CODEOWNERS"),
            text: CODEOWNERS.to_owned(),
        })
    );
    // `docs/CODEOWNERS` is never asked for.
    let paths: Vec<_> = responder.requests().into_iter().map(|r| r.path).collect();
    assert_eq!(
        paths,
        [
            contents_path(".github/CODEOWNERS"),
            contents_path("CODEOWNERS")
        ]
    );
}

#[tokio::test]
async fn no_codeowners_file_is_none() {
    let responder = Responder::start().await;
    for _ in CODEOWNERS_PATHS {
        responder.reply(404, NOT_FOUND);
    }
    let reads = client(&responder);
    let token = token();
    let found = reads
        .repo_reads(&token)
        .codeowners(&commit())
        .await
        .unwrap();
    assert_eq!(found, None);
    let paths: Vec<_> = responder.requests().into_iter().map(|r| r.path).collect();
    let expected: Vec<_> = CODEOWNERS_PATHS.iter().map(|p| contents_path(p)).collect();
    assert_eq!(paths, expected);
}

#[tokio::test]
async fn codeowners_at_a_moving_ref_is_refused_before_any_request() {
    let responder = Responder::start().await;
    let reads = client(&responder);
    let token = token();
    let error = reads
        .repo_reads(&token)
        .codeowners(&RevId("master".into()))
        .await
        .unwrap_err();
    assert!(matches!(error, RepoReadError::InvalidRev), "{error:?}");
    assert!(responder.requests().is_empty());
}
