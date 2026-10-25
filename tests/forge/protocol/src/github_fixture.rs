//! GitHub-only scenario tables; common bounds, streams and public App key come from the machine world.
use skein_fake_forge_github::{
    app::App,
    installations::{self, Kind, Reach},
    pages,
    service::{Account, AppFacts, Config, Limits, Row, Service, Shown},
};
use skein_forge::credential::Credential;
use skein_forge_connection::{Access, Settings, settings::Kind as Dialect};
use skein_forge_machine_world::{fixtures, github};
use skein_lib::{Duration, List, Secret, Wall};

pub(crate) fn limits(log_poll: Duration) -> skein_forge_connection::Limits {
    skein_forge_connection::Limits { log_poll, ..crate::fixtures::limits() }
}

pub(crate) const PATH: &[u8] = b"acme/widgets";
pub(crate) const USER: u64 = 9;

pub(crate) fn access(key: &[u8]) -> Access {
    Access { credential: Credential::App { id: 7, key: Secret::new(Box::from(key)) }, user: USER, webhook_secret: None }
}

pub(crate) fn settings() -> Settings {
    let mut settings = fixtures::unconnected();
    settings.forges.get_mut(0).expect("one forge").kind = Dialect::GitHub;
    settings
}

// The machine world's service and domain are private, so these are the world's
// minimal scenario tables rather than a copy of its three-repository fixture.
pub(crate) fn service(seed: u64, key: &'static [u8]) -> Service {
    let mut repositories = List::with_capacity(1);
    repositories
        .push(Row {
            ci_setup: skein_fake_forge_github::service::CiSetup::DEFAULT,
            id: 2,
            owner: Box::from(b"acme".as_slice()),
            owner_id: 22,
            organization: true,
            name: Box::from(b"widgets".as_slice()),
            private: true,
        })
        .expect("one repository");
    let mut accounts = List::with_capacity(1);
    accounts.push(Account { login: Box::from(b"acme".as_slice()), id: 22, organization: true }).expect("one account");
    let documents = fixtures::fake_limits().documents;
    let limits = Limits {
        deliveries: skein_fake_forge_github::delivery::Limits { entries: 64, payload_bytes: 32768 },
        http: fixtures::fake_limits().http,
        github: skein_forge::github::limits::Limits {
            json: documents.json,
            writer: documents.writer,
            string_bytes: documents.string_bytes,
            body_bytes: documents.body_bytes,
            inner: documents.inner,
            target_bytes: documents.target_bytes,
        },
        installations: installations::Limits { installations: 1, tokens: 8, listed: 1 },
        pages: pages::Limits { states: 1, manifest_bytes: 2048 },
        page: 2,
        body: 4096,
        response: 16384,
        blob_records: 16,
        git_records: 16,
    };
    let mut service = Service::new(
        Config {
            deliveries: skein_fake_forge_github::delivery::Config::standard(),
            rate_quota: 5000,
            rate_window: skein_lib::Duration::from_secs(3600),
            token_life: skein_fake_forge_github::installations::TOKEN_LIFE,
            base: Box::from(b"http://127.0.0.1:3000".as_slice()),
            blob: Box::from(b"http://logs.example.test".as_slice()),
            third_origin: None,
            blob_redirect: None,
            app: App {
                id: 7,
                slug: Box::from(b"test-app".as_slice()),
                owner: 1,
                permissions: skein_fake_forge_github::app::wanted(),
                events: Box::from([]),
                key,
                webhook: Secret::new(Box::from(b"test-webhook".as_slice())),
            },
            app_facts: AppFacts::Complete,
            shown: Shown::Ruleset,
            ruleset_id: 17,
            pull_requests_only: false,
            required_signatures: false,
            tokens: List::with_capacity(0),
            faults: skein_fake_forge_github::Faults { reset: 0, drop: 0, server_error: 0 },
            bot: USER,
            hook: Box::from(b"https://hook.test/start".as_slice()),
            repositories,
            accounts,
            person: Box::from(b"acme".as_slice()),
        },
        &limits,
        seed,
    )
    .expect("GitHub scenario");
    assert_eq!(service.installations_mut().install(22, b"acme", Kind::Organization, Reach::All, Wall::EPOCH), Ok(1));
    service
}

pub(crate) fn key() -> &'static [u8] {
    github::app_key()
}
