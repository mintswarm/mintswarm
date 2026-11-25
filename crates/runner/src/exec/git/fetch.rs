//! `fetch` into the store over HTTPS. The URL is given on the command line and never written to
//! the store; the credential reaches git only through the inline helper, which reads it from the
//! child's environment, so it is never in a URL, the store's config or argv.

use std::process::Command;

use mintswarm_domain::ids::RevId;
use mintswarm_domain::plane::Credential;
use mintswarm_domain::vcs::{FetchReport, FetchedRef, RefOrId, Remote};

use super::env::{failure, success};
use super::read::{id_text, resolve};
use super::{GitError, GitStore};

/// Where fetched refs go: `<FETCHED>/<remote>/heads/…`, `<FETCHED>/<remote>/tags/…` and
/// `<FETCHED>/ids/<id>`.
pub(super) const FETCHED: &str = "refs/mintswarm/fetched";

/// `github/common.sh`'s inline helper, word for word: it answers only for `https://github.com`,
/// with the token from `MSW_TOKEN` in its own environment.
pub(super) const CREDENTIAL_HELPER: &str = r#"credential.helper=!f() { test "$1" = get || exit 0; p=; h=; while IFS= read -r l; do case "$l" in "") break;; protocol=https) p=1;; host=github.com) h=1;; esac; done; test "$p$h" = 11 && printf "username=x-access-token\npassword=%s\n" "$MSW_TOKEN"; }; f"#;

/// Fetch `wants` from `from` into the store.
pub(super) fn fetch(
    store: &GitStore,
    from: &Remote,
    wants: &[RefOrId],
    cred: Option<&Credential>,
) -> Result<FetchReport, GitError> {
    check_url(&from.url, store.allow_file)?;
    check_remote_name(&from.name)?;
    if wants.is_empty() {
        return Ok(FetchReport::default());
    }
    let mut refspecs = Vec::with_capacity(wants.len());
    let mut targets = Vec::with_capacity(wants.len());
    for want in wants {
        let (refspec, target) = match want {
            RefOrId::Ref(name) => {
                check_ref(store, name)?;
                let rest = name.strip_prefix("refs/").unwrap_or(name);
                let target = format!("{FETCHED}/{}/{rest}", from.name);
                (format!("+{name}:{target}"), target)
            }
            RefOrId::Id(id) => {
                let id = store.checked_id(id)?;
                let target = format!("{FETCHED}/ids/{id}");
                (format!("{id}:{target}"), target)
            }
        };
        refspecs.push(refspec);
        targets.push(target);
    }
    let mut args = vec![
        "fetch",
        "--no-write-fetch-head",
        "--no-tags",
        "--no-auto-gc",
        "--no-recurse-submodules",
        "--quiet",
        from.url.as_str(),
    ];
    args.extend(refspecs.iter().map(String::as_str));
    let output = store.output(credential_command(store, cred, &args), "fetch")?;
    success("fetch", output)?;
    read_back(store, &targets)
}

/// A git command with the inline credential helper and, when given, the credential in
/// `MSW_TOKEN`; without a credential, no helper answers.
pub(super) fn credential_command(
    store: &GitStore,
    cred: Option<&Credential>,
    args: &[&str],
) -> Command {
    match cred {
        Some(cred) => {
            let mut command = store.command_with(&[CREDENTIAL_HELPER], args);
            command.env("MSW_TOKEN", &cred.token);
            command
        }
        None => store.command(args),
    }
}

/// The fetched refs, in the order of `targets`, each with the commit it ends at.
fn read_back(store: &GitStore, targets: &[String]) -> Result<FetchReport, GitError> {
    let mut args = vec!["for-each-ref", "--format=%(refname)%00%(objectname)"];
    args.extend(targets.iter().map(String::as_str));
    let out = store.git(args)?;
    let mut found = Vec::new();
    for line in out
        .split(|&byte| byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let text = String::from_utf8_lossy(line);
        let Some((name, id)) = text.split_once('\0') else {
            return Err(GitError::Parse {
                command: "for-each-ref".to_owned(),
                detail: format!("line {text:?}"),
            });
        };
        found.push((name.to_owned(), id.to_owned()));
    }
    let refs = targets
        .iter()
        .map(|target| {
            found
                .iter()
                .find(|(name, _)| name == target)
                .ok_or_else(|| GitError::NotFound(target.clone()))
                .and_then(|(name, id)| {
                    Ok(FetchedRef {
                        name: name.clone(),
                        id: peel(store, name, id)?,
                    })
                })
        })
        .collect::<Result<_, _>>()?;
    Ok(FetchReport { refs })
}

/// The commit the object `id` of the fetched ref `name` ends at: the object itself, or what an
/// annotated tag peels to. A ref that ends at anything else is deleted and refused.
fn peel(store: &GitStore, name: &str, id: &str) -> Result<RevId, GitError> {
    let id = id_text(store, id.as_bytes(), "for-each-ref")?;
    let spec = format!("{id}^{{commit}}");
    let output = store.output(
        store.command(["rev-parse", "--verify", "--quiet", spec.as_str()]),
        "rev-parse",
    )?;
    match output.status.code() {
        Some(0) => {
            let commit = id_text(store, output.stdout.trim_ascii_end(), "rev-parse")?;
            resolve(store, &commit)
        }
        Some(1) => {
            store.git(["update-ref", "-d", name, id.as_str()])?;
            Err(GitError::NotCommit(name.to_owned()))
        }
        _ => Err(failure("rev-parse", &output)),
    }
}

/// Refuse any URL but `https://host/path` without userinfo, query or fragment; `file:///path`
/// also when the store allows the test-only remote.
pub(super) fn check_url(url: &str, allow_file: bool) -> Result<(), GitError> {
    let refuse = |why: &str| Err(GitError::Url(why.to_owned()));
    if url
        .bytes()
        .any(|byte| byte.is_ascii_control() || byte == b' ')
    {
        return refuse("it holds a space or a control character");
    }
    if url.contains(['?', '#']) {
        return refuse("it has a query or a fragment");
    }
    if allow_file && url.starts_with("file:///") {
        return Ok(());
    }
    let Some(rest) = url.strip_prefix("https://") else {
        return refuse("only https:// is fetched");
    };
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty() {
        return refuse("it has no host");
    }
    if authority.contains('@') {
        return refuse("it has userinfo");
    }
    Ok(())
}

/// Refuse a remote name that cannot be one ref component.
fn check_remote_name(name: &str) -> Result<(), GitError> {
    let ok = !name.is_empty()
        && !name.starts_with(['.', '-'])
        && name != "ids"
        && !name.ends_with(".lock")
        && !name.contains("..")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(GitError::RefName(name.to_owned()))
    }
}

/// Refuse a want that is not a full, well-formed branch or tag name.
fn check_ref(store: &GitStore, name: &str) -> Result<(), GitError> {
    let refuse = || Err(GitError::RefName(name.to_owned()));
    let rest = name
        .strip_prefix("refs/heads/")
        .or_else(|| name.strip_prefix("refs/tags/"));
    if rest.is_none_or(str::is_empty) || name.contains([':', '+', '*', '\0']) {
        return refuse();
    }
    let output = store.output(
        store.command(["check-ref-format", name]),
        "check-ref-format",
    )?;
    if output.status.success() {
        Ok(())
    } else {
        refuse()
    }
}
