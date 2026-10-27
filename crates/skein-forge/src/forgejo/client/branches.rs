use crate::forgejo::{
    decode::{
        Malformed, Step,
        walk::{Field, Kind, Move, Read, Value, Walk},
    },
    document::{self, Branch, BranchProtection, CollaboratorPermission, Document, Permission, Row, User},
    limits::Limits,
};
use alloc::boxed::Box;
use skein_lib::List;
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BranchKind {
    Branch,
    Branches,
    Refs,
    Ref,
    Commit,
    Protection,
    Protections,
    Contexts,
    Context,
    Permission,
}
const fn field(key: &'static [u8], read: Read) -> Field {
    Field { key, read, required: true }
}
pub(crate) fn fields(kind: BranchKind) -> &'static [Field] {
    match kind {
        BranchKind::Ref => const { &[field(b"ref", Read::Text)] },
        BranchKind::Branch => {
            const {
                &[
                    field(b"name", Read::Text),
                    field(b"commit", Read::Object(Kind::Branch(BranchKind::Commit))),
                    field(b"effective_branch_protection_name", Read::Text),
                ]
            }
        }
        BranchKind::Commit => const { &[field(b"id", Read::Id { empty: false })] },
        BranchKind::Protection => {
            const {
                &[
                    field(b"rule_name", Read::Text),
                    field(b"enable_status_check", Read::Boolean),
                    field(b"status_check_contexts", Read::Array(Kind::Branch(BranchKind::Contexts))),
                    field(b"required_approvals", Read::Number),
                    field(b"dismiss_stale_approvals", Read::Boolean),
                    field(b"require_signed_commits", Read::Boolean),
                ]
            }
        }
        BranchKind::Permission => {
            const { &[field(b"permission", Read::Text), field(b"user", Read::Object(Kind::User))] }
        }
        BranchKind::Branches
        | BranchKind::Refs
        | BranchKind::Protections
        | BranchKind::Contexts
        | BranchKind::Context => &[],
    }
}
pub(crate) fn element(kind: BranchKind) -> Option<Kind> {
    match kind {
        BranchKind::Refs => Some(Kind::Branch(BranchKind::Ref)),
        BranchKind::Branches => Some(Kind::Branch(BranchKind::Branch)),
        BranchKind::Protections => Some(Kind::Branch(BranchKind::Protection)),
        BranchKind::Contexts => Some(Kind::Branch(BranchKind::Context)),
        BranchKind::Branch
        | BranchKind::Ref
        | BranchKind::Protection
        | BranchKind::Commit
        | BranchKind::Context
        | BranchKind::Permission => None,
    }
}
fn branch() -> Branch {
    Branch {
        name: Box::from([]),
        commit: crate::forgejo::id::ObjectId { format: crate::forgejo::id::ObjectFormat::Sha1, bytes: [0; 32] },
        effective_branch_protection_name: None,
    }
}
fn protection() -> BranchProtection {
    BranchProtection {
        rule_name: Box::from([]),
        enable_status_check: false,
        status_check_contexts: Box::from([]),
        required_approvals: 0,
        dismiss_stale_approvals: false,
        require_signed_commits: false,
    }
}
pub(crate) fn permission(text: &[u8]) -> Option<Permission> {
    match text {
        b"none" => Some(Permission::None),
        b"read" => Some(Permission::Read),
        b"write" => Some(Permission::Write),
        b"admin" => Some(Permission::Admin),
        b"owner" => Some(Permission::Owner),
        _ => None,
    }
}
#[derive(Debug)]
pub(super) struct Branches {
    root: BranchKind,
    branch: Branch,
    protection: BranchProtection,
    user: User,
    permission: Permission,
    contexts: List<Box<[u8]>>,
    inner: u32,
}
impl Branches {
    pub(super) fn new(root: BranchKind, limits: &Limits) -> Self {
        Self {
            root,
            branch: branch(),
            protection: protection(),
            user: document::user(),
            permission: Permission::None,
            contexts: List::with_capacity(limits.inner),
            inner: limits.inner,
        }
    }
    pub(super) fn step(&mut self, movement: Move, walk: &Walk, limits: &Limits) -> Step {
        let result = match movement {
            Move::Demand(demand) => {
                return Step::Demand(if walk.parent() == Some(Kind::Branch(BranchKind::Contexts)) {
                    skein_json::tokenizer::Request::Text(limits.string_bytes)
                } else if walk.parent() == Some(Kind::Branch(BranchKind::Ref))
                    && let skein_json::tokenizer::Request::Text(_) = demand
                {
                    skein_json::tokenizer::Request::Text(
                        limits
                            .string_bytes
                            .saturating_add(u32::try_from(b"refs/heads/".len()).expect("namespace length fits")),
                    )
                } else {
                    demand
                });
            }
            Move::Scalar { kind: _, key, value } => self.scalar(key, value),
            Move::End(Kind::Branch(BranchKind::Branch | BranchKind::Ref)) => {
                if self.root == BranchKind::Branches || self.root == BranchKind::Refs {
                    return Step::Row(
                        Row::Branch(core::mem::replace(&mut self.branch, branch())),
                        skein_json::tokenizer::Request::Next,
                    );
                }
                Ok(())
            }
            Move::End(Kind::Branch(BranchKind::Protection)) => {
                let contexts = core::mem::replace(&mut self.contexts, List::with_capacity(self.inner));
                self.protection.status_check_contexts = contexts.into_boxed();
                if self.root == BranchKind::Protections {
                    return Step::Row(
                        Row::Protection(core::mem::replace(&mut self.protection, protection())),
                        skein_json::tokenizer::Request::Next,
                    );
                }
                Ok(())
            }
            Move::End(
                Kind::User
                | Kind::Branch(
                    BranchKind::Branches
                    | BranchKind::Refs
                    | BranchKind::Protections
                    | BranchKind::Commit
                    | BranchKind::Contexts
                    | BranchKind::Context
                    | BranchKind::Permission,
                ),
            ) => Ok(()),
            Move::End(
                Kind::Reachable(_)
                | Kind::Error
                | Kind::Write(_)
                | Kind::Settings
                | Kind::Repository
                | Kind::Search
                | Kind::Setup(_)
                | Kind::Users
                | Kind::File(_)
                | Kind::Action(_)
                | Kind::Item(_)
                | Kind::Delivery(_)
                | Kind::Review(_),
            ) => unreachable!("branch tables"),
            Move::Done => {
                return Step::Done(match self.root {
                    BranchKind::Branch => Document::Branch(core::mem::replace(&mut self.branch, branch())),
                    BranchKind::Protection => {
                        Document::Protection(core::mem::replace(&mut self.protection, protection()))
                    }
                    BranchKind::Permission => Document::Permission(CollaboratorPermission {
                        permission: self.permission,
                        user: core::mem::replace(&mut self.user, document::user()),
                    }),
                    BranchKind::Branches | BranchKind::Refs | BranchKind::Protections => {
                        Document::Listed { skipped: walk.skipped() }
                    }
                    BranchKind::Ref | BranchKind::Commit | BranchKind::Contexts | BranchKind::Context => {
                        unreachable!("branch root selection")
                    }
                });
            }
        };
        match result {
            Ok(()) => Step::Demand(if walk.parent() == Some(Kind::Branch(BranchKind::Contexts)) {
                skein_json::tokenizer::Request::Text(limits.string_bytes)
            } else {
                skein_json::tokenizer::Request::Next
            }),
            Err(error) => Step::Malformed(error),
        }
    }
    fn scalar(&mut self, key: &'static [u8], value: Value) -> Result<(), Malformed> {
        match value {
            Value::Input(_) => unreachable!("dispatch input table"),
            Value::Number(n) => match key {
                b"id" => self.user.id = n,
                b"required_approvals" => {
                    self.protection.required_approvals = match u32::try_from(n) {
                        Ok(n) => n,
                        Err(_) => return Err(Malformed::Range { field: key }),
                    }
                }
                _ => unreachable!("branch number table"),
            },
            Value::Text(text) => match key {
                b"login" => self.user.login = text,
                b"name" => self.branch.name = text,
                b"ref" => {
                    let name = text.strip_prefix(b"refs/heads/").ok_or(Malformed::Value { field: key })?;
                    self.branch.name = Box::from(name);
                }
                b"rule_name" => self.protection.rule_name = text,
                b"effective_branch_protection_name" => {
                    self.branch.effective_branch_protection_name = if text.is_empty() { None } else { Some(text) }
                }
                b"permission" => self.permission = permission(&text).ok_or(Malformed::Value { field: key })?,
                b"status_check_contexts" => {
                    if self.contexts.push(text).is_err() {
                        return Err(Malformed::Bound { field: key });
                    }
                }
                _ => unreachable!("branch text table"),
            },
            Value::Id(id) => self.branch.commit = id.expect("required branch commit"),
            Value::Boolean(value) => match key {
                b"enable_status_check" => self.protection.enable_status_check = value,
                b"dismiss_stale_approvals" => self.protection.dismiss_stale_approvals = value,
                b"require_signed_commits" => self.protection.require_signed_commits = value,
                _ => unreachable!("branch boolean table"),
            },
            Value::Time(_) | Value::Body(_) => unreachable!("branch scalar table"),
        }
        Ok(())
    }
}
