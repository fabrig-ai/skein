//! Frozen terminal ownership and bounded observation payloads.
use super::layer as protocol;
use super::{Connection, Forge, Observation, Peer};
use skein_fake_forge_domain::{self as domain, api};
use skein_lib::{Env, ReplyTo};
use std::mem::size_of_val;

impl Peer {
    pub(super) fn observe(&mut self, record: Observation, bytes: u64) {
        self.reserve(bytes);
        self.observed_bytes = self.observed_bytes.checked_add(bytes).expect("bounded frozen bytes");
        self.observations.push(record);
    }
    fn reserve(&self, bytes: u64) {
        assert!(
            self.observations.len() < usize::try_from(self.face.limits.observations).expect("u32 fits"),
            "frozen record count fits"
        );
        assert!(
            self.observed_bytes
                .checked_add(bytes)
                .is_some_and(|total| total <= u64::from(self.face.limits.observation_bytes)),
            "frozen payload admitted before copying"
        );
    }
    pub(super) fn capture(&mut self, connection: &mut Connection) {
        if let Some(call) = connection.server.take_head() {
            if self.kept.heads {
                let bytes = sum(&[call.target.heap(), call.headers.heap()]).expect("bounded HTTP head");
                self.observe(Observation::Head { connection: connection.owner, call }, bytes);
            }
            connection.head_seen = true;
        }
    }
    pub(super) fn route(&mut self, forge: &mut Forge, env: &Env<protocol::Limits>) {
        for _ in 0..forge.pending.len() {
            let reply = forge.pending.pop().expect("pending pass length");
            let (token, reply) = match reply {
                domain::Request::Reply { to, result } => {
                    let token = to.into_token();
                    (token, domain::Request::Reply { to: ReplyTo::new(token), result })
                }
                hook @ domain::Request::Hook { .. } => {
                    if self.service.is_github() {
                        self.github_hook(&hook, env.now, env.wall);
                    } else {
                        self.deliver(&hook);
                    }
                    continue;
                }
            };
            let Some(index) = self.calls.iter().position(|call| call.token == token) else {
                forge.pending.push(reply);
                continue;
            };
            let owner = self.calls[index].connection;
            let target = self.connections.iter().position(|connection| connection.owner == owner);
            if let Some(target) = target {
                let connection = &self.connections[target];
                if connection.above.room() < protocol::MAX_UP || connection.below.room() < protocol::MAX_DOWN {
                    forge.pending.push(reply);
                    continue;
                }
            }
            let call = self.calls.remove(index);
            let domain::Request::Reply { result, .. } = &reply else { unreachable!("reply snapshot") };
            if let Some((repository, op)) = call.snapshot {
                let bytes = sum(&[repository.heap(), op.heap(), result.heap()]).expect("bounded frozen call");
                self.reserve(bytes);
                self.observe(
                    Observation::Call {
                        connection: target.map(|_| owner),
                        user: call.user,
                        repository,
                        op,
                        result: result.clone(),
                    },
                    bytes,
                );
            }
            // A retired HTTP call still consumes its snapshot; an unrelated reply stays above.
            let domain::Request::Reply { result, .. } = reply else { unreachable!("reply terminal") };
            let reply = domain::Request::Reply { to: ReplyTo::new(call.local), result };
            if let Some((owner, reply)) = self.service.target(reply)
                && let Some(index) = self.connections.iter().position(|connection| connection.owner == owner)
            {
                let mut connection = self.connections.remove(index);
                protocol::down(
                    &mut connection.server,
                    &mut self.service,
                    env,
                    reply,
                    &forge.domain,
                    &mut connection.above,
                    &mut connection.below,
                );
                self.capture(&mut connection);
                self.connections.insert(index, connection);
            }
        }
    }
}

// The byte cap counts owned arrays and their nested buffers, not inline enum fields.
trait Heap {
    fn heap(&self) -> Option<u64>;
}
fn sum(values: &[Option<u64>]) -> Option<u64> {
    values.iter().try_fold(0u64, |total, next| total.checked_add((*next)?))
}
impl<T: Heap> Heap for Box<T>
where
    T: ?Sized,
{
    fn heap(&self) -> Option<u64> {
        u64::try_from(size_of_val(&**self)).ok()?.checked_add((**self).heap()?)
    }
}
impl<T: Heap> Heap for [T] {
    fn heap(&self) -> Option<u64> {
        self.iter().try_fold(0u64, |total, next| total.checked_add(next.heap()?))
    }
}
impl<T: Heap> Heap for Option<T> {
    fn heap(&self) -> Option<u64> {
        self.as_ref().map_or(Some(0), Heap::heap)
    }
}
impl<T: Heap, E> Heap for Result<T, E> {
    fn heap(&self) -> Option<u64> {
        match self {
            Ok(value) => value.heap(),
            Err(_) => Some(0),
        }
    }
}
impl Heap for u8 {
    fn heap(&self) -> Option<u64> {
        Some(0)
    }
}
impl Heap for u64 {
    fn heap(&self) -> Option<u64> {
        Some(0)
    }
}
impl Heap for skein_http::Header {
    fn heap(&self) -> Option<u64> {
        sum(&[self.name.heap(), self.value.heap()])
    }
}
impl Heap for api::Op {
    fn heap(&self) -> Option<u64> {
        match self {
            Self::Read(read) => read.heap(),
            Self::Write(write) => write.heap(),
            Self::Git(git) => git.heap(),
        }
    }
}
impl Heap for api::Read {
    fn heap(&self) -> Option<u64> {
        match self {
            Self::Items { labels, .. } => labels.heap(),
            Self::FirstPull { head, base } | Self::PullFor { head, base } => sum(&[head.heap(), base.heap()]),
            Self::Protection { branch } | Self::Branch { branch } => branch.heap(),
            Self::File { path, .. } => path.heap(),
            Self::Pages { after } => after.heap(),
            Self::Runs { workflow, .. } => workflow.heap(),
            Self::Page { name } => name.heap(),
            Self::Locate
            | Self::Item { .. }
            | Self::Comment { .. }
            | Self::Dependencies { .. }
            | Self::LabelIds
            | Self::Labels
            | Self::Pull { .. }
            | Self::PullFiles { .. }
            | Self::Compare { .. }
            | Self::Checks { .. }
            | Self::Job { .. }
            | Self::Log { .. }
            | Self::Run { .. }
            | Self::Jobs { .. }
            | Self::Comments { .. }
            | Self::OpenPulls { .. }
            | Self::Settings
            | Self::Collaborators
            | Self::Statuses { .. }
            | Self::Permission { .. }
            | Self::ListingPermission
            | Self::GitTree { .. }
            | Self::Tree { .. } => Some(0),
        }
    }
}
impl Heap for api::Write {
    fn heap(&self) -> Option<u64> {
        match self {
            Self::SetProtection { protection } => protection.heap(),
            Self::CreateIssue { title, body, labels } => sum(&[title.heap(), body.heap(), labels.heap()]),
            Self::Comment { body, .. } | Self::EditComment { body, .. } | Self::Review { body, .. } => body.heap(),
            Self::EditItem { title, body, .. } => sum(&[title.heap(), body.heap()]),
            Self::SetDependencies { dependencies, .. } => dependencies.heap(),
            Self::SetLabels { labels, .. } | Self::AddLabels { labels, .. } | Self::RemoveLabels { labels, .. } => {
                labels.heap()
            }
            Self::DefineLabel { name } | Self::DeletePage { name } => name.heap(),
            Self::OpenPull { title, body, head, base, .. } => {
                sum(&[title.heap(), body.heap(), head.heap(), base.heap()])
            }
            Self::RequestReviewers { reviewers, .. }
            | Self::RemoveReviewers { reviewers, .. }
            | Self::SetReviewers { reviewers, .. } => reviewers.heap(),
            Self::CreateBranch { branch, .. } | Self::DeleteBranch { branch } => branch.heap(),
            Self::Status { context, description, link, .. } => sum(&[context.heap(), description.heap(), link.heap()]),
            Self::Dispatch { workflow, branch, inputs } => sum(&[workflow.heap(), branch.heap(), inputs.heap()]),
            Self::PutPage { name, content } => sum(&[name.heap(), content.heap()]),
            Self::Rerun { .. }
            | Self::DeleteComment { .. }
            | Self::Submit { .. }
            | Self::Enqueue { .. }
            | Self::Merge { .. }
            | Self::Update { .. }
            | Self::Close { .. }
            | Self::Reopen { .. } => Some(0),
        }
    }
}
impl Heap for api::Git {
    fn heap(&self) -> Option<u64> {
        match self {
            Self::Clone | Self::Fetch { want: api::Want::Commit(_) | api::Want::Default } => Some(0),
            Self::Fetch { want: api::Want::Branch(branch) }
            | Self::Push { branch, .. }
            | Self::Create { branch, .. } => branch.heap(),
        }
    }
}
impl Heap for api::Answer {
    fn heap(&self) -> Option<u64> {
        match self {
            Self::Items { items, .. } => items.heap(),
            Self::Item { item, comments, .. } => sum(&[item.heap(), comments.heap()]),
            Self::Pull(pull) => pull.heap(),
            Self::Pulls(pulls) => pulls.heap(),
            Self::Comments(comments) => comments.heap(),
            Self::Runs(runs) => runs.heap(),
            Self::Jobs(jobs) => jobs.heap(),
            Self::Job { job, steps } => sum(&[job.heap(), steps.heap()]),
            Self::PullFiles { files, .. } => files.heap(),
            Self::Comparison { files, commits, .. } => sum(&[files.heap(), commits.heap()]),
            Self::Checks(checks) => checks.heap(),
            Self::Protection(protection) => protection.heap(),
            Self::Settings(settings) => settings.default.heap(),
            Self::Collaborators(collaborators) => collaborators.heap(),
            Self::Statuses(statuses) => statuses.heap(),
            Self::Comment { comment, .. } => comment.heap(),
            Self::Dependencies(numbers) => numbers.heap(),
            Self::Labels(labels) => labels.heap(),
            Self::LabelIds(labels) => labels.heap(),
            Self::Tree(files) => files.heap(),
            Self::GitTree { entries, .. } => entries.heap(),
            Self::File(bytes) => bytes.heap(),
            Self::Directory(entries) => entries.heap(),
            Self::Pages { pages, next } => sum(&[pages.heap(), next.heap()]),
            Self::Page(page) => sum(&[page.name.heap(), page.content.heap()]),
            Self::Cloned { default, branches } => sum(&[default.heap(), branches.heap()]),
            Self::Run(run) => run.heap(),
            Self::Located { .. }
            | Self::Permission(_)
            | Self::Commit(_)
            | Self::Created(_)
            | Self::Commented(_)
            | Self::Reviewed(_)
            | Self::Merged(_)
            | Self::Revision(_)
            | Self::Done
            | Self::Pushed(_)
            | Self::Branch(_) => Some(0),
        }
    }
}
impl Heap for api::Summary {
    fn heap(&self) -> Option<u64> {
        sum(&[self.title.heap(), self.body.heap(), self.labels.heap()])
    }
}
impl Heap for api::Comment {
    fn heap(&self) -> Option<u64> {
        self.body.heap()
    }
}
impl Heap for api::Pull {
    fn heap(&self) -> Option<u64> {
        sum(&[self.head.heap(), self.base.heap(), self.reviewers.heap(), self.reviews.heap(), self.statuses.heap()])
    }
}
impl Heap for api::Review {
    fn heap(&self) -> Option<u64> {
        self.body.heap()
    }
}
impl Heap for api::Status {
    fn heap(&self) -> Option<u64> {
        sum(&[self.context.heap(), self.description.heap(), self.link.heap()])
    }
}
impl Heap for api::ChangedFile {
    fn heap(&self) -> Option<u64> {
        sum(&[self.path.heap(), self.before.heap(), self.after.heap()])
    }
}
impl Heap for api::CheckSummary {
    fn heap(&self) -> Option<u64> {
        sum(&[self.status.heap(), self.description.heap(), self.link.heap()])
    }
}
impl Heap for api::Protection {
    fn heap(&self) -> Option<u64> {
        sum(&[self.branch.heap(), self.contexts.heap()])
    }
}
impl Heap for api::File {
    fn heap(&self) -> Option<u64> {
        sum(&[self.path.heap(), self.content.heap()])
    }
}
impl Heap for api::PageName {
    fn heap(&self) -> Option<u64> {
        self.name.heap()
    }
}
impl Heap for api::Head {
    fn heap(&self) -> Option<u64> {
        self.branch.heap()
    }
}
impl Heap for api::Run {
    fn heap(&self) -> Option<u64> {
        sum(&[self.workflow.heap(), self.inputs.heap()])
    }
}
impl Heap for api::Job {
    fn heap(&self) -> Option<u64> {
        sum(&[self.name.heap(), self.steps.heap()])
    }
}
impl Heap for api::WorkflowInput {
    fn heap(&self) -> Option<u64> {
        sum(&[self.0.heap(), self.1.heap()])
    }
}
impl Heap for api::TreeEntry {
    fn heap(&self) -> Option<u64> {
        self.name.heap()
    }
}
impl Heap for api::Step {
    fn heap(&self) -> Option<u64> {
        sum(&[self.name.heap(), self.log.heap()])
    }
}
impl Heap for api::Collaborator {
    fn heap(&self) -> Option<u64> {
        Some(0)
    }
}

impl Heap for api::Entry {
    fn heap(&self) -> Option<u64> {
        self.name.heap()
    }
}

impl Heap for api::Label {
    fn heap(&self) -> Option<u64> {
        self.name.heap()
    }
}
