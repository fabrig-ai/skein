//! Preorder runs over small schemas (codec.md, 11.4–11.7).
#![expect(clippy::disallowed_macros, reason = "schema and patch fixtures use formatting")]

use crate::Reason;
use crate::patch::{Capacity, Halt, Patcher, PlanIx, RefusalIx, Step};
use crate::schema::{self, DeclIx, FieldIx, VariantIx};
use crate::value::{Part, Path, Problem, Refusal, Value};

// The inline fixtures have at most 64 nodes and 256 bytes; three kept versions
// and three plans admit an identity beside two consecutive passes.
const CAPACITY: Capacity = Capacity { versions: 3, patches: 3, text: 4096, bytes: 256, nodes: 64 };
const HEADER: &str = "patch 1\nowner o 7\nfamily f 1 to 2\n";
const D: DeclIx = DeclIx::new(0);
const F: FieldIx = FieldIx::new(0);
const FIELD: Path = Path { decl: D, part: Part::Field(F) };
const TAG: Path = Path { decl: D, part: Part::Tag };

fn planned_at(capacity: Capacity, old: &str, new: &str, lines: &str) -> (Patcher, PlanIx) {
    let mut patcher = Patcher::new(capacity);
    patcher.keep(format!("family f 1\n{old}").as_bytes()).expect("old schema");
    patcher.keep(format!("family f 2\n{new}").as_bytes()).expect("new schema");
    let plan = patcher.plan(format!("{HEADER}{lines}").as_bytes()).expect("patch").expect("family");
    (patcher, plan)
}
fn planned(old: &str, new: &str, lines: &str) -> (Patcher, PlanIx) {
    planned_at(CAPACITY, old, new, lines)
}
fn started(old: &str, new: &str, lines: &str, root: &str, bytes: &[u8]) -> (Patcher, PlanIx) {
    let (mut patcher, plan) = planned(old, new, lines);
    let root = patcher.schema(1).expect("old").find(root.as_bytes()).expect("root");
    patcher.start(plan, root, bytes.into()).expect("start");
    (patcher, plan)
}
fn finished(patcher: &mut Patcher, root: &str, expected: &[u8]) {
    assert_eq!(patcher.next(), Ok(Step::Done));
    assert_eq!(patcher.finish().expect("finish"), expected);
    let version = patcher.versions(patcher.build.plan.expect("running plan")).to;
    let schema = patcher.schema(version).expect("new schema");
    let mut value = Value::with_room(CAPACITY.nodes);
    value.decode(schema, schema.find(root.as_bytes()).expect("new root"), expected.into()).expect("new bytes decode");
}
fn stopped(patcher: &mut Patcher) -> crate::value::Node {
    match patcher.next().expect("next stop") {
        Step::Written { at, .. } | Step::Held { at, .. } => at,
        Step::Done => unreachable!("fixture has a stop"),
    }
}
fn restart(patcher: &mut Patcher, plan: PlanIx, bytes: &[u8]) {
    assert_eq!(patcher.next(), Err(Halt::Order));
    patcher.start(plan, D, bytes.into()).expect("fresh start after halt");
}

struct Derived {
    old: &'static str,
    new: &'static str,
    lines: &'static str,
    bytes: &'static [u8],
    expected: &'static [u8],
}
const DERIVED: &[Derived] = &[
    Derived { old: "record D {}", new: "record D { f: u8 }", lines: "add D.f u8 42\n", bytes: b"", expected: &[42] },
    Derived {
        old: "record D {}",
        new: "record D { f: duration }",
        lines: "add D.f duration 42\n",
        bytes: b"",
        expected: &[0, 0, 0, 0, 0, 0, 0, 42],
    },
    Derived {
        old: "record D {}",
        new: "record D { f: bool }",
        lines: "add D.f bool true\n",
        bytes: b"",
        expected: &[1],
    },
    Derived {
        old: "record D {}",
        new: "record D { f: fixed 2 }",
        lines: "add D.f fixed 2 0xabcd\n",
        bytes: b"",
        expected: &[0xab, 0xcd],
    },
    Derived {
        old: "record D {}",
        new: "record D { f: bytes 2 }",
        lines: "add D.f bytes 2 0xabcd\n",
        bytes: b"",
        expected: &[0, 0, 0, 2, 0xab, 0xcd],
    },
    Derived {
        old: "record D {}",
        new: "record D { f: text 2 }",
        lines: "add D.f text 2 \"hi\"\n",
        bytes: b"",
        expected: &[0, 0, 0, 2, b'h', b'i'],
    },
    Derived {
        old: "record D {}",
        new: "record D { f: option u8 }",
        lines: "add D.f option u8 none\n",
        bytes: b"",
        expected: &[0],
    },
    Derived {
        old: "record D {}",
        new: "record D { f: option u8 }",
        lines: "add D.f option u8 some 42\n",
        bytes: b"",
        expected: &[1, 42],
    },
    Derived {
        old: "record D {}",
        new: "record D { f: list 2 u8 }",
        lines: "add D.f list 2 u8 []\n",
        bytes: b"",
        expected: &[0, 0, 0, 0],
    },
    Derived {
        old: "enum E { a b } record D {}",
        new: "enum E { a b } record D { f: E }",
        lines: "add D.f E b\n",
        bytes: b"",
        expected: &[1],
    },
    Derived { old: "record D { f: u8 }", new: "record D {}", lines: "drop D.f\n", bytes: &[42], expected: b"" },
    Derived {
        old: "record D { f: u8 }",
        new: "record D { g: u8 }",
        lines: "rename D.f g\n",
        bytes: &[42],
        expected: &[42],
    },
    Derived {
        old: "record D { a: u8 b: u16 }",
        new: "record D { b: u16 a: u8 }",
        lines: "order D b a\n",
        bytes: &[1, 2, 3],
        expected: &[2, 3, 1],
    },
    Derived {
        old: "record D { f: u8 }",
        new: "record D { f: u16 }",
        lines: "widen D.f u8 u16\n",
        bytes: &[255],
        expected: &[0, 255],
    },
    Derived {
        old: "record D { f: text 1 }",
        new: "record D { f: bytes 2 }",
        lines: "",
        bytes: &[0, 0, 0, 1, 97],
        expected: &[0, 0, 0, 1, 97],
    },
    Derived {
        old: "record D { f: u8 }",
        new: "record D { f: option option u16 }",
        lines: "wrap D.f u8 option option u16\n",
        bytes: &[42],
        expected: &[1, 1, 0, 42],
    },
    Derived {
        old: "record D { f: list 2 u8 }",
        new: "record D { f: list 3 option u8 }",
        lines: "wrap D.f list 2 u8 list 3 option u8\n",
        bytes: &[0, 0, 0, 2, 4, 5],
        expected: &[0, 0, 0, 2, 1, 4, 1, 5],
    },
    Derived { old: "enum D { a b }", new: "enum D { x a b }", lines: "add D.x\n", bytes: &[1], expected: &[2] },
    Derived { old: "enum D { a b }", new: "enum D { a b x }", lines: "", bytes: &[1], expected: &[1] },
    Derived { old: "enum D { a b }", new: "enum D { b a }", lines: "order D b a\n", bytes: &[0], expected: &[1] },
    Derived {
        old: "record D { f: u8 }",
        new: "versioned record D { f: u8 }",
        lines: "versioned D\n",
        bytes: &[42],
        expected: &[0, 2, 42],
    },
    Derived {
        old: "versioned record D { f: u8 }",
        new: "record D { f: u8 }",
        lines: "unversioned D\n",
        bytes: &[0, 1, 42],
        expected: &[42],
    },
];
#[test]
fn derived_operations_emit_the_expected_canonical_bytes() {
    for row in DERIVED {
        let (mut patcher, _) = started(row.old, row.new, row.lines, "D", row.bytes);
        finished(&mut patcher, "D", row.expected);
    }
}

#[test]
fn versions_inside_lists_and_variants_are_all_rewritten() {
    const TEXT: &str = "versioned record R { f: u8 } enum E { r: R } versioned record D { rs: list 2 R e: E }";
    let (mut patcher, _) = started(TEXT, TEXT, "", "D", &[0, 1, 0, 0, 0, 2, 0, 1, 3, 0, 1, 4, 0, 0, 1, 5]);
    finished(&mut patcher, "D", &[0, 2, 0, 0, 0, 2, 0, 2, 3, 0, 2, 4, 0, 0, 2, 5]);
}

#[test]
fn written_field_reads_old_fields_and_supplies_its_value() {
    let (mut patcher, _) =
        started("record D { a: u8 }", "record D { a: u8 f: u16 }", "add D.f u16 written\nwritten D.f\n", "D", &[7]);
    let at = match patcher.next().expect("written field") {
        Step::Written { site, at } => {
            assert_eq!(site, crate::patch::Site::new(7, 0));
            at
        }
        Step::Held { .. } | Step::Done => unreachable!("written field fixture"),
    };
    let (old, new) = patcher.parts();
    let value = old.uint(old.field(at, F).expect("a")).expect("uint");
    new.put_uint(value.checked_mul(10).expect("small fixture")).expect("answer");
    finished(&mut patcher, "D", &[7, 0, 70]);
}

#[test]
fn written_record_builds_and_carries_its_fields() {
    const OLD: &str = "record D { a: u8 b: text 3 }";
    const NEW: &str = "versioned record D { a: option u16 b: text 4 }";
    let (mut patcher, _) =
        started(OLD, NEW, "versioned D\nwrap D.a u8 option u16\nwritten D\n", "D", &[7, 0, 0, 0, 1, 97]);
    let at = stopped(&mut patcher);
    let (old, new) = patcher.parts();
    new.open().expect("record");
    new.carry(old, old.field(at, F).expect("a")).expect("wrapped widened a");
    new.put_content_of(old, old.field(at, FieldIx::new(1)).expect("b")).expect("b");
    new.close().expect("record complete");
    finished(&mut patcher, "D", &[0, 2, 1, 0, 7, 0, 0, 0, 1, 97]);
}

#[test]
fn written_enum_carries_kept_variants_and_builds_dropped_ones() {
    const OLD: &str = "record R { f: u8 } enum D { kept: R dropped }";
    const NEW: &str = "record R { f: u8 } enum D { kept: R }";
    const LINES: &str = "drop D.dropped\nwritten D\n";
    let (mut patcher, _) = started(OLD, NEW, LINES, "D", &[0, 9]);
    let at = stopped(&mut patcher);
    let (old, new) = patcher.parts();
    new.carry(old, at).expect("own site's kept variant");
    finished(&mut patcher, "D", &[0, 9]);
    let (mut patcher, _) = started(OLD, NEW, LINES, "D", &[1]);
    stopped(&mut patcher);
    let (_, new) = patcher.parts();
    new.open_variant(VariantIx::new(0)).expect("kept");
    new.put_uint(0).expect("f");
    new.close().expect("payload");
    finished(&mut patcher, "D", &[0, 0]);
}

#[test]
fn list_sites_follow_item_order_and_read_enclosing_records() {
    const OLD: &str = "record R { f: u8 } record D { rs: list 2 R base: u8 }";
    const NEW: &str = "record R { f: u8 g: u8 } record D { rs: list 2 R base: u8 }";
    let (mut patcher, _) = started(OLD, NEW, "add R.g u8 written\nwritten R.g\n", "D", &[0, 0, 0, 2, 3, 4, 10]);
    for wanted in [3, 4] {
        let at = stopped(&mut patcher);
        let (old, new) = patcher.parts();
        let list = old.parent(at).expect("parent").expect("list");
        let record = old.parent(list).expect("parent").expect("record");
        assert_eq!(record, old.root());
        assert_eq!(old.parent(record), Ok(None));
        assert_eq!(old.count(list), Ok(2));
        assert_eq!(old.uint(old.field(at, F).expect("f")), Ok(wanted));
        let base = old.uint(old.field(record, FieldIx::new(2)).expect("base")).expect("uint");
        new.put_uint(base.checked_add(wanted).expect("fixture")).expect("answer");
    }
    finished(&mut patcher, "D", &[0, 0, 0, 2, 3, 13, 4, 14, 10]);
}

#[test]
fn declared_and_undeclared_refusals_end_the_run() {
    for (refusal, expected) in [
        (RefusalIx::new(0), Halt::Refused(RefusalIx::new(0))),
        (RefusalIx::new(1), Halt::Undeclared(RefusalIx::new(1))),
    ] {
        let (mut patcher, plan) =
            started("record D {}", "record D {}", "written D\nrefuses bad \"cannot convert\"\n", "D", b"");
        stopped(&mut patcher);
        assert_eq!(patcher.refusal(RefusalIx::new(0)), Some(b"cannot convert".as_slice()));
        assert_eq!(patcher.refusal(RefusalIx::new(1)), None);
        assert_eq!(patcher.parts().1.refuse(refusal), expected);
        restart(&mut patcher, plan, b"");
    }
}

#[test]
fn held_fields_accept_replacements_and_carry_their_old_bytes() {
    const TEXT: &str = "record D { a: u8 f: bytes 3 holds other R }";
    for (replacement, expected) in
        [(Some(b"xyz".as_slice()), &[7, 0, 0, 0, 3, 120, 121, 122][..]), (None, &[7, 0, 0, 0, 1, 97][..])]
    {
        let (mut patcher, _) = started(TEXT, TEXT, "", "D", &[7, 0, 0, 0, 1, 97]);
        let at = match patcher.next().expect("held") {
            Step::Held { at, family, name } => {
                assert_eq!(patcher.spelled(family), b"other");
                assert_eq!(patcher.spelled(name), b"R");
                at
            }
            Step::Written { .. } | Step::Done => unreachable!("held fixture"),
        };
        let (old, new) = patcher.parts();
        match replacement {
            Some(bytes) => new.put_content(bytes).expect("replacement"),
            None => new.carry(old, at).expect("held copy"),
        }
        finished(&mut patcher, "D", expected);
    }
}

fn identity_carries_held(text: &[u8]) {
    const BYTES: &[u8] = &[0, 1, 7, 0, 0, 0, 1, 97];
    let mut patcher = Patcher::new(CAPACITY);
    patcher.keep(text).expect("schema");
    let identity = patcher.identity(1).expect("identity");
    patcher.start(identity, D, BYTES.into()).expect("start");
    let at = stopped(&mut patcher);
    let (old, new) = patcher.parts();
    new.carry(old, at).expect("held");
    finished(&mut patcher, "D", BYTES);
}

#[test]
fn identity_stops_only_at_held_fields_and_returns_the_same_bytes() {
    identity_carries_held(b"family f 1\nversioned record D { a: u8 f: bytes 3 holds other R }");
}

#[test]
fn identity_stops_at_held_secrets_and_carries_the_bytes() {
    identity_carries_held(b"family f 1\nversioned record D { a: u8 f: secret 3 holds other R }");
}

#[test]
fn copying_content_of_a_held_field_is_misbuilt() {
    const TEXT: &str = "record D { f: bytes 3 holds other R }";
    let (mut patcher, plan) = started(TEXT, TEXT, "", "D", &[0, 0, 0, 1, 97]);
    let at = stopped(&mut patcher);
    let (old, new) = patcher.parts();
    assert_eq!(new.put_content_of(old, at), Err(Halt::Misbuilt(FIELD)));
    restart(&mut patcher, plan, &[0, 0, 0, 1, 97]);
}

#[test]
fn copying_content_of_a_held_secret_is_misbuilt() {
    const TEXT: &str = "record D { f: secret 3 holds other R }";
    let (mut patcher, plan) = started(TEXT, TEXT, "", "D", &[0, 0, 0, 1, 97]);
    let at = stopped(&mut patcher);
    let (old, new) = patcher.parts();
    assert_eq!(new.put_content_of(old, at), Err(Halt::Misbuilt(FIELD)));
    restart(&mut patcher, plan, &[0, 0, 0, 1, 97]);
}

// The held secret admits three content bytes after its length prefix.
const HELD_SECRET: &str = "record D { f: secret 3 holds other R }";
const SECRET_BYTES: &[u8] = &[0, 0, 0, 1, 97];

fn stopped_secret() -> Patcher {
    let (mut patcher, _) = started(HELD_SECRET, HELD_SECRET, "", "D", SECRET_BYTES);
    match patcher.next().expect("held secret") {
        Step::Held { family, name, .. } => {
            assert_eq!(patcher.spelled(family), b"other");
            assert_eq!(patcher.spelled(name), b"R");
        }
        Step::Written { .. } | Step::Done => unreachable!("held secret fixture"),
    }
    patcher
}

#[test]
fn held_secret_accepts_replacement_at_its_bound() {
    let mut patcher = stopped_secret();
    patcher.parts().1.put_content(b"xyz").expect("replacement at bound");
    finished(&mut patcher, "D", &[0, 0, 0, 3, 120, 121, 122]);
}

#[test]
fn held_secret_refuses_replacement_past_its_bound() {
    let mut patcher = stopped_secret();
    assert_eq!(
        patcher.parts().1.put_content(b"wxyz"),
        Err(Halt::Built(Problem { path: FIELD, reason: Reason::Bound }))
    );
}

#[test]
fn malformed_old_bytes_halt_then_good_bytes_start() {
    const TEXT: &str = "record D { f: bool }";
    let (mut patcher, plan) = planned(TEXT, TEXT, "");
    assert_eq!(
        patcher.start(plan, D, Box::from([2])),
        Err(Halt::Decode(Refusal::Problem(Problem { path: FIELD, reason: Reason::Bool })))
    );
    restart(&mut patcher, plan, &[1]);
    finished(&mut patcher, "D", &[1]);
}

#[test]
fn old_misreads_map_the_node_without_changing_the_build() {
    const TEXT: &str = "record D { f: bool }";
    let (mut patcher, _) = started(TEXT, TEXT, "written D\n", "D", &[1]);
    let at = stopped(&mut patcher);
    let (old, new) = patcher.parts();
    assert_eq!(old.uint(at), Err(Halt::Misread(at)));
    assert_eq!(old.count(at), Err(Halt::Misread(at)));
    new.open().expect("record");
    new.put_bool(true).expect("ignored reader failure changes no build state");
    new.close().expect("record");
    finished(&mut patcher, "D", &[1]);
}

#[test]
fn wrong_put_kind_halts_and_allows_a_fresh_start() {
    const TEXT: &str = "record D { f: bool }";
    let (mut patcher, plan) = started(TEXT, TEXT, "written D.f\n", "D", &[1]);
    stopped(&mut patcher);
    assert_eq!(patcher.parts().1.put_uint(1), Err(Halt::Misbuilt(FIELD)));
    restart(&mut patcher, plan, &[1]);
}

#[test]
fn closing_a_record_reports_the_first_missing_field() {
    const TEXT: &str = "record D { a: u8 b: u8 }";
    let (mut patcher, plan) = started(TEXT, TEXT, "written D\n", "D", &[1, 2]);
    stopped(&mut patcher);
    let (_, new) = patcher.parts();
    new.open().expect("record");
    new.put_uint(1).expect("a");
    assert_eq!(new.close(), Err(Halt::Misbuilt(Path { decl: D, part: Part::Field(FieldIx::new(1)) })));
    restart(&mut patcher, plan, &[1, 2]);
}

#[test]
fn integers_at_each_width_fit_and_one_past_halts() {
    for (kind, maximum) in [("u8", u64::from(u8::MAX)), ("u16", u64::from(u16::MAX)), ("u32", u64::from(u32::MAX))] {
        let text = format!("record D {{ f: {kind} }}");
        let (mut patcher, plan) = planned("record D {}", &text, &format!("add D.f {kind} written\nwritten D.f\n"));
        patcher.start(plan, D, Box::default()).expect("start");
        stopped(&mut patcher);
        patcher.parts().1.put_uint(maximum).expect("at width");
        assert_eq!(patcher.next(), Ok(Step::Done));
        let bytes = skein_lib::bytes::copy_of(patcher.finish().expect("finish"));
        let schema = patcher.schema(2).expect("schema");
        let mut decoded = Value::with_room(CAPACITY.nodes);
        let root = decoded.decode(schema, D, bytes).expect("new decodes");
        assert_eq!(decoded.uint(decoded.field(root, F).expect("field")), Ok(maximum));
        patcher.start(plan, D, Box::default()).expect("start");
        stopped(&mut patcher);
        assert_eq!(
            patcher.parts().1.put_uint(maximum.checked_add(1).expect("below u64")),
            Err(Halt::Built(Problem { path: FIELD, reason: Reason::Bound }))
        );
        restart(&mut patcher, plan, b"");
    }
}

#[test]
fn content_checks_width_bound_and_utf8_like_the_new_decoder() {
    for (kind, bytes, reason) in [
        ("fixed 2", b"a".as_slice(), Reason::Short),
        ("fixed 2", b"abc".as_slice(), Reason::Trailing),
        ("bytes 2", b"abc".as_slice(), Reason::Bound),
        ("secret 2", b"abc".as_slice(), Reason::Bound),
        ("text 2", b"abc".as_slice(), Reason::Bound),
        ("text 2", b"\xff".as_slice(), Reason::Utf8),
    ] {
        let text = format!("record D {{ f: {kind} }}");
        let (mut patcher, plan) =
            started("record D {}", &text, &format!("add D.f {kind} written\nwritten D.f\n"), "D", b"");
        stopped(&mut patcher);
        assert_eq!(patcher.parts().1.put_content(bytes), Err(Halt::Built(Problem { path: FIELD, reason })));
        restart(&mut patcher, plan, b"");
    }
}

#[test]
fn content_at_its_bound_is_encoded_for_every_kind() {
    for (kind, expected) in [
        ("fixed 2", &[0xc3, 0xa9][..]),
        ("bytes 2", &[0, 0, 0, 2, 0xc3, 0xa9][..]),
        ("secret 2", &[0, 0, 0, 2, 0xc3, 0xa9][..]),
        ("text 2", &[0, 0, 0, 2, 0xc3, 0xa9][..]),
    ] {
        let text = format!("record D {{ f: {kind} }}");
        let (mut patcher, _) =
            started("record D {}", &text, &format!("add D.f {kind} written\nwritten D.f\n"), "D", b"");
        stopped(&mut patcher);
        patcher.parts().1.put_content("é".as_bytes()).expect("at bound");
        finished(&mut patcher, "D", expected);
    }
}

#[test]
fn written_lists_back_patch_the_count_and_options_require_one_value() {
    const TEXT: &str = "record D { f: list 2 option u8 }";
    let (mut patcher, _) = started(TEXT, TEXT, "written D.f\n", "D", &[0, 0, 0, 0]);
    stopped(&mut patcher);
    let (_, new) = patcher.parts();
    new.open().expect("list");
    new.put_none().expect("first absent");
    new.open().expect("second present");
    new.put_uint(9).expect("value");
    new.close().expect("option");
    new.close().expect("list");
    finished(&mut patcher, "D", &[0, 0, 0, 2, 0, 1, 9]);
}

#[test]
fn a_list_item_past_its_bound_halts() {
    const TEXT: &str = "record D { f: list 1 u8 }";
    let (mut patcher, plan) = started(TEXT, TEXT, "written D.f\n", "D", &[0, 0, 0, 0]);
    stopped(&mut patcher);
    let (_, new) = patcher.parts();
    new.open().expect("list");
    new.put_uint(9).expect("at bound");
    assert_eq!(new.put_uint(10), Err(Halt::Built(Problem { path: FIELD, reason: Reason::Bound })));
    restart(&mut patcher, plan, &[0, 0, 0, 0]);
}

#[test]
fn an_option_cannot_close_without_its_value() {
    const TEXT: &str = "record D { f: option u8 }";
    let (mut patcher, plan) = started(TEXT, TEXT, "written D.f\n", "D", &[0]);
    stopped(&mut patcher);
    let (_, new) = patcher.parts();
    new.open().expect("option");
    assert_eq!(new.close(), Err(Halt::Misbuilt(FIELD)));
    restart(&mut patcher, plan, &[0]);
}

#[test]
fn variant_building_checks_membership_and_payload_shape() {
    const TEXT: &str = "record R {} enum D { a b: R }";
    for (variant, open) in [(VariantIx::new(0), true), (VariantIx::new(1), false), (VariantIx::new(2), false)] {
        let (mut patcher, plan) = started(TEXT, TEXT, "written D\n", "D", &[0]);
        stopped(&mut patcher);
        let new = patcher.parts().1;
        let result = if open { new.open_variant(variant) } else { new.put_variant(variant) };
        assert_eq!(result, Err(Halt::Misbuilt(Path { decl: DeclIx::new(1), part: Part::Tag })));
        assert_eq!(patcher.next(), Err(Halt::Order));
        patcher.start(plan, DeclIx::new(1), Box::from([0])).expect("restart");
    }
}

#[test]
fn a_payloadless_variant_is_one_complete_value() {
    const TEXT: &str = "enum D { a b }";
    let (mut patcher, _) = started(TEXT, TEXT, "written D\n", "D", &[0]);
    stopped(&mut patcher);
    patcher.parts().1.put_variant(VariantIx::new(1)).expect("b");
    finished(&mut patcher, "D", &[1]);
}

#[test]
fn carrying_an_incompatible_type_is_misbuilt() {
    let (mut patcher, plan) =
        started("record D { f: u8 }", "record D { f: bool }", "retype D.f u8 bool\nwritten D.f\n", "D", &[1]);
    let at = stopped(&mut patcher);
    let (old, new) = patcher.parts();
    assert_eq!(new.carry(old, old.field(at, F).expect("field")), Err(Halt::Misbuilt(FIELD)));
    restart(&mut patcher, plan, &[1]);
}

#[test]
fn own_declaration_carry_reports_a_nonderived_field() {
    let (mut patcher, plan) =
        started("record D { f: u8 }", "record D { f: bool }", "retype D.f u8 bool\nwritten D\n", "D", &[1]);
    let at = stopped(&mut patcher);
    let (old, new) = patcher.parts();
    assert_eq!(new.carry(old, at), Err(Halt::Misbuilt(FIELD)));
    restart(&mut patcher, plan, &[1]);
}

#[test]
fn own_enum_carry_refuses_a_dropped_variant() {
    let (mut patcher, plan) = started("enum D { a b }", "enum D { a }", "drop D.b\nwritten D\n", "D", &[1]);
    let at = stopped(&mut patcher);
    let (old, new) = patcher.parts();
    assert_eq!(new.carry(old, at), Err(Halt::Misbuilt(TAG)));
    restart(&mut patcher, plan, &[1]);
}

#[test]
fn carrying_a_part_with_another_site_in_reach_is_misbuilt() {
    const TEXT: &str = "record R { f: u8 } record D { r: R }";
    let (mut patcher, _) = started(TEXT, TEXT, "written R\nwritten D\n", "D", &[1]);
    let at = stopped(&mut patcher);
    let (old, new) = patcher.parts();
    new.open().expect("record");
    assert_eq!(
        new.carry(old, old.field(at, FieldIx::new(1)).expect("r")),
        Err(Halt::Misbuilt(Path { decl: DeclIx::new(1), part: Part::Field(FieldIx::new(1)) }))
    );
    assert_eq!(patcher.next(), Err(Halt::Order));
}

#[test]
fn order_errors_end_the_run_and_start_recovers() {
    const TEXT: &str = "record D { f: u8 }";
    let (mut patcher, plan) = planned(TEXT, TEXT, "written D.f\n");
    assert_eq!(patcher.next(), Err(Halt::Order));
    patcher.start(plan, D, Box::from([1])).expect("start");
    assert_eq!(patcher.finish(), Err(Halt::Order));
    restart(&mut patcher, plan, &[1]);
    stopped(&mut patcher);
    assert_eq!(patcher.next(), Err(Halt::Order));
    restart(&mut patcher, plan, &[1]);
    stopped(&mut patcher);
    patcher.parts().1.put_uint(1).expect("one value");
    assert_eq!(patcher.parts().1.put_uint(2), Err(Halt::Order));
    restart(&mut patcher, plan, &[1]);
}

#[test]
fn every_builder_call_out_of_turn_is_order() {
    let (mut patcher, plan) = planned("record D {}", "record D {}", "written D\n");
    // Each call is made idle; each must stay idle.
    let (_, new) = patcher.parts();
    assert_eq!(new.put_uint(0), Err(Halt::Order));
    assert_eq!(new.put_bool(false), Err(Halt::Order));
    assert_eq!(new.put_content(b""), Err(Halt::Order));
    assert_eq!(new.put_none(), Err(Halt::Order));
    assert_eq!(new.put_variant(VariantIx::new(0)), Err(Halt::Order));
    assert_eq!(new.open_variant(VariantIx::new(0)), Err(Halt::Order));
    assert_eq!(new.open(), Err(Halt::Order));
    assert_eq!(new.close(), Err(Halt::Order));
    assert_eq!(new.refuse(RefusalIx::new(0)), Halt::Order);
    restart(&mut patcher, plan, b"");
}

#[test]
fn record_bytes_at_capacity_fit_and_one_past_halts() {
    const SMALL: Capacity = Capacity { bytes: 2, ..CAPACITY };
    const TEXT: &str = "record D { f: bytes 2 }";
    let (mut patcher, plan) = planned_at(SMALL, "record D { f: fixed 2 }", "record D { f: fixed 2 }", "");
    patcher.start(plan, D, Box::from([1, 2])).expect("at byte capacity");
    finished(&mut patcher, "D", &[1, 2]);
    assert_eq!(patcher.start(plan, D, Box::from([1, 2, 3])), Err(Halt::Room));
    restart(&mut patcher, plan, &[1, 2]);
    let (mut patcher, plan) = planned_at(SMALL, "record D {}", TEXT, "add D.f bytes 2 written\nwritten D.f\n");
    patcher.start(plan, D, Box::default()).expect("start");
    stopped(&mut patcher);
    assert_eq!(patcher.parts().1.put_content(b""), Err(Halt::Room));
    restart(&mut patcher, plan, b"");
}

#[test]
fn node_capacity_counts_copies_added_values_and_written_values() {
    const SMALL: Capacity = Capacity { nodes: 2, ..CAPACITY };
    let (mut patcher, plan) =
        planned_at(SMALL, "record D {}", "record D { f: option u8 }", "add D.f option u8 some 1\n");
    patcher.start(plan, D, Box::default()).expect("old fits");
    assert_eq!(patcher.next(), Err(Halt::Room));
    restart(&mut patcher, plan, b"");
    let (mut patcher, plan) =
        planned_at(SMALL, "record D {}", "record D { f: option u8 }", "add D.f option u8 written\nwritten D.f\n");
    patcher.start(plan, D, Box::default()).expect("old fits");
    stopped(&mut patcher);
    let (_, new) = patcher.parts();
    new.open().expect("option fits");
    assert_eq!(new.put_uint(1), Err(Halt::Room));
    restart(&mut patcher, plan, b"");
}

#[test]
fn two_passes_find_the_old_root_and_reuse_finished_bytes() {
    let (mut patcher, first) =
        planned("versioned record D { f: u8 }", "versioned record D { f: u16 }", "widen D.f u8 u16\n");
    patcher.keep(b"family f 3\nversioned record D { f: u32 }").expect("third");
    let second =
        patcher.plan(b"patch 1\nowner o 8\nfamily f 2 to 3\nwiden D.f u16 u32\n").expect("patch").expect("family");
    assert_eq!(patcher.plan_from(1), Some(first));
    assert_eq!(patcher.plan_from(2), Some(second));
    assert_eq!(patcher.source(first, D), Some(D));
    assert_eq!(patcher.source(second, D), Some(D));
    patcher.start(first, D, Box::from([0, 1, 9])).expect("first");
    finished(&mut patcher, "D", &[0, 2, 0, 9]);
    patcher.next_pass(second, D).expect("second");
    finished(&mut patcher, "D", &[0, 3, 0, 0, 0, 9]);
}

#[test]
fn next_pass_requires_finish_and_ends_the_run_on_error() {
    let (mut patcher, plan) = started("record D {}", "record D {}", "", "D", b"");
    assert_eq!(patcher.next(), Ok(Step::Done));
    assert_eq!(patcher.next_pass(plan, D), Err(Halt::Order));
    restart(&mut patcher, plan, b"");
}

#[test]
fn debug_exposes_neither_old_nor_new_record_bytes() {
    const SENTINEL: &[u8] = b"private!";
    let (mut patcher, _) =
        started("record D { f: fixed 8 }", "record D { f: fixed 8 }", "written D.f\n", "D", SENTINEL);
    let at = stopped(&mut patcher);
    let (old, new) = patcher.parts();
    new.put_content_of(old, old.field(at, F).expect("field")).expect("content");
    let debug = format!("{patcher:?}");
    assert!(!debug.contains("private!"), "no text content");
    assert!(!debug.contains("112, 114, 105, 118, 97, 116, 101, 33"), "no decimal byte content");
    finished(&mut patcher, "D", SENTINEL);
}

#[test]
fn value_counts_lists_and_names_each_parent() {
    let schema = schema::parse(b"family f 1\nrecord D { f: list 2 u8 }").expect("schema");
    let mut value = Value::with_room(CAPACITY.nodes);
    let root = value.decode(&schema, D, Box::from([0, 0, 0, 2, 7, 8])).expect("value");
    let list = value.field(root, F).expect("list");
    let mut items = value.items(list).expect("items");
    let item = value.next_item(&mut items).expect("item");
    assert_eq!(value.count(list), Ok(2));
    assert_eq!(value.parent(root), Ok(None));
    assert_eq!(value.parent(list), Ok(Some(root)));
    assert_eq!(value.parent(item), Ok(Some(list)));
    assert_eq!(value.count(root), Err(crate::value::Misread { node: root }));
}

#[test]
fn heap_arithmetic_refuses_overflow() {
    assert_eq!(
        Patcher::worst_case_heap(Capacity {
            versions: u32::MAX,
            patches: u32::MAX,
            text: u32::MAX,
            bytes: u32::MAX,
            nodes: u32::MAX
        }),
        None
    );
}

#[test]
fn carry_of_a_reshaped_declaration_is_misbuilt() {
    let (mut patcher, plan) = started("record D {}", "enum D { a }", "reshape D\nwritten D\n", "D", b"");
    let at = stopped(&mut patcher);
    let (old, new) = patcher.parts();
    assert_eq!(new.carry(old, at), Err(Halt::Misbuilt(TAG)));
    restart(&mut patcher, plan, b"");
}

#[test]
fn old_content_misread_from_a_put_ends_the_run() {
    const TEXT: &str = "record D { f: text 2 }";
    const BYTES: &[u8] = &[0, 0, 0, 1, 97];
    let (mut patcher, plan) = started(TEXT, TEXT, "written D.f\n", "D", BYTES);
    let at = stopped(&mut patcher);
    let (old, new) = patcher.parts();
    assert_eq!(new.put_content_of(old, at), Err(Halt::Misread(at)));
    restart(&mut patcher, plan, BYTES);
}

#[test]
fn a_nonstopped_put_ends_a_walking_record() {
    let (mut patcher, plan) = started("record D {}", "record D {}", "", "D", b"");
    assert_eq!(patcher.parts().1.open(), Err(Halt::Order));
    restart(&mut patcher, plan, b"");
}

#[test]
fn copied_subtrees_count_toward_the_new_node_capacity() {
    const SMALL: Capacity = Capacity { nodes: 4, ..CAPACITY };
    const OLD: &str = "record R { f: u8 } record D { r: R }";
    const NEW: &str = "record R { f: u8 } record D { r: R g: option u8 }";
    let (mut patcher, plan) = planned_at(SMALL, OLD, NEW, "add D.g option u8 some 1\n");
    patcher.start(plan, DeclIx::new(1), Box::from([9])).expect("three old nodes fit");
    assert_eq!(patcher.next(), Err(Halt::Room));
    assert_eq!(patcher.finish(), Err(Halt::Order));
}

#[test]
fn old_nodes_past_capacity_refuse_before_conversion() {
    const SMALL: Capacity = Capacity { nodes: 1, ..CAPACITY };
    const TEXT: &str = "record D { f: u8 }";
    let (mut patcher, plan) = planned_at(SMALL, TEXT, TEXT, "");
    assert_eq!(patcher.start(plan, D, Box::from([9])), Err(Halt::Decode(Refusal::Room)));
    assert_eq!(patcher.next(), Err(Halt::Order));
}

#[test]
fn a_dropped_root_has_no_new_value_to_build() {
    let (mut patcher, plan) = planned("record D {}", "", "drop D\n");
    assert_eq!(patcher.start(plan, D, Box::default()), Err(Halt::Misbuilt(Path { decl: D, part: Part::Tail })));
    assert_eq!(patcher.next(), Err(Halt::Order));
}

#[test]
fn holds_of_this_family_do_not_stop_a_versioned_record() {
    const TEXT: &str = "versioned record D { f: bytes 2 holds f R }";
    let (mut patcher, _) = started(TEXT, TEXT, "", "D", &[0, 1, 0, 0, 0, 1, 97]);
    finished(&mut patcher, "D", &[0, 2, 0, 0, 0, 1, 97]);
}

#[test]
fn a_new_record_at_its_byte_capacity_fits() {
    const SMALL: Capacity = Capacity { bytes: 2, ..CAPACITY };
    let (mut patcher, plan) = planned_at(SMALL, "record D {}", "record D { f: fixed 2 }", "add D.f fixed 2 0xabcd\n");
    patcher.start(plan, D, Box::default()).expect("empty old record");
    finished(&mut patcher, "D", &[0xab, 0xcd]);
}

#[test]
fn a_new_record_one_byte_past_capacity_halts() {
    const SMALL: Capacity = Capacity { bytes: 2, ..CAPACITY };
    let (mut patcher, plan) = planned_at(SMALL, "record D {}", "record D { f: fixed 3 }", "add D.f fixed 3 0xabcdef\n");
    patcher.start(plan, D, Box::default()).expect("empty old record");
    assert_eq!(patcher.next(), Err(Halt::Room));
    restart(&mut patcher, plan, b"");
}

#[test]
fn added_and_copied_nodes_at_capacity_fit() {
    const SMALL: Capacity = Capacity { nodes: 4, ..CAPACITY };
    const OLD: &str = "record R { f: u8 } record D { r: R }";
    const NEW: &str = "record R { f: u8 } record D { r: R g: option u8 }";
    let (mut patcher, plan) = planned_at(SMALL, OLD, NEW, "add D.g option u8 none\n");
    patcher.start(plan, DeclIx::new(1), Box::from([9])).expect("old fits");
    finished(&mut patcher, "D", &[9, 0]);
}

#[test]
fn a_written_payload_versions_its_record_and_completes_its_parent() {
    const OLD: &str = "record R { f: u8 } enum E { a b: R } record D { e: E after: bool }";
    const NEW: &str = "versioned record R { f: u8 } enum E { a b: R } record D { e: E after: bool }";
    let (mut patcher, _) = started(OLD, NEW, "versioned R\nwritten D\n", "D", &[0, 0]);
    stopped(&mut patcher);
    let (_, new) = patcher.parts();
    new.open().expect("D");
    new.open_variant(VariantIx::new(1)).expect("E.b and R");
    new.put_uint(9).expect("R.f");
    new.close().expect("R closes E too");
    new.put_bool(true).expect("D.after");
    new.close().expect("D");
    finished(&mut patcher, "D", &[1, 0, 2, 9, 1]);
}

#[test]
fn deeply_wrapped_new_values_halt_at_the_node_capacity() {
    // A nonrecursive walk must count the added wrappers, including ones not
    // present in the old value. Sixteen levels are enough to exceed this room.
    const DEPTH: usize = 16;
    const SMALL: Capacity = Capacity { nodes: 8, ..CAPACITY };
    let kind = format!("{}u8", "option ".repeat(DEPTH));
    let new = format!("record D {{ f: {kind} }}");
    let lines = format!("wrap D.f u8 {kind}\n");
    let (mut patcher, plan) = planned_at(SMALL, "record D { f: u8 }", &new, &lines);
    patcher.start(plan, D, Box::from([9])).expect("old fits");
    assert_eq!(patcher.next(), Err(Halt::Room));
    restart(&mut patcher, plan, &[9]);
}

#[test]
fn a_driver_can_start_fresh_after_an_old_reader_halt() {
    const TEXT: &str = "record D { f: bool }";
    let (mut patcher, plan) = started(TEXT, TEXT, "written D.f\n", "D", &[1]);
    let at = stopped(&mut patcher);
    assert_eq!(patcher.parts().0.uint(at), Err(Halt::Misread(at)));
    patcher.start(plan, D, Box::from([0])).expect("driver starts the next record after its halt");
    stopped(&mut patcher);
    patcher.parts().1.put_bool(true).expect("answer to new stop");
    finished(&mut patcher, "D", &[1]);
}

#[test]
fn a_wrong_put_at_an_enum_field_names_its_tag() {
    const TEXT: &str = "enum E { a } record D { f: E }";
    let (mut patcher, _) = started(TEXT, TEXT, "written D.f\n", "D", &[0]);
    stopped(&mut patcher);
    assert_eq!(patcher.parts().1.put_uint(0), Err(Halt::Misbuilt(TAG)));
    assert_eq!(patcher.next(), Err(Halt::Order));
}

#[test]
fn an_enum_item_past_a_list_bound_names_the_list_field() {
    const TEXT: &str = "enum E { a } record D { f: list 1 E }";
    let (mut patcher, _) = started(TEXT, TEXT, "written D.f\n", "D", &[0, 0, 0, 0]);
    stopped(&mut patcher);
    let (_, new) = patcher.parts();
    new.open().expect("list");
    new.put_variant(VariantIx::new(0)).expect("first item");
    assert_eq!(
        new.put_variant(VariantIx::new(0)),
        Err(Halt::Built(Problem { path: Path { decl: DeclIx::new(1), part: Part::Field(F) }, reason: Reason::Bound }))
    );
    assert_eq!(patcher.next(), Err(Halt::Order));
}

// Store transcript ceilings, with the memory story's two texts and one patch.
const LARGE: Capacity = Capacity { versions: 2, patches: 1, text: 256, nodes: 1 << 21, bytes: 1 << 22 };
// Nine doublings expose several entry and output growth steps before the ceiling.
const GROWTH_CEILING: u32 = 1 << 9;
const NODE_CEILING: Capacity = Capacity { nodes: GROWTH_CEILING, bytes: GROWTH_CEILING + 5, ..LARGE };
const BYTE_CEILING: Capacity = Capacity { nodes: GROWTH_CEILING - 3, bytes: GROWTH_CEILING, ..LARGE };
// The report's store configuration; texts and kept slots dominate its fixed price.
const STORE: Capacity = Capacity { versions: 4, patches: 4, text: 65536, ..LARGE };
// The memory fixture's root with two scalar fields fits these ceilings.
const SMALL: Capacity = Capacity { nodes: 8, bytes: 64, ..LARGE };

fn run_price(nodes: u32, bytes: u32) -> u64 {
    use skein_lib::{Buffer, Stack};
    Value::worst_case_heap(nodes)
        .expect("entries")
        .checked_add(Buffer::worst_case(bytes).expect("output"))
        .expect("run fits u64")
        .checked_add(Stack::<super::Frame>::worst_case(nodes).expect("walk"))
        .expect("run fits u64")
        .checked_add(Stack::<super::Open>::worst_case(nodes).expect("build"))
        .expect("run fits u64")
}

fn eager_price(capacity: Capacity) -> u64 {
    use skein_lib::List;
    let mut total = run_price(capacity.nodes, capacity.bytes);
    for bytes in [
        u64::from(capacity.bytes).checked_mul(2).expect("records"),
        List::<schema::Schema>::worst_case(capacity.versions).expect("schemas"),
        List::<crate::patch::plan::Plan>::worst_case(capacity.patches).expect("plans"),
        schema::worst_case_heap(capacity.text)
            .expect("schema scratch")
            .checked_mul(u64::from(capacity.versions))
            .expect("schemas"),
        crate::patch::plan::worst_case_heap(capacity.text)
            .expect("plan scratch")
            .checked_mul(u64::from(capacity.patches.max(1)))
            .expect("plans"),
    ] {
        total = total.checked_add(bytes).expect("eager price fits u64");
    }
    total
}

#[test]
fn growth_adds_at_most_a_sixteenth_to_the_price() {
    for capacity in [LARGE, NODE_CEILING, BYTE_CEILING, CAPACITY, STORE, SMALL] {
        let eager = eager_price(capacity);
        let run = run_price(capacity.nodes, capacity.bytes);
        let growing = Patcher::worst_case_heap(capacity).expect("price");
        assert!(eager <= growing);
        assert!(growing <= eager + run.div_ceil(16));
    }
}

fn run_held(patcher: &Patcher) -> u64 {
    use skein_lib::{Buffer, List};
    Value::worst_case_heap(patcher.old.value.room())
        .expect("old")
        .checked_add(Value::worst_case_heap(patcher.build.reserve.room()).expect("reserved entries"))
        .expect("held fits u64")
        .checked_add(Buffer::worst_case(patcher.build.out.capacity()).expect("output"))
        .expect("held fits u64")
        .checked_add(List::<super::Frame>::worst_case(patcher.build.walk.capacity()).expect("walk"))
        .expect("held fits u64")
        .checked_add(List::<super::Open>::worst_case(patcher.build.opened.capacity()).expect("build"))
        .expect("held fits u64")
}

#[derive(Debug, PartialEq, Eq)]
struct Allocations {
    entries: [(u32, *const ()); 2],
    out: (u32, *const ()),
    walk: (u32, *const ()),
    opened: (u32, *const ()),
}

fn allocations(patcher: &Patcher) -> Allocations {
    let mut entries = [patcher.old.value.allocation(), patcher.build.reserve.allocation()];
    entries.sort();
    Allocations {
        entries,
        out: (patcher.build.out.capacity(), patcher.build.out.as_slice().as_ptr().cast()),
        walk: (patcher.build.walk.capacity(), patcher.build.walk.as_slice().as_ptr().cast()),
        opened: (patcher.build.opened.capacity(), patcher.build.opened.as_slice().as_ptr().cast()),
    }
}

fn assert_run_rooms(patcher: &Patcher) {
    let tables = allocations(patcher);
    let capacity = patcher.build.tables.capacity;
    for (room, _) in tables.entries {
        assert!(room <= capacity.nodes, "entry room stays under the node cap");
    }
    assert!(tables.walk.0 <= capacity.nodes);
    assert!(tables.opened.0 <= capacity.nodes);
    assert!(tables.out.0 <= capacity.bytes);
}

fn list_bytes(items: u32) -> Box<[u8]> {
    // A list count followed by one byte per item; this fixture is unversioned.
    let length = usize::try_from(items).expect("items").checked_add(size_of::<u32>()).expect("wire fits usize");
    let mut writer = skein_lib::Writer::new(length);
    writer.put(&items.to_be_bytes()).expect("count fits");
    for _ in 0..items {
        writer.put(&[7]).expect("item fits");
    }
    writer.finish()
}

#[test]
fn small_records_hold_at_most_twice_their_run_price() {
    const TEXT: &str = "record D { f: list 1024 u8 }";
    // Each row crosses a small doubling, staying below LARGE's sixteenth.
    const ITEMS: &[u32] = &[0, 1, 3, 7, 15, 31, 63, 127, 255, 511, 1023];
    const PARENTS: u32 = 2; // Root and list, besides their items.
    let (mut patcher, plan) = planned_at(LARGE, TEXT, TEXT, "");
    assert_eq!(run_held(&patcher), 0, "new, keep and plan allocate no run table");
    for &items in ITEMS {
        let wire = list_bytes(items);
        let bytes = u32::try_from(wire.len()).expect("wire");
        let bound = run_price(items + PARENTS, bytes) * 2;
        patcher.start(plan, D, wire).expect("start");
        assert!(run_held(&patcher) <= bound);
        assert_eq!(patcher.next(), Ok(Step::Done));
        assert!(run_held(&patcher) <= bound);
        patcher.finish().expect("finish");
        let before = allocations(&patcher);
        patcher.start(plan, D, list_bytes(items)).expect("reuse");
        assert_eq!(allocations(&patcher), before);
        assert_eq!(patcher.next(), Ok(Step::Done));
        assert_eq!(allocations(&patcher), before);
        patcher.finish().expect("finish");
        assert_eq!(allocations(&patcher), before);
    }
}

#[test]
fn zero_and_one_node_rooms_keep_their_refusals() {
    // An empty record takes one node and no bytes, testing room before reading.
    for (nodes, expected) in [(0, Err(Halt::Decode(Refusal::Room))), (1, Ok(()))] {
        let (mut patcher, plan) =
            planned_at(Capacity { nodes, bytes: 0, ..CAPACITY }, "record D {}", "record D {}", "");
        assert_eq!(patcher.start(plan, D, Box::default()), expected);
    }
}

#[test]
fn tiny_ceilings_allocate_only_their_configured_room() {
    // A record and its byte take two nodes, below the first sixteenth.
    const TINY: Capacity = Capacity { nodes: 2, bytes: 1, ..CAPACITY };
    let (mut patcher, plan) = planned_at(TINY, "record D { f: u8 }", "record D { f: u8 }", "");
    patcher.start(plan, D, Box::from([7])).expect("tiny start");
    assert_eq!(patcher.next(), Ok(Step::Done));
    assert_eq!(run_held(&patcher), run_price(TINY.nodes, TINY.bytes));
    let before = allocations(&patcher);
    patcher.start(plan, D, Box::from([7])).expect("tiny reuse");
    assert_eq!(allocations(&patcher), before);
}

#[test]
fn nested_walks_grow_both_stacks_and_reuse_them() {
    // Three nested lists and their leaf, inside the root: five nodes.
    const NESTED: &str = "record D { f: list 1 list 1 list 1 u8 }";
    const WIRE: &[u8] = &[0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 7];
    let (mut patcher, plan) = planned_at(NODE_CEILING, NESTED, NESTED, "written D.f\n");
    patcher.start(plan, D, WIRE.into()).expect("nested old");
    let before = allocations(&patcher);
    let at = stopped(&mut patcher);
    let (old, build) = patcher.parts();
    build.carry(old, old.field(at, F).expect("list")).expect("nested carry");
    assert_eq!(patcher.next(), Ok(Step::Done));
    assert_eq!(patcher.finish().expect("finish"), WIRE);
    assert_eq!(allocations(&patcher), before, "nested walk reuses reserved stacks");
    assert!(patcher.build.walk.capacity() > 2);
    assert!(patcher.build.opened.capacity() > 2);
}

#[test]
fn written_nesting_grows_live_stacks_and_reuses_grown_old_entries() {
    // Six nested options and their leaf outgrow the two-node old record.
    const DEPTH: u32 = 6;
    const NEW: &str = "record D { f: option option option option option option u8 }";
    const LINES: &str = "retype D.f bool option option option option option option u8\nwritten D.f\n";
    const WIRE: &[u8] = &[1, 1, 1, 1, 1, 1, 7];
    let (mut patcher, plan) =
        planned_at(Capacity { patches: CAPACITY.patches, ..NODE_CEILING }, "record D { f: bool }", NEW, LINES);
    patcher.start(plan, D, Box::from([1])).expect("old");
    stopped(&mut patcher);
    for _ in 0..DEPTH {
        patcher.build.open().expect("open option");
        assert!(run_held(&patcher) <= run_price(patcher.build.nodes, patcher.build.out.len()) * 2);
    }
    patcher.build.put_uint(7).expect("leaf");
    assert!(run_held(&patcher) <= run_price(patcher.build.nodes, patcher.build.out.len()) * 2);
    for _ in 0..DEPTH {
        patcher.build.close().expect("close option");
    }
    assert_eq!(patcher.next(), Ok(Step::Done));
    assert_eq!(patcher.finish().expect("finish"), WIRE);
    assert!(patcher.build.opened.capacity() >= DEPTH);
    assert!(patcher.build.walk.capacity() >= DEPTH);
    let before = allocations(&patcher);
    // Decoding the larger old shape fills its replacement entries for the first time.
    let identity = patcher.identity(2).expect("identity");
    patcher.start(identity, D, WIRE.into()).expect("larger old record");
    let grown = allocations(&patcher);
    assert_eq!(grown.walk, before.walk);
    assert_eq!(grown.opened, before.opened);
    assert_eq!(grown.out, before.out);
    assert_run_rooms(&patcher);
    assert_eq!(patcher.next(), Ok(Step::Done));
    assert_eq!(patcher.finish().expect("finish"), WIRE);
    let before = allocations(&patcher);
    for wire in [WIRE, &[0][..]] {
        patcher.start(identity, D, wire.into()).expect("reuse grown entries");
        assert_eq!(allocations(&patcher), before);
        assert_eq!(patcher.next(), Ok(Step::Done));
        assert_eq!(patcher.finish().expect("finish"), wire);
        assert_eq!(allocations(&patcher), before);
    }
}

#[test]
fn a_larger_next_pass_grows_tables_and_smaller_passes_reuse_them() {
    const NEW: &str = "versioned record D { f: u8 g: option u8 }";
    const LINES: &str = "add D.g option u8 some 9\n";
    const FIRST: &[u8] = &[0, 2, 7, 1, 9];
    const LAST: &[u8] = &[0, 3, 7, 1, 9];
    let (mut patcher, first) = planned_at(
        Capacity { versions: CAPACITY.versions, patches: CAPACITY.patches, ..NODE_CEILING },
        "versioned record D { f: u8 }",
        NEW,
        LINES,
    );
    patcher.keep(b"family f 3\nversioned record D { f: u8 g: option u8 }").expect("third version");
    let second = patcher.plan(b"patch 1\nowner o 8\nfamily f 2 to 3\n").expect("patch").expect("family");
    patcher.start(first, D, Box::from([0, 1, 7])).expect("first");
    assert_eq!(patcher.next(), Ok(Step::Done));
    assert_eq!(patcher.finish().expect("first finish"), FIRST);
    patcher.next_pass(second, D).expect("larger second pass");
    assert_run_rooms(&patcher);
    assert_eq!(patcher.next(), Ok(Step::Done));
    assert_eq!(patcher.finish().expect("last finish"), LAST);
    assert_run_rooms(&patcher);
    let before = allocations(&patcher);
    for wire in [FIRST, &[0, 2, 7, 0][..]] {
        patcher.start(second, D, wire.into()).expect("same or smaller old record");
        assert_eq!(allocations(&patcher), before);
        assert_eq!(patcher.next(), Ok(Step::Done));
        patcher.finish().expect("finish");
        assert_eq!(allocations(&patcher), before);
    }
}

#[test]
fn refused_growth_keeps_its_room_for_smaller_records() {
    const TEXT: &str = "record D { f: list 1024 u8 }";
    // The count fits, but an extra byte after the list refuses at the root's tail.
    const ITEMS: u32 = 23;
    let (mut patcher, plan) = planned_at(NODE_CEILING, TEXT, TEXT, "");
    let bytes = list_bytes(ITEMS);
    let mut wire = skein_lib::Writer::new(bytes.len().checked_add(1).expect("wire and trailing byte"));
    wire.put(&bytes).expect("list fits");
    wire.put(&[0]).expect("tail fits");
    assert_eq!(
        patcher.start(plan, D, wire.finish()),
        Err(Halt::Decode(Refusal::Problem(Problem {
            path: Path { decl: D, part: Part::Tail },
            reason: Reason::Trailing,
        })))
    );
    let old_entries = patcher.old.value.allocation();
    patcher.start(plan, D, list_bytes(ITEMS - 1)).expect("smaller record after refusal");
    assert_eq!(patcher.old.value.allocation(), old_entries, "refused decode's entries are reused");
    assert_run_rooms(&patcher);
    // The refused decode did not run the walk; let that table fill once.
    assert_eq!(patcher.next(), Ok(Step::Done));
    patcher.finish().expect("finish");
    assert_run_rooms(&patcher);
    let before = allocations(&patcher);
    for items in [ITEMS - 1, ITEMS - 2] {
        patcher.start(plan, D, list_bytes(items)).expect("reuse after refusal");
        assert_eq!(allocations(&patcher), before);
        assert_eq!(patcher.next(), Ok(Step::Done));
        patcher.finish().expect("finish");
        assert_eq!(allocations(&patcher), before);
    }
}

#[test]
fn entry_and_output_room_grow_in_several_steps() {
    const TEXT: &str = "record D { f: list 1024 u8 }";
    // These item counts visit the small growth steps before either ceiling.
    const ITEMS: &[u32] = &[0, 1, 3, 7, 15, 23, 31, 63, 127, 255];
    const MIN_STEPS: u32 = 3; // codec.md, 11.8's growth story.
    for capacity in [NODE_CEILING, BYTE_CEILING] {
        let (mut patcher, plan) = planned_at(capacity, TEXT, TEXT, "");
        let mut entries = 0;
        let mut output = 0;
        let mut entry_steps = 0;
        let mut output_steps = 0;
        for &items in ITEMS {
            patcher.start(plan, D, list_bytes(items)).expect("growing record");
            if patcher.old.value.room() > entries {
                entries = patcher.old.value.room();
                entry_steps += 1;
            }
            if patcher.build.out.capacity() > output {
                output = patcher.build.out.capacity();
                output_steps += 1;
            }
            assert_eq!(patcher.next(), Ok(Step::Done));
            patcher.finish().expect("finish");
        }
        assert!(entry_steps >= MIN_STEPS);
        assert!(output_steps >= MIN_STEPS);
    }
}
