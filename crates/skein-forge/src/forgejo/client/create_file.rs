//! A contents API commit (forge-setup.md, 4.3).
use super::{Encoded, Refusal, items::Target};
use crate::forgejo::{
    decode::{
        Malformed, Step,
        walk::{Move, Value},
    },
    document::Document,
    id::ObjectId,
    limits::Limits,
    request::{Author, Method, Request},
};
use skein_json::{
    tokenizer,
    writer::{self, Encoder},
};
use skein_lib::{
    Writer,
    base64::{self, Alphabet},
};

pub(super) fn encode(request: &Request, limits: &Limits) -> Result<Encoded, Refusal> {
    let Request::CreateFile { at, path, content, .. } = request else { unreachable!("contents request") };
    let mut measure = Target { length: 0, writer: None };
    measure.repo(at);
    measure.put(b"/contents/");
    measure.file(path);
    if measure.length > usize::try_from(limits.target_bytes).expect("u32 fits") {
        return Err(Refusal::TooLong);
    }
    let length = base64::encoded_len(Alphabet::Standard, u32::try_from(content.len()).or(Err(Refusal::TooLong))?)
        .ok_or(Refusal::TooLong)?;
    if length > limits.writer.length {
        return Err(Refusal::TooLong);
    }
    let mut encoded = Writer::new(usize::try_from(length).expect("u32 fits"));
    base64::encode(Alphabet::Standard, content, &mut encoded).expect("measured base64");
    let encoded = encoded.finish();
    let mut json = Encoder::measure(&limits.writer);
    body(&mut json, request, &encoded);
    let length = match json.measured() {
        Ok(length) => length,
        Err(writer::Refusal::Text) => return Err(Refusal::Text),
        Err(writer::Refusal::TooLong | writer::Refusal::TooDeep | writer::Refusal::Number) => {
            return Err(Refusal::TooLong);
        }
    };
    let mut json = Encoder::write(length, &limits.writer);
    body(&mut json, request, &encoded);
    let mut target = Target { length: 0, writer: Some(Writer::new(measure.length)) };
    target.repo(at);
    target.put(b"/contents/");
    target.file(path);
    Ok(Encoded {
        method: Method::Post,
        target: target.writer.expect("writing pass").finish(),
        body: Some(json.finish()),
    })
}
fn body(json: &mut Encoder, request: &Request, encoded: &[u8]) {
    let Request::CreateFile { branch, message, author, .. } = request else { unreachable!("contents body") };
    json.object_start();
    json.key(b"content");
    json.string(encoded);
    json.key(b"branch");
    json.string(branch);
    json.key(b"message");
    json.string(message);
    json.key(b"author");
    signature(json, author);
    json.key(b"committer");
    signature(json, author);
    json.object_end();
}
fn signature(json: &mut Encoder, author: &Author) {
    json.object_start();
    json.key(b"name");
    json.string(&author.name);
    json.key(b"email");
    json.string(&author.email);
    json.object_end();
}
pub(super) fn step(commit: &mut Option<ObjectId>, movement: Move) -> Step {
    match movement {
        Move::Demand(demand) => return Step::Demand(demand),
        Move::Scalar { value: Value::Id(id), .. } => *commit = id,
        Move::Scalar {
            key,
            value:
                Value::Input(_) | Value::Number(_) | Value::Text(_) | Value::Boolean(_) | Value::Body(_) | Value::Time(_),
            ..
        } => return Step::Malformed(Malformed::Shape { field: key }),
        Move::End(_) => (),
        Move::Done => return Step::Done(Document::CreatedFile { commit: commit.expect("required commit sha") }),
    }
    Step::Demand(tokenizer::Request::Next)
}
