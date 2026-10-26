//! The dialect's plans against the in-process fake; no transport or live forge.
use super::fixtures as f;
use crate::service::Response;
use skein_fake_forge_domain::{self as fake, api as a};
use skein_forge::{
    api,
    github::{
        self,
        document::Kind,
        header::Head,
        plans::{
            Body, Heard,
            plan::{Bounds, Step},
            reads::Plan,
            tables::Tables,
        },
        request::Request,
    },
};
use skein_lib::{List, ReplyTo, Secret, Token};

pub fn domain_config() -> fake::Config {
    let mut config = f::domain_config();
    config.limits.items = 8;
    config.limits.comments = 4;
    config.limits.reviews = 4;
    config.limits.files = 4;
    config
}
pub fn harness() -> f::Harness {
    f::Harness::with_domain(f::limits(), f::config(), domain_config())
}
pub fn call(h: &mut f::Harness, op: a::Op) -> a::Answer {
    call_as(h, 1, op)
}
pub fn call_as(h: &mut f::Harness, user: u64, op: a::Op) -> a::Answer {
    let result = h
        .drive(fake::Event::Call {
            reply_to: ReplyTo::new(Token::new(77)),
            user,
            repository: f::boxed(b"acme/widgets"),
            op,
        })
        .expect("scenario call succeeds");
    h.hooks = 0;
    result
}
pub fn issue(h: &mut f::Harness, title: &[u8]) -> u64 {
    let a::Answer::Created(number) = call(
        h,
        a::Op::Write(a::Write::CreateIssue { title: f::boxed(title), body: f::boxed(b"body"), labels: Box::from([]) }),
    ) else {
        panic!("created issue")
    };
    number
}
pub fn token(h: &mut f::Harness) -> Secret {
    f::bearer(h.mint(1, false).token.expose())
}
pub fn bounds() -> Bounds {
    Bounds { inner: 8, page_size: 2, rows: 8, answer_bytes: 8192, body_bytes: 4096 }
}
pub fn events(h: &mut f::Harness, request: &Request, token: &Secret) -> List<Heard> {
    let encoded = github::request::encode(request, &h.limits.github).expect("dialect encodes request");
    let response = h.ask(encoded.method, &encoded.target, Some(token.expose()), encoded.body.as_deref().unwrap_or(b""));
    response_events(h, request, response)
}
pub fn response_events(h: &f::Harness, request: &Request, response: Response) -> List<Heard> {
    let mut events = List::with_capacity(32);
    let mut head = Head::new();
    head.date = Some(skein_lib::Wall::from_nanos(fake::time(&h.env.limits, h.env.now).as_nanos()));
    if let Some(link) = &response.link {
        github::header::field(&mut head, b"Link", link, &h.limits.github);
    }
    if response.status == 204 {
        assert!(response.body.is_empty(), "204 has no document");
        events.push(Heard::Head { status: response.status, head }).expect("head room");
        events.push(Heard::Done(Body::Empty)).expect("terminal room");
        return events;
    }
    if response.status < 300 && github::decode::kind(request).is_none() {
        events.push(Heard::Head { status: response.status, head }).expect("head room");
        events.push(Heard::Done(Body::Raw { bytes: response.body, more: false })).expect("raw room");
        return events;
    }
    let kind = if response.status < 300 { github::decode::kind(request).expect("JSON read") } else { Kind::Error };
    let (rows, document) = h.decode(kind, &response);
    events.push(Heard::Head { status: response.status, head }).expect("head room");
    for row in rows.into_boxed() {
        events.push(Heard::Row(row)).expect("row room");
    }
    events.push(Heard::Done(Body::Document(document))).expect("terminal room");
    events
}
pub fn read(h: &mut f::Harness, token: &Secret, read: api::Read) -> Result<api::Answer, api::Error> {
    let mut plan = Plan::new(api::Repository { forge: 1, repository: 2 }, read);
    let mut tables = Tables::new(4, 8);
    let mut step = plan.begin(bounds(), b"example-app", &mut tables).expect("begin read plan");
    for _exchange in 0..64_u32 {
        let exchange = match step {
            Step::Done(result) => return result,
            Step::Redirect { .. } => panic!("this plan uses only API exchanges"),
            Step::Send(exchange) => exchange,
        };
        let events = exchange_events(h, &exchange, token);
        let mut next = None;
        for event in events.into_boxed() {
            next = plan.heard(event, bounds(), b"example-app", &mut tables).expect("ordered exchange");
            if next.is_some() {
                break;
            }
        }
        step = next.expect("exchange advances plan");
    }
    panic!("plan settles within fixture bound")
}

pub fn exchange_events(h: &mut f::Harness, exchange: &github::plans::plan::Exchange, token: &Secret) -> List<Heard> {
    let encoded = github::request::encode(&exchange.request, &h.limits.github).expect("encoded exchange");
    let response = h.ask(encoded.method, &encoded.target, Some(token.expose()), encoded.body.as_deref().unwrap_or(b""));
    if response.status != 200 {
        return response_events(h, &exchange.request, response);
    }
    if exchange.comparison == github::plans::plan::Comparison::MergeBase {
        let (rows, document) =
            f::decode_with(github::decode::merge_base_decoder(&h.limits.github), &response.body, &h.limits.github);
        assert_eq!(rows.len(), 0);
        let mut events = List::with_capacity(2);
        events.push(Heard::Head { status: 200, head: Head::new() }).unwrap();
        events.push(Heard::Done(Body::Document(document))).unwrap();
        return events;
    }
    let mut events = response_events(h, &exchange.request, response);
    if let Some(retention) = exchange.retention {
        for index in 0..events.len() {
            let event = events.get_mut(index).unwrap();
            if let Heard::Done(Body::Raw { bytes, more }) = event {
                let cap = match retention {
                    github::plans::plan::Retention::Head { bytes }
                    | github::plans::plan::Retention::Whole { bytes }
                    | github::plans::plan::Retention::Tail { bytes } => usize::try_from(bytes).unwrap(),
                };
                if bytes.len() > cap {
                    *more = true;
                    *bytes = f::boxed(bytes.get(..cap).unwrap());
                }
            }
        }
    }
    events
}
pub fn object(count: u64) -> api::Commit {
    skein_fake_forge_protocol::routes::object(count).bytes
}
pub fn pull(h: &mut f::Harness) -> (u64, u64) {
    let first = h.first;
    let tree = Box::from([
        a::File { path: f::boxed(b"one"), content: f::boxed(b"one contents") },
        a::File { path: f::boxed(b"two"), content: f::boxed(b"two contents") },
        a::File { path: f::boxed(b"three"), content: f::boxed(b"three contents") },
    ]);
    let commit = fake::commit(&mut h.domain, &h.env.limits, first, tree, b"feature").unwrap().unwrap();
    call(h, a::Op::Git(a::Git::Push { branch: f::boxed(b"feature"), commit, expected: None }));
    let a::Answer::Created(number) = call(
        h,
        a::Op::Write(a::Write::OpenPull {
            title: f::boxed(b"feature"),
            body: f::boxed(b"pull body"),
            head: f::boxed(b"feature"),
            base: f::boxed(b"trunk"),
            opens_empty: false,
        }),
    ) else {
        panic!("created pull")
    };
    (number, commit)
}
