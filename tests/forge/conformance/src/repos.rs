use crate::{
    Result,
    cases::{Auth, Recorder},
    facts::{Fact, add, case},
    json, setup,
    write::SecretKind,
};
use skein_throwaway_forgejo::{Token, fresh_secret};
use std::path::Path;

/// Captures the connected listing after provisioning each source of repository access.
pub(crate) fn record(r: &mut Recorder<'_>, admin: &Token) -> Result<()> {
    for login in ["alice", "bob"] {
        let password = fresh_secret().map_err(|e| crate::failure(&e))?;
        let body = json::object(&[
            ("email", &format!("{login}@example.invalid")),
            ("must_change_password", &false),
            ("password", &password),
            ("username", &login),
        ])?;
        r.send_new(
            &format!("setup-user-repos-{login}"),
            "POST",
            "/api/v1/admin/users",
            Some(std::str::from_utf8(&body)?),
            Auth::Token(admin),
            &[(SecretKind::Password, &password)],
        )?;
    }
    let alice = setup::scoped(r.f, r.secrets, "alice", "example-app", &["read:user", "write:repository"])?;
    let bob = setup::scoped(r.f, r.secrets, "bob", "example-app", &["read:user", "write:repository"])?;
    let body = json::object(&[("name", &"widgets"), ("private", &true)])?;
    for token in [&alice, &bob] {
        let response = r.f.exchange("POST", "/api/v1/user/repos", Some(&body), Some(token), None)?;
        crate::fixture::api_answer("/api/v1/user/repos", &response)?;
    }
    r.f.api("PUT", "/api/v1/repos/bob/widgets/collaborators/alice", &json::object(&[("permission", &"read")])?)?;
    r.f.api(
        "POST",
        "/api/v1/admin/users/bob/orgs",
        &json::object(&[("username", &"acme"), ("visibility", &"private")])?,
    )?;
    r.f.api("POST", "/api/v1/orgs/acme/repos", &body)?;
    let team = r.f.api(
        "POST",
        "/api/v1/orgs/acme/teams",
        br#"{"name":"example-app","permission":"write","includes_all_repositories":true,"units_map":{"repo.code":"write","repo.issues":"write","repo.pulls":"write"}}"#,
    )?;
    let id = team.root().get("id").u64().ok_or("repository listing team id absent")?;
    r.f.api("PUT", &format!("/api/v1/teams/{id}/members/alice"), b"")?;
    for (name, query) in [
        ("setup-user-repos", "page=1&limit=1"),
        ("setup-user-repos-page-2", "page=2&limit=1"),
        ("setup-user-repos-all", "page=1&limit=50"),
        ("setup-user-repos-rest", "page=2&limit=50"),
        ("setup-user-repos-repeat", "page=1&limit=1"),
        ("setup-user-repos-past-end", "page=4&limit=1"),
    ] {
        r.send(name, "GET", &format!("/api/v1/user/repos?{query}"), None, Some(&alice), None)?;
    }
    Ok(())
}

/// Evaluates recorded membership, write permissions and stable repository pages.
pub(crate) fn facts(out: &Path) -> Result<Vec<Fact>> {
    let all = case(out, "setup-user-repos-all")?;
    let rest = case(out, "setup-user-repos-rest")?;
    let first = case(out, "setup-user-repos")?;
    let second = case(out, "setup-user-repos-page-2")?;
    let repeat = case(out, "setup-user-repos-repeat")?;
    let end = case(out, "setup-user-repos-past-end")?;
    let rows = all.body.root().array().zip(rest.body.root().array()).map(|(mut rows, rest)| {
        rows.extend(rest);
        rows
    });
    let expected = [("alice", true), ("bob", false), ("acme", true)];
    let listed = all.status == 200
        && rest.status == 200
        && rows.as_ref().is_some_and(|rows| {
            rows.len() == expected.len()
                && expected.iter().all(|(owner, _)| {
                    rows.iter().any(|row| {
                        row.get("owner").get("login").is(owner)
                            && row.get("name").is("widgets")
                            && row.get("private").bool() == Some(true)
                            && row.get("id").u64().is_some()
                    })
                })
        });
    let permissions = listed
        && rows.as_ref().is_some_and(|rows| {
            expected.iter().all(|(owner, push)| {
                rows.iter().any(|row| {
                    row.get("owner").get("login").is(owner) && row.get("permissions").get("push").bool() == Some(*push)
                })
            })
        });
    let pages = [&first, &second, &repeat]
        .iter()
        .all(|page| page.status == 200 && page.body.root().array().is_some_and(|rows| rows.len() == 1))
        && end.status == 200
        && end.body.root().array().is_some_and(|rows| rows.is_empty())
        && listed
        && first.body.root().nth(0).get("id").u64() == all.body.root().nth(0).get("id").u64()
        && second.body.root().nth(0).get("id").u64() == all.body.root().nth(1).get("id").u64()
        && first.body.root().nth(0).get("id").u64() == repeat.body.root().nth(0).get("id").u64()
        && first.body.root().nth(0).get("id").u64() != second.body.root().nth(0).get("id").u64();
    let mut facts = Vec::new();
    for (name, holds) in [
        ("setup-user-repos-lists-owned-collaborating-and-team-repositories", listed),
        ("setup-user-repos-rows-carry-permissions-push", permissions),
        ("setup-user-repos-pages-by-page-and-limit", pages),
    ] {
        add(&mut facts, name, holds, "captured repository membership, permissions or pages differ");
    }
    Ok(facts)
}
