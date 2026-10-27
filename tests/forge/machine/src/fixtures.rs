//! The component's small limits and configured forge.
use skein_forge_connection::Limits;
use skein_lib::Duration;
pub const WRITER: u64 = 2;
#[must_use]
pub fn limits() -> Limits {
    Limits {
        forges: 2,
        token_bytes: 64,
        key_bytes: 1200,
        connections: 2,
        head_bytes: 4096,
        headers: 32,
        body_bytes_in: 1_048_576,
        depth: 16,
        chunk: 1024,
        string_bytes: 128,
        body_bytes: 1042,
        inner: 8,
        target_bytes: 1024,
        repositories: 2,
        users: 8,
        installations: 3,
        log_origins: 2,
        deadline: Duration::from_secs(10),
        idle: Duration::from_secs(5),
        start_backoff: Duration::from_secs(1),
        start_backoff_max: Duration::from_secs(8),
        renew_before: Duration::from_secs(60),
        // Beyond the token-renewal stories; log routes await the fake.
        log_poll: Duration::from_secs(86_400),
        log_catch_up: Duration::from_secs(3600),
        log_pages: 10,
        delivery_bytes: 65_536,
        hooks: 4,
        hook_head_bytes: 8192,
        hook_body_bytes: 1_048_576,
        hook_deadline: Duration::from_secs(10),
        hook_secret_bytes: 128,
        tls: skein_tls::client::Limits { read: 16384, send: 16384, records: skein_tls::client::MAX_RECORD },
    }
}

use core::net::{Ipv4Addr, SocketAddr};
use skein_forge::api;
use skein_forge_connection::{
    Access, Settings, Shared,
    settings::{Forge, Kind, Transport},
};
use skein_lib::{List, Secret};
pub const FORGE: u16 = 1;
pub const REPO: api::Repository = api::Repository { forge: 1, repository: 1 };
pub const TOKEN: &[u8] = b"0123456789abcdef0123456789abcdef01234567";
pub const TOKEN_B: &[u8] = b"fedcba9876543210fedcba9876543210fedcba98";
#[must_use]
pub fn access(token: &[u8], user: u64, secret: Option<&[u8]>) -> Access {
    Access {
        credential: skein_forge::credential::Credential::Token(Secret::new(Box::from(token))),
        user,
        webhook_secret: secret.map(|secret| Secret::new(Box::from(secret))),
    }
}
#[must_use]
pub fn unconnected() -> Settings {
    let mut settings = settings();
    settings.forges.get_mut(0).expect("forge").access = None;
    settings
}
#[must_use]
pub fn shared() -> Shared {
    Shared {
        calls: 2,
        rows: 8,
        answer_bytes: 65_536,
        op_bytes: 512,
        lifetime: Duration::from_secs(10),
        name_bytes: 128,
        repositories: 2,
        io: skein_io::Limits {
            sockets: 8,
            names: 0,
            refusals: 2,
            intake: 32_768,
            receive: 4096,
            output: 32_768,
            sends: 4,
            accepts: 1,
            backlog: 1,
            close_timeout: Duration::from_secs(1),
            retry: Duration::from_millis(10),
        },
    }
}
#[must_use]
pub fn settings() -> Settings {
    let mut forges = List::with_capacity(2);
    forges
        .push(Forge {
            number: FORGE,
            kind: Kind::Forgejo,
            log_origins: List::with_capacity(0),
            address: skein_io::Addr::Ip(SocketAddr::from((Ipv4Addr::LOCALHOST, 3000))),
            transport: Transport::Plaintext { name: None },
            access: Some(access(TOKEN, WRITER, None)),
        })
        .expect("one forge");
    Settings {
        forges,
        webhooks: None,
        word: Box::from(b"kit".as_slice()),
        user_agent: Box::from(b"kit/0.0.0".as_slice()),
    }
}
/// Makes a literal HTTP answer for scenarios independent of recorded cases.
#[must_use]
pub fn response(code: u16, body: &[u8]) -> Vec<u8> {
    let mut bytes = format!("HTTP/1.1 {code} Answer\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
    bytes.extend_from_slice(body);
    bytes
}
/// An Items listing supplies the time of its own snapshot (forge.md, 5.3).
#[must_use]
pub fn dated_response(code: u16, body: &[u8]) -> Vec<u8> {
    let mut bytes = format!(
        "HTTP/1.1 {code} Answer\r\nDate: Thu, 01 Jan 2026 00:00:00 GMT\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    bytes.extend_from_slice(body);
    bytes
}
/// One branch read after its repository resolution.
#[must_use]
pub fn branch() -> skein_forge::api::Op {
    api::Op::Read(api::Read::Branch { branch: Box::from(*b"main") })
}

/// The fake's HTTP and document bounds, used by the kit worlds.
#[must_use]
pub fn fake_limits() -> skein_fake_forge_protocol::Limits {
    skein_fake_forge_protocol::Limits {
        calls: 4,
        connections: 4,
        raw_bytes: 4096,
        http: skein_http::server::Limits {
            head: 4096,
            headers: 16,
            body: 16384,
            read: 1024,
            response: 4096,
            send: 1024,
        },
        documents: skein_forge::forgejo::limits::Limits {
            json: skein_json::tokenizer::Limits { chunk: 1024, string: 16384, number: 32, depth: 16, length: 65536 },
            writer: skein_json::writer::Limits { depth: 16, length: 65536 },
            string_bytes: 512,
            body_bytes: 16384,
            inner: 32,
            target_bytes: 4096,
        },
    }
}
/// The token's writer is user 1; the scenario's repository is id 2.
#[must_use]
pub fn protocol_config() -> skein_fake_forge_protocol::Config {
    use skein_fake_forge_protocol::{Config, Faults, Repository};
    let mut tokens = List::with_capacity(1);
    tokens.push((Secret::new(Box::from(TOKEN)), 1)).expect("one token");
    let mut users = List::with_capacity(2);
    users.push((1, Box::from(b"engine".as_slice()))).expect("user");
    users.push((2, Box::from(b"person".as_slice()))).expect("user");
    let mut repositories = List::with_capacity(1);
    repositories.push(Repository { id: 2, full: Box::from(b"org/repo".as_slice()), protected: None }).expect("repo");
    Config {
        base: Box::from(b"http://127.0.0.1:3000".as_slice()),
        tokens,
        users,
        repositories,
        max_response_items: 2,
        default_paging_num: 30,
        hooks: None,
        faults: Faults { reset: 0, drop: 0, server_error: 0 },
    }
}

/// The kit world's hook fixture keeps four connections and a ten-second request deadline.
#[must_use]
pub fn hook_limits() -> Limits {
    Limits {
        hooks: 4,
        hook_head_bytes: 8192,
        hook_body_bytes: 1_048_576,
        hook_deadline: Duration::from_secs(10),
        hook_secret_bytes: 128,
        ..limits()
    }
}
pub const HOOK_SECRET: &[u8] = b"fixture-hook-secret";
pub const PING: &[u8] = b"{\"zen\":\"a delivery of no row\"}";
#[must_use]
pub fn hook_settings() -> Settings {
    let mut settings = settings();
    settings.forges.get_mut(0).expect("forge").access.as_mut().expect("fixture access").webhook_secret =
        Some(Secret::new(Box::from(HOOK_SECRET)));
    settings.webhooks =
        Some(skein_forge_connection::hooks::Webhooks { listener: skein_io::Addr::Ip(([127, 0, 0, 1], 7200).into()) });
    settings
}
#[must_use]
pub fn hooked_protocol_config() -> skein_fake_forge_protocol::Config {
    let mut config = protocol_config();
    config.hooks = Some(skein_fake_forge_protocol::delivery::Endpoint {
        forge: FORGE,
        host: Box::from(b"127.0.0.1:7200".as_slice()),
        secret: Secret::new(Box::from(HOOK_SECRET)),
    });
    config
}
#[must_use]
pub fn push_hook(branch: &[u8], commit: u64) -> skein_fake_forge_domain::Request {
    use skein_fake_forge_domain::{Request, api::Change};
    Request::Hook {
        queue: None,
        repository: Box::from(b"org/repo".as_slice()),
        change: Change::Push,
        number: None,
        branch: Some(Box::from(branch)),
        commit: Some(commit),
        by: Some(2),
        body: None,
        ended: None,
    }
}

/// The independent fake domain used by the joined component world.
#[must_use]
pub fn fake_config() -> skein_fake_forge_domain::Config {
    skein_fake_forge_domain::Config {
        limits: skein_fake_forge_domain::Limits {
            repositories: 1,
            users: 3,
            bypassing_users: 4,
            labels: 2,
            items: 64,
            comments: 32,
            dependencies: 2,
            reviews: 8,
            branches: 4,
            commits: 16,
            files: 4,
            statuses: 4,
            contexts: 4,
            workflows: 0,
            workflow_runs: 0,
            pages: 1,
            // A merge queue branch, gh-readonly-queue/<base>/pr-<n>-<sha>, is
            // 68 bytes here: the bound leaves it room.
            name_bytes: 96,
            title_bytes: 64,
            body_bytes: 1024,
            content_bytes: 128,
            page_size: 2,
            calls: 4,
            hooks: 8,
            observations: 64,
        },
        latency_min: Duration::from_millis(10),
        latency_max: Duration::from_millis(50),
        settle_min: Duration::from_secs(2),
        settle_max: Duration::from_secs(2),
        late: 0,
        late_min: Duration::from_secs(1),
        late_max: Duration::from_secs(3),
        unavailable: 0,
        timeouts: 0,
        landing: 0,
        land_min: Duration::from_secs(1),
        land_max: Duration::from_secs(3),
        rate_limit: 0,
        rate_window: Duration::from_secs(10),
        ci: 3,
        setup_faults: 0,
        lost: 0,
        hook_min: Duration::ZERO,
        hook_max: Duration::ZERO,
        hooks_late: 0,
        hooks_lost: 0,
        resolution: Duration::from_secs(1),
        skew: skein_fake_forge_domain::Skew::Ahead(Duration::from_secs(20)),
        status_updates: false,
        edit_updates: false,
    }
}

/// The first call has no repository-resolution prelude.
#[must_use]
pub fn locate() -> api::Op {
    api::Op::Read(api::Read::Locate { owner: Box::from(&b"fixture"[..]), name: Box::from(&b"specimen"[..]) })
}
pub const LOCATE: &str = "GET /api/v1/repos/fixture/specimen";
/// A small generated startup and locate exchange for byte sweeps.
#[must_use]
pub fn locate_script() -> Vec<crate::machine::Answer> {
    use crate::machine::{Answer, Response};
    [
        ("GET /api/v1/user", br#"{"id":2,"login":"writer"}"#.as_slice()),
        ("GET /api/v1/settings/api", br#"{"max_response_items":2,"default_paging_num":30}"#.as_slice()),
        (LOCATE, br#"{"id":1,"owner":{"id":1,"login":"fixture"},"name":"specimen","object_format_name":"sha1","default_branch":"main","default_merge_style":"merge","allow_merge_commits":true,"allow_squash_merge":true,"allow_rebase":true,"html_url":"http://127.0.0.1:3000/fixture/specimen"}"#.as_slice()),
    ].into_iter().map(|(request, body)| Answer { request, response: Response::Bytes(response(200, body)) }).collect()
}

/// Settings for the fake GitHub App, awaiting the owner's records and Connect.
#[must_use]
pub fn github_settings() -> Settings {
    let mut settings = unconnected();
    settings.forges.get_mut(0).expect("forge").kind = Kind::GitHub;
    settings
}

/// The fake GitHub App's public test key and bot identity.
#[must_use]
pub fn github_access() -> Access {
    Access {
        credential: skein_forge::credential::Credential::App {
            id: 7,
            key: Secret::new(Box::from(crate::github::app_key())),
        },
        user: 9,
        webhook_secret: None,
    }
}
