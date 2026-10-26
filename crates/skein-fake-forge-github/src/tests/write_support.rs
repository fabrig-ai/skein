//! Write plans use the same in-process exchanges and readback as read plans.
use super::{fixtures as f, read_support as r};
use skein_forge::{
    api,
    github::plans::{plan::Step, tables::Tables, writes::Plan},
};
use skein_lib::Secret;

pub fn harness() -> f::Harness {
    let mut h = r::harness();
    skein_fake_forge_domain::grant(
        &mut h.domain,
        b"acme/widgets",
        f::BOT,
        skein_fake_forge_domain::api::Permission::Write,
    );
    h
}
pub fn write(h: &mut f::Harness, token: &Secret, write: api::Write) -> Result<api::Answer, api::Error> {
    write_after_push(h, token, write, None)
}
pub fn write_after_push(
    h: &mut f::Harness,
    token: &Secret,
    write: api::Write,
    moved: Option<u64>,
) -> Result<api::Answer, api::Error> {
    drive(h, token, write, moved, None)
}
pub fn write_with_settings(
    h: &mut f::Harness,
    token: &Secret,
    write: api::Write,
    settings: skein_fake_forge_domain::api::Settings,
) -> Result<api::Answer, api::Error> {
    drive(h, token, write, None, Some(settings))
}
fn drive(
    h: &mut f::Harness,
    token: &Secret,
    write: api::Write,
    moved: Option<u64>,
    settings: Option<skein_fake_forge_domain::api::Settings>,
) -> Result<api::Answer, api::Error> {
    let mut plan = Plan::new(api::Repository { forge: 1, repository: 2 }, write);
    let mut tables = Tables::new(4, 8);
    let mut step = plan.begin(r::bounds(), b"example-app", &mut tables).expect("begin write");
    for _exchange in 0..32_u32 {
        let exchange = match step {
            Step::Done(result) => return result,
            Step::Redirect { .. } => panic!("this plan uses only API exchanges"),
            Step::Send(exchange) => exchange,
        };
        if let skein_forge::github::request::Request::Update { .. } = &exchange.request
            && let Some(commit) = moved
        {
            r::call(
                h,
                skein_fake_forge_domain::api::Op::Git(skein_fake_forge_domain::api::Git::Push {
                    branch: f::boxed(b"feature"),
                    commit,
                    expected: None,
                }),
            );
        }
        if let skein_forge::github::request::Request::Merge { .. } = &exchange.request
            && let Some(settings) = &settings
        {
            skein_fake_forge_domain::settings(&mut h.domain, b"acme/widgets", settings.clone());
        }
        let events = r::exchange_events(h, &exchange, token);
        let mut next = None;
        for event in events.into_boxed() {
            next = plan.heard(event, r::bounds(), b"example-app", &mut tables).expect("ordered write exchange");
            if next.is_some() {
                break;
            }
        }
        step = next.expect("exchange advances");
    }
    panic!("bounded write plan settles")
}

pub fn ask(
    h: &mut f::Harness,
    token: &Secret,
    request: &skein_forge::github::request::Request,
) -> crate::service::Response {
    let encoded = skein_forge::github::request::encode(request, &h.limits.github).unwrap();
    h.ask(encoded.method, &encoded.target, Some(token.expose()), encoded.body.as_deref().unwrap_or(b""))
}
