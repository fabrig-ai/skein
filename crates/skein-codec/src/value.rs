//! An index over canonical bytes at a schema's ceilings (codec.md, section 8).

use alloc::boxed::Box;
use skein_lib::{List, Reader, Secret};

use crate::schema::{DeclIx, FieldIx, Schema, Shape, Span, Type, TypeIx, VariantIx};
use crate::{Reason, read_count, read_len, text_is_valid};

#[cfg(test)]
mod tests;

/// A record's bytes and their index in preorder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Value {
    bytes: Secret,
    entries: List<Entry>,
    growing: Option<u32>,
}

/// An entry in preorder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Node(u32);

/// The next list item and how many are left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Items {
    next: u32,
    left: u32,
}

/// A declaration's field, tag, tail or version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Path {
    /// The declaration named by the generated path.
    pub decl: DeclIx,
    /// The part named by the generated path.
    pub part: Part,
}

/// The part of a declaration named by a refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    /// A record's field, through lists and options.
    Field(FieldIx),
    /// An enumeration's tag or trailing bytes.
    Tag,
    /// A record's trailing bytes.
    Tail,
    /// A versioned record's version.
    Version,
}

/// A decoding refusal at its generated path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Problem {
    /// The declaration and part refused.
    pub path: Path,
    /// Why the bytes were refused.
    pub reason: Reason,
}

/// A problem in the bytes or too little room for their nodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Refusal {
    /// The generated decoder's path and reason.
    Problem(Problem),
    /// A node passed the value's room.
    Room,
}

/// The node is absent or not of the requested kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Misread {
    /// The node that could not be read that way.
    pub node: Node,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Record { decl: DeclIx, versioned: bool, first: FieldIx, count: u32 },
    Enum { decl: DeclIx, first: VariantIx, count: u32 },
    Uint(u32),
    Bool,
    Content { width: u32, text: bool, prefixed: bool },
    Secret(u32),
    List { bound: u32, item: TypeIx },
    Option(TypeIx),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    kind: Kind,
    source: Source,
    span: Span,
    content: u32,
    count: u32,
    parent: Option<Node>,
    next: u32,
    progress: u32,
    path: Path,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    Declaration(DeclIx),
    Type(TypeIx),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Child {
    source: Source,
    kind: Kind,
    path: Path,
}

impl Value {
    /// Sets the room for this many nodes; entries allocate as they fill.
    #[must_use]
    pub fn with_room(nodes: u32) -> Value {
        Value { bytes: Secret::new(Box::default()), entries: List::with_capacity(nodes), growing: None }
    }

    /// The heap allocated for the entries, or none past u64.
    #[must_use]
    pub fn worst_case_heap(nodes: u32) -> Option<u64> {
        List::<Entry>::worst_case(nodes)
    }

    /// Decodes at the schema's ceilings, allocating only to grow tables past
    /// what an earlier decode needed.
    pub fn decode(&mut self, schema: &Schema, root: DeclIx, bytes: Box<[u8]>) -> Result<Node, Refusal> {
        self.decode_index(schema, root, bytes, &mut List::with_capacity(0)).0
    }

    pub(crate) fn growing(nodes: u32) -> Value {
        Value { bytes: Secret::new(Box::default()), entries: List::with_capacity(0), growing: Some(nodes) }
    }

    pub(crate) fn room(&self) -> u32 {
        self.entries.capacity()
    }

    // A builder cannot borrow the old index mutably. Its empty replacement
    // waits here until start can move it into the old half (codec.md, 11.7).
    pub(crate) fn reserve(&mut self, need: u32, occupied: u32) {
        if need <= occupied || need <= self.room() {
            return;
        }
        let ceiling = self.growing.expect("only a patch reserves growing entries");
        let mut room = growing_room(self.room(), need, ceiling);
        if need <= ceiling.checked_div(16).expect("positive divisor") {
            let shared = need
                .checked_mul(2)
                .expect("small need fits twice")
                .checked_sub(occupied)
                .expect("need exceeds occupied");
            room = room.min(shared);
        }
        assert!(self.entries.is_empty(), "the replacement holds no decoded entries");
        // The empty replacement has nothing to copy; free it before allocating.
        self.entries = List::with_capacity(0);
        self.entries = List::with_capacity(room);
    }

    pub(crate) fn use_reserved(&mut self, reserve: &mut Value) {
        if reserve.room() > self.room() {
            let entries = core::mem::replace(&mut reserve.entries, List::with_capacity(0));
            reserve.entries = core::mem::replace(&mut self.entries, entries);
            reserve.entries.clear();
        }
    }

    #[cfg(test)]
    pub(crate) fn allocation(&self) -> (u32, *const ()) {
        (self.room(), self.entries.as_slice().as_ptr().cast())
    }

    pub(crate) fn decode_count(
        &mut self,
        schema: &Schema,
        root: DeclIx,
        bytes: Box<[u8]>,
        reserve: &mut Value,
    ) -> (Result<Node, Refusal>, u32) {
        self.decode_index(schema, root, bytes, &mut reserve.entries)
    }

    fn decode_index(
        &mut self,
        schema: &Schema,
        root: DeclIx,
        bytes: Box<[u8]>,
        spare: &mut List<Entry>,
    ) -> (Result<Node, Refusal>, u32) {
        self.entries.clear();
        self.bytes = Secret::new(bytes);
        let mut reader = Reader::new(self.bytes.expose());
        let result = index(&mut self.entries, spare, self.growing, schema, root, &mut reader);
        let nodes = self.entries.len();
        if result.is_err() {
            self.entries.clear();
            self.bytes = Secret::new(Box::default());
        }
        (result, nodes)
    }

    /// The canonical encoding.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        self.bytes.expose()
    }

    /// Moves the bytes back to their owner, leaving the value empty.
    pub fn take(&mut self) -> Secret {
        self.entries.clear();
        core::mem::replace(&mut self.bytes, Secret::new(Box::default()))
    }

    /// The node's encoded range.
    pub fn span(&self, node: Node) -> Result<Span, Misread> {
        Ok(self.entry(node)?.span)
    }

    /// Finds a record's field by hopping over its preceding siblings.
    pub fn field(&self, record: Node, field: FieldIx) -> Result<Node, Misread> {
        let entry = self.entry(record)?;
        match entry.kind {
            Kind::Record { first, count, .. } => {
                let offset = field.index().checked_sub(first.index()).ok_or(Misread { node: record })?;
                if offset >= count {
                    return Err(Misread { node: record });
                }
                let mut next = add(record.0, 1);
                for _ in 0..offset {
                    next = self.entry(Node(next))?.next;
                }
                Ok(Node(next))
            }
            Kind::Enum { .. }
            | Kind::Uint(_)
            | Kind::Bool
            | Kind::Content { .. }
            | Kind::Secret(_)
            | Kind::List { .. }
            | Kind::Option(_) => Err(Misread { node: record }),
        }
    }

    /// The number of items in a list.
    pub fn count(&self, list: Node) -> Result<u32, Misread> {
        let items = self.items(list)?;
        Ok(items.left)
    }

    /// The node holding this value, or none for the root.
    pub fn parent(&self, node: Node) -> Result<Option<Node>, Misread> {
        Ok(self.entry(node)?.parent)
    }

    pub(crate) fn source(&self, node: Node) -> Result<Source, Misread> {
        Ok(self.entry(node)?.source)
    }

    pub(crate) fn nodes(&self, node: Node) -> Result<u32, Misread> {
        Ok(self.entry(node)?.next.checked_sub(node.0).expect("preorder subtree ends after its root"))
    }

    /// Starts walking a list's items.
    pub fn items(&self, list: Node) -> Result<Items, Misread> {
        let entry = self.entry(list)?;
        match entry.kind {
            Kind::List { .. } => Ok(Items { next: add(list.0, 1), left: entry.count }),
            Kind::Record { .. }
            | Kind::Enum { .. }
            | Kind::Uint(_)
            | Kind::Bool
            | Kind::Content { .. }
            | Kind::Secret(_)
            | Kind::Option(_) => Err(Misread { node: list }),
        }
    }

    /// Hops to the next list item, if any remains.
    pub fn next_item(&self, items: &mut Items) -> Option<Node> {
        if items.left == 0 {
            return None;
        }
        let node = Node(items.next);
        let entry = self.entries.get(node.0)?;
        items.next = entry.next;
        items.left = items.left.checked_sub(1).expect("an item remains");
        Some(node)
    }

    /// Reads an option's presence and its value when present.
    pub fn some(&self, option: Node) -> Result<Option<Node>, Misread> {
        let entry = self.entry(option)?;
        match entry.kind {
            Kind::Option(_) => Ok(if entry.count == 0 { None } else { Some(Node(add(option.0, 1))) }),
            Kind::Record { .. }
            | Kind::Enum { .. }
            | Kind::Uint(_)
            | Kind::Bool
            | Kind::Content { .. }
            | Kind::Secret(_)
            | Kind::List { .. } => Err(Misread { node: option }),
        }
    }

    /// The enumeration's selected variant in its schema.
    pub fn tag(&self, enumeration: Node) -> Result<VariantIx, Misread> {
        let entry = self.entry(enumeration)?;
        match entry.kind {
            Kind::Enum { first, .. } => Ok(VariantIx::new(add(first.index(), entry.count))),
            Kind::Record { .. }
            | Kind::Uint(_)
            | Kind::Bool
            | Kind::Content { .. }
            | Kind::Secret(_)
            | Kind::List { .. }
            | Kind::Option(_) => Err(Misread { node: enumeration }),
        }
    }

    /// The enumeration's record, when its selected variant holds one.
    pub fn payload(&self, enumeration: Node) -> Result<Option<Node>, Misread> {
        let entry = self.entry(enumeration)?;
        match entry.kind {
            Kind::Enum { .. } => {
                let child = add(enumeration.0, 1);
                Ok(if child < entry.next { Some(Node(child)) } else { None })
            }
            Kind::Record { .. }
            | Kind::Uint(_)
            | Kind::Bool
            | Kind::Content { .. }
            | Kind::Secret(_)
            | Kind::List { .. }
            | Kind::Option(_) => Err(Misread { node: enumeration }),
        }
    }

    /// Reads u8 through u64, or a duration's nanoseconds, big-endian.
    pub fn uint(&self, node: Node) -> Result<u64, Misread> {
        let entry = self.entry(node)?;
        match entry.kind {
            Kind::Uint(_) => {
                let mut value = 0_u64;
                for &byte in self.range(node, entry.span.start)? {
                    value = value.checked_mul(256).expect("an integer has at most eight bytes");
                    value = value.checked_add(u64::from(byte)).expect("an integer fits u64");
                }
                Ok(value)
            }
            Kind::Record { .. }
            | Kind::Enum { .. }
            | Kind::Bool
            | Kind::Content { .. }
            | Kind::Secret(_)
            | Kind::List { .. }
            | Kind::Option(_) => Err(Misread { node }),
        }
    }

    /// Reads a boolean byte as false or true.
    pub fn boolean(&self, node: Node) -> Result<bool, Misread> {
        let entry = self.entry(node)?;
        match entry.kind {
            Kind::Bool => Ok(self.range(node, entry.span.start)?.first() == Some(&1)),
            Kind::Record { .. }
            | Kind::Enum { .. }
            | Kind::Uint(_)
            | Kind::Content { .. }
            | Kind::Secret(_)
            | Kind::List { .. }
            | Kind::Option(_) => Err(Misread { node }),
        }
    }

    /// Reads fixed, bytes or text content, without a length prefix.
    pub fn content(&self, node: Node) -> Result<&[u8], Misread> {
        let entry = self.entry(node)?;
        match entry.kind {
            Kind::Content { .. } => self.range(node, entry.content),
            Kind::Record { .. }
            | Kind::Enum { .. }
            | Kind::Uint(_)
            | Kind::Bool
            | Kind::Secret(_)
            | Kind::List { .. }
            | Kind::Option(_) => Err(Misread { node }),
        }
    }

    /// Exposes a secret's bytes for their owner's use.
    pub fn expose(&self, node: Node) -> Result<&[u8], Misread> {
        let entry = self.entry(node)?;
        match entry.kind {
            Kind::Secret(_) => self.range(node, entry.content),
            Kind::Record { .. }
            | Kind::Enum { .. }
            | Kind::Uint(_)
            | Kind::Bool
            | Kind::Content { .. }
            | Kind::List { .. }
            | Kind::Option(_) => Err(Misread { node }),
        }
    }

    fn entry(&self, node: Node) -> Result<&Entry, Misread> {
        self.entries.get(node.0).ok_or(Misread { node })
    }

    fn range(&self, node: Node, start: u32) -> Result<&[u8], Misread> {
        let entry = self.entry(node)?;
        let start = usize::try_from(start).expect("byte offset fits usize");
        let end = usize::try_from(entry.span.end).expect("byte offset fits usize");
        self.bytes.expose().get(start..end).ok_or(Misread { node })
    }
}

fn declaration(schema: &Schema, decl: DeclIx) -> Kind {
    match schema.declaration(decl).shape {
        Shape::Record { versioned, first, count } => Kind::Record { decl, versioned, first, count },
        Shape::Enum { first, count } => Kind::Enum { decl, first, count },
    }
}

fn kind(schema: &Schema, ty: TypeIx) -> Kind {
    match schema.ty(ty) {
        Type::U8 => Kind::Uint(1),
        Type::U16 => Kind::Uint(2),
        Type::U32 => Kind::Uint(4),
        Type::U64 | Type::Duration => Kind::Uint(8),
        Type::Bool => Kind::Bool,
        Type::Fixed(width) => Kind::Content { width, text: false, prefixed: false },
        Type::Bytes { bound, .. } => Kind::Content { width: bound, text: false, prefixed: true },
        Type::Text(width) => Kind::Content { width, text: true, prefixed: true },
        Type::Secret { bound, .. } => Kind::Secret(bound),
        Type::List { bound, item } => Kind::List { bound, item },
        Type::Option(item) => Kind::Option(item),
        Type::Named(decl) => declaration(schema, decl),
    }
}

impl Entry {
    fn open(
        child: Child,
        parent: Option<Node>,
        schema: &Schema,
        reader: &mut Reader<'_>,
        total: u32,
    ) -> Result<Entry, Problem> {
        let start = position(total, reader);
        let mut entry = Entry {
            kind: child.kind,
            source: child.source,
            span: Span { start, end: start },
            content: start,
            count: 0,
            parent,
            next: 0,
            progress: 0,
            path: child.path,
        };
        let result = entry.read(schema, reader, total);
        match result {
            Ok(()) => Ok(entry),
            Err(reason) => Err(Problem { path: entry.path, reason }),
        }
    }

    fn read(&mut self, schema: &Schema, reader: &mut Reader<'_>, total: u32) -> Result<(), Reason> {
        match self.kind {
            Kind::Record { decl, versioned, .. } => {
                if versioned {
                    self.path = Path { decl, part: Part::Version };
                    let version = reader.u16().ok_or(Reason::Short)?;
                    if version != schema.version() {
                        return Err(Reason::Version);
                    }
                }
            }
            Kind::Enum { decl, count, .. } => {
                self.path = Path { decl, part: Part::Tag };
                self.count = u32::from(reader.u8().ok_or(Reason::Short)?);
                if self.count >= count {
                    return Err(Reason::Tag);
                }
            }
            Kind::Uint(width) => reader.skip(width).ok_or(Reason::Short)?,
            Kind::Bool => {
                if reader.u8().ok_or(Reason::Short)? > 1 {
                    return Err(Reason::Bool);
                }
            }
            Kind::Content { width, text, prefixed } => {
                let length = if prefixed { read_len(reader, width)? } else { width };
                self.content = position(total, reader);
                let bytes = reader.bytes(length).ok_or(Reason::Short)?;
                if text && !text_is_valid(bytes) {
                    return Err(Reason::Utf8);
                }
            }
            Kind::Secret(bound) => {
                let length = read_len(reader, bound)?;
                self.content = position(total, reader);
                reader.skip(length).ok_or(Reason::Short)?;
            }
            Kind::List { bound, item } => {
                self.count = read_count(reader, bound)?;
                let minimum = schema.fewest_bytes(item);
                if minimum > 0 && self.count > reader.remaining().checked_div(minimum).expect("positive minimum") {
                    return Err(Reason::Short);
                }
            }
            Kind::Option(_) => {
                self.count = u32::from(reader.u8().ok_or(Reason::Short)?);
                if self.count > 1 {
                    return Err(Reason::Tag);
                }
            }
        }
        Ok(())
    }

    fn child(&mut self, schema: &Schema) -> Option<Child> {
        let child = match self.kind {
            Kind::Record { decl, first, count, .. } => {
                if self.progress >= count {
                    return None;
                }
                let field = FieldIx::new(add(first.index(), self.progress));
                Child {
                    source: Source::Type(schema.field(field).ty),
                    kind: kind(schema, schema.field(field).ty),
                    path: Path { decl, part: Part::Field(field) },
                }
            }
            Kind::Enum { first, .. } => {
                if self.progress > 0 {
                    return None;
                }
                let variant = schema.variant(VariantIx::new(add(first.index(), self.count)));
                let decl = variant.record?;
                Child { source: Source::Declaration(decl), kind: declaration(schema, decl), path: self.path }
            }
            Kind::List { item, .. } | Kind::Option(item) => {
                if self.progress >= self.count {
                    return None;
                }
                Child { source: Source::Type(item), kind: kind(schema, item), path: self.path }
            }
            Kind::Uint(_) | Kind::Bool | Kind::Content { .. } | Kind::Secret(_) => return None,
        };
        self.progress = add(self.progress, 1);
        Some(child)
    }
}

fn index(
    entries: &mut List<Entry>,
    spare: &mut List<Entry>,
    growing: Option<u32>,
    schema: &Schema,
    root: DeclIx,
    reader: &mut Reader<'_>,
) -> Result<Node, Refusal> {
    let total = reader.remaining();
    let root_path = match schema.declaration(root).shape {
        Shape::Record { .. } => Path { decl: root, part: Part::Tail },
        Shape::Enum { .. } => Path { decl: root, part: Part::Tag },
    };
    let mut current: Option<Node> = None;
    let ceiling = growing.unwrap_or(entries.capacity());
    let turns = u64::from(ceiling)
        .checked_mul(2)
        .expect("twice u32 fits u64")
        .checked_add(1)
        .expect("twice u32 plus one fits u64");
    for _ in 0..turns {
        let child = match current {
            None => Some(Child { source: Source::Declaration(root), kind: declaration(schema, root), path: root_path }),
            Some(node) => entries.get_mut(node.0).expect("open node belongs to the index").child(schema),
        };
        match child {
            Some(child) => {
                if entries.len() == ceiling {
                    return Err(Refusal::Room);
                }
                let entry = match Entry::open(child, current, schema, reader, total) {
                    Ok(entry) => entry,
                    Err(problem) => return Err(Refusal::Problem(problem)),
                };
                if entries.room() == 0 {
                    *spare = List::with_capacity(0);
                    let need = add(entries.len(), 1);
                    let mut larger = List::with_capacity(growing_room(entries.capacity(), need, ceiling));
                    for &entry in &*entries {
                        larger.push(entry).expect("the larger index holds every entry");
                    }
                    *entries = larger;
                }
                let node = Node(entries.len());
                entries.push(entry).expect("room checked before reading a node");
                current = Some(node);
            }
            None => {
                let node = current.expect("only an open node can close");
                let next = entries.len();
                let entry = entries.get_mut(node.0).expect("open node belongs to the index");
                entry.span.end = position(total, reader);
                entry.next = next;
                current = entry.parent;
                if current.is_none() {
                    if !reader.is_empty() {
                        return Err(Refusal::Problem(Problem { path: root_path, reason: Reason::Trailing }));
                    }
                    return Ok(Node(0));
                }
            }
        }
    }
    unreachable!("each turn opens or closes a node, bounded by twice the room plus one")
}

fn add(left: u32, right: u32) -> u32 {
    left.checked_add(right).expect("entry and schema indices fit u32")
}

fn position(total: u32, reader: &Reader<'_>) -> u32 {
    total.checked_sub(reader.remaining()).expect("the reader only consumes its input")
}

// Below a sixteenth, at most twice the need; beyond it, the ceiling.
// A copied allocation is at most a sixteenth of its table's ceiling.
pub(crate) fn growing_room(room: u32, need: u32, ceiling: u32) -> u32 {
    assert!(need <= ceiling, "admission checks the configured ceiling");
    if need <= room {
        return room;
    }
    let small = ceiling.checked_div(16).expect("positive divisor");
    if need > small {
        return ceiling;
    }
    room.max(1).checked_mul(2).expect("twice a sixteenth fits u32").max(need).min(small)
}
