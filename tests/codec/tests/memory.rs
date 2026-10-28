//! A decoded value at every ceiling attains its declared heap worst case
//! (codec.md, sections 4 and 6; testing-strategy.md, section 6).

use skein_heap::Counting;

#[global_allocator]
static HEAP: Counting = Counting;

#[cfg(test)]
mod tests {
    use skein_codec_tests::v1::{CEILINGS, Effect, Field, FieldParts, Report, ReportParts};
    use skein_heap::Meter;
    use skein_lib::{Duration, List, Reader, Writer};

    fn full_field() -> Field {
        Field::new(&CEILINGS, FieldParts { name: Box::from(&b"abcd"[..]), value: Box::from(&[1, 2, 3, 4][..]) })
            .expect("field at ceilings")
    }

    fn full_report() -> Report {
        let mut fields = List::with_capacity(CEILINGS.report_fields);
        fields.push(full_field()).expect("first field room");
        fields.push(full_field()).expect("second field room");
        Report::new(
            &CEILINGS,
            ReportParts {
                title: Box::from(&b"abcdefgh"[..]),
                fields,
                effect: Effect::Write,
                present: Some(true),
                nonce: [0xff, 0xff],
                age: Duration::from_nanos(u64::MAX),
            },
        )
        .expect("report at ceilings")
    }

    #[test]
    fn full_record_decoder_attains_and_stays_within_heap_worst_case() {
        let value = full_report();
        assert_eq!(
            value.measure(),
            u32::try_from(Report::worst_case_bytes(&CEILINGS).expect("wire bound")).expect("u32 ceiling")
        );
        let mut writer = Writer::new(usize::try_from(value.measure()).expect("u32 fits usize"));
        value.encode(&mut writer).expect("measured room");
        let wire = writer.finish();
        drop(value);

        let bound = Report::worst_case_heap(&CEILINGS).expect("heap bound");
        let meter = Meter::new();
        meter.start();
        let decoded = Report::decode(&CEILINGS, &mut Reader::new(&wire)).expect("full record");
        let measured = meter.end();
        assert_eq!(measured.peak(), bound, "the full value attains the heap bound");
        assert_eq!(measured.held(), bound, "the decoded value owns all of its heap");
        drop(decoded);
        assert_eq!(meter.held(), 0, "decoded allocations released");
    }

    fn full_numbers() -> skein_codec_tests::every::Numbers {
        use skein_codec_tests::every::{CEILINGS, Numbers, NumbersParts};
        Numbers::new(
            &CEILINGS,
            NumbersParts {
                small: u8::MAX,
                short: u16::MAX,
                word: u32::MAX,
                long: u64::MAX,
                flag: true,
                wait: Duration::from_nanos(u64::MAX),
                tag: [0xff; 3],
            },
        )
        .expect("numbers at ceilings")
    }

    fn full_stamp() -> skein_codec_tests::every::Stamp {
        use skein_codec_tests::every::{CEILINGS, Stamp, StampParts};
        Stamp::new(
            &CEILINGS,
            StampParts {
                at: u64::MAX,
                note: Some(vec![b'a'; usize::try_from(CEILINGS.stamp_note_some).expect("bound")].into_boxed_slice()),
            },
        )
        .expect("stamp at ceilings")
    }

    fn full_strings() -> skein_codec_tests::every::Strings {
        use skein_codec_tests::every::{CEILINGS, Strings, StringsParts};
        let bytes = |bound| vec![b'a'; usize::try_from(bound).expect("bound")].into_boxed_slice();
        Strings::new(
            &CEILINGS,
            StringsParts {
                raw: bytes(CEILINGS.strings_raw),
                key: skein_lib::Secret::new(bytes(CEILINGS.strings_key)),
                words: bytes(CEILINGS.strings_words),
            },
        )
        .expect("strings at ceilings")
    }

    fn full_every() -> skein_codec_tests::every::Every {
        use skein_codec_tests::every::{Blank, BlankParts, CEILINGS, Every, EveryParts, Light, Nest, NestParts, Shape};
        let bytes = |bound| vec![b'a'; usize::try_from(bound).expect("bound")].into_boxed_slice();
        let mut blanks = List::with_capacity(CEILINGS.nest_blanks);
        for _ in 0..CEILINGS.nest_blanks {
            blanks.push(Blank::new(&CEILINGS, BlankParts {}).expect("blank")).expect("room");
        }
        let mut lights = List::with_capacity(CEILINGS.nest_lights);
        for _ in 0..CEILINGS.nest_lights {
            lights.push(Light::Green).expect("room");
        }
        let mut maybe_lights = List::with_capacity(CEILINGS.nest_maybe_lights);
        for _ in 0..CEILINGS.nest_maybe_lights {
            maybe_lights.push(Some(Light::Green)).expect("room");
        }
        let mut grid = List::with_capacity(CEILINGS.nest_grid);
        for _ in 0..CEILINGS.nest_grid {
            let mut row = List::with_capacity(CEILINGS.nest_grid_item);
            for _ in 0..CEILINGS.nest_grid_item {
                row.push(u16::MAX).expect("room");
            }
            grid.push(row).expect("room");
        }
        let mut maybe_list = List::with_capacity(CEILINGS.nest_maybe_list_some);
        for _ in 0..CEILINGS.nest_maybe_list_some {
            maybe_list.push(u8::MAX).expect("room");
        }
        let mut shapes = List::with_capacity(CEILINGS.nest_shapes);
        for _ in 0..CEILINGS.nest_shapes {
            shapes.push(Shape::Strings(Box::new(full_strings()))).expect("room");
        }
        let mut secrets = List::with_capacity(CEILINGS.nest_secrets);
        for _ in 0..CEILINGS.nest_secrets {
            secrets.push(skein_lib::Secret::new(bytes(CEILINGS.nest_secrets_item))).expect("room");
        }
        let mut stamps = List::with_capacity(CEILINGS.nest_stamps);
        for _ in 0..CEILINGS.nest_stamps {
            stamps.push(full_stamp()).expect("room");
        }
        let mut pairs = List::with_capacity(CEILINGS.nest_pairs);
        for _ in 0..CEILINGS.nest_pairs {
            pairs.push([0xff; 2]).expect("room");
        }
        let nest = Nest::new(
            &CEILINGS,
            NestParts {
                blanks,
                lights,
                maybe_lights,
                grid,
                maybe_list: Some(maybe_list),
                maybe_numbers: Some(full_numbers()),
                shapes,
                secrets,
                stamps,
                pairs,
            },
        )
        .expect("nest at ceilings");
        Every::new(
            &CEILINGS,
            EveryParts {
                numbers: full_numbers(),
                strings: full_strings(),
                light: Light::Green,
                shape: Shape::Strings(Box::new(full_strings())),
                stamp: full_stamp(),
                nest,
                empty: Blank::new(&CEILINGS, BlankParts {}).expect("blank"),
                held: bytes(CEILINGS.every_held),
                items: bytes(CEILINGS.every_items),
            },
        )
        .expect("every at ceilings")
    }

    #[test]
    fn a_value_grows_on_first_decode_and_reuses_its_tables_on_the_next() {
        use skein_codec::value::Value;
        use skein_codec_tests::every::{CEILINGS, Every};
        // Every's largest tree has 99 nodes; room also admits every other fixture.
        const ROOM: u32 = 256;
        let schema = skein_codec::schema::parse(include_bytes!("../schema/every-v1.schema")).expect("schema");
        let root = schema.find(b"Every").expect("root");
        let full = full_every();
        assert_eq!(u64::from(full.measure()), Every::worst_case_bytes(&CEILINGS).expect("wire bound"));
        let mut writer = Writer::new(usize::try_from(full.measure()).expect("wire length"));
        full.encode(&mut writer).expect("measured room");
        let wire = writer.finish();
        drop(full);

        let meter = Meter::new();
        meter.start();
        let mut value = Value::with_room(ROOM);
        let measured = meter.end();
        let bound = Value::worst_case_heap(ROOM).expect("heap bound");
        assert_eq!(measured.peak(), 0, "empty room allocates nothing");
        assert_eq!(measured.held(), 0, "empty room holds nothing");

        let meter = Meter::new();
        meter.start();
        value.decode(&schema, root, wire).expect("full every decodes");
        let measured = meter.end();
        assert!(measured.held() <= bound, "grown entries stay within their room");
        // Realloc counts both the old and the new buffer at the growth peak.
        assert!(measured.peak() <= 2 * bound, "first decode growth stays bounded");

        // Reuse the allocated room after moving out its previous input.
        let previous = value.take();
        let wire = previous.expose().into();
        drop(previous);
        let meter = Meter::new();
        meter.start();
        value.decode(&schema, root, wire).expect("reused every decodes");
        let measured = meter.end();
        assert_eq!(measured.peak(), 0, "decoding again allocates nothing");
        assert_eq!(measured.held(), 0, "decoding again holds no new allocation");
    }
}

#[cfg(test)]
mod patch_memory {
    use skein_codec::patch::{Capacity, Patcher, Step};
    use skein_codec::schema::{DeclIx, FieldIx};
    use skein_heap::{Meter, Span};

    // Two short texts, one patch, and a root with two scalar fields.
    const CAPACITY: Capacity = Capacity { versions: 2, patches: 1, text: 256, bytes: 64, nodes: 8 };
    const OLD: &[u8] = b"family f 1\nversioned record D { f: u8 held: bytes 3 holds other R }";
    const NEW: &[u8] = b"family f 2\nversioned record D { f: u16 held: bytes 3 holds other R }";
    const PATCH: &[u8] = b"patch 1\nowner o 1\nfamily f 1 to 2\nwiden D.f u8 u16\nwritten D.f\n";
    const WIRE: &[u8] = &[0, 1, 7, 0, 0, 0, 1, 97];
    const EXPECTED: &[u8] = &[0, 2, 0, 7, 0, 0, 0, 1, 97];
    // Repeated records make retention observable after the first growth.
    const RECORDS: u32 = 128;

    // Store-sized transcript ceilings (codec.md, 11.8).
    const LARGE: Capacity = Capacity { nodes: 1 << 21, bytes: 1 << 22, ..CAPACITY };
    // The contract's capacity-independent allowance for the run's floor.
    const FLOOR: u64 = 65_536;
    // A list's record and list take two nodes; its version and count take six bytes.
    const LIST_NODES: u32 = 2;
    const LIST_BYTES: u32 = 6;
    // Nine doublings leave several small growth steps before the full allocation.
    const CEILING: u32 = 1 << 9;
    const NODE_CAPACITY: Capacity = Capacity { nodes: CEILING, bytes: CEILING + 5, ..CAPACITY };
    const BYTE_CAPACITY: Capacity = Capacity { nodes: CEILING - 3, bytes: CEILING, ..CAPACITY };
    // The schema admits the item count one past either machine ceiling.
    const LIST_BOUND: u32 = CEILING;
    const LIST_OLD: &[u8] = b"family f 1\nversioned record D { f: list 512 u8 }";
    const LIST_NEW: &[u8] = b"family f 2\nversioned record D { f: list 512 u8 }";
    const LIST_PATCH: &[u8] = b"patch 1\nowner o 1\nfamily f 1 to 2\n";
    // Small lists visit both sides of successive entry and output growth steps.
    const RISING: &[u32] = &[0, 1, 3, 7, 15, 23, 31, 63, 127, 255];

    fn call<T, F: FnOnce() -> T>(meter: &Meter, peak: &mut u64, bound: u64, reused: bool, operation: F) -> T {
        let before = meter.held();
        let span = Span::start();
        let result = operation();
        let grown = span.end();
        let at_peak = before
            .checked_add(u64::try_from(grown.peak).expect("span peak is nonnegative"))
            .expect("call peak fits u64");
        *peak = (*peak).max(at_peak);
        assert!(at_peak <= bound, "call peak {at_peak} exceeds {bound}");
        if reused {
            assert_eq!(grown.peak, 0, "reused tables allocate nothing beyond the input box");
        }
        result
    }

    fn setup(
        meter: &Meter,
        capacity: Capacity,
        bound: u64,
        old: &[u8],
        new: &[u8],
        patch: &[u8],
    ) -> (Patcher, skein_codec::patch::PlanIx, u64) {
        meter.start();
        let mut patcher = Patcher::new(capacity);
        patcher.keep(old).expect("old");
        patcher.keep(new).expect("new");
        let plan = patcher.plan(patch).expect("patch").expect("family");
        let measured = meter.end();
        assert!(measured.peak() <= bound, "setup's peak includes parsing and planning scratch");
        (patcher, plan, measured.peak())
    }

    fn small_records(capacity: Capacity, bound: u64, first_answers_grow: bool) {
        let meter = Meter::new();
        let (mut patcher, plan, mut peak) = setup(&meter, capacity, bound, OLD, NEW, PATCH);
        for record in 0..RECORDS {
            let reused = record > 0;
            let wire = Box::from(WIRE);
            call(&meter, &mut peak, bound, reused, || patcher.start(plan, DeclIx::new(0), wire)).expect("record");
            let reused = reused || !first_answers_grow;
            let at = call(&meter, &mut peak, bound, reused, || patcher.next()).expect("written");
            let at = match at {
                Step::Written { at, .. } => at,
                Step::Held { .. } | Step::Done => unreachable!("written field first"),
            };
            call(&meter, &mut peak, bound, reused, || {
                let (old, new) = patcher.parts();
                let field = old.field(at, FieldIx::new(0)).expect("f");
                new.put_uint(old.uint(field).expect("uint"))
            })
            .expect("written answer");
            let at = call(&meter, &mut peak, bound, reused, || patcher.next()).expect("held");
            let at = match at {
                Step::Held { at, .. } => at,
                Step::Written { .. } | Step::Done => unreachable!("held field second"),
            };
            call(&meter, &mut peak, bound, reused, || {
                let (old, new) = patcher.parts();
                new.carry(old, at)
            })
            .expect("held answer");
            assert_eq!(call(&meter, &mut peak, bound, reused, || patcher.next()), Ok(Step::Done));
            call(&meter, &mut peak, bound, reused, || assert_eq!(patcher.finish().expect("finish"), EXPECTED));
        }

        drop(patcher);
        assert_eq!(meter.held(), 0, "all machine storage released");
    }

    #[test]
    fn patcher_stays_within_its_heap_bound_after_every_call() {
        small_records(CAPACITY, Patcher::worst_case_heap(CAPACITY).expect("bound"), false);
    }

    #[test]
    fn large_ceilings_keep_small_records_near_their_own_price() {
        let small = Capacity { nodes: CAPACITY.nodes, bytes: CAPACITY.bytes, ..LARGE };
        small_records(LARGE, Patcher::worst_case_heap(small).expect("small bound") + FLOOR, true);
    }

    fn list_wire(items: u32, version: u16) -> Box<[u8]> {
        let mut wire = Vec::with_capacity(
            usize::try_from(items.checked_add(LIST_BYTES).expect("list length fits u32")).expect("length"),
        );
        wire.extend_from_slice(&version.to_be_bytes());
        wire.extend_from_slice(&items.to_be_bytes());
        wire.resize(usize::try_from(items.checked_add(LIST_BYTES).expect("list length fits u32")).expect("length"), 7);
        wire.into_boxed_slice()
    }

    fn list_record(
        patcher: &mut Patcher,
        plan: skein_codec::patch::PlanIx,
        meter: &Meter,
        peak: &mut u64,
        bound: u64,
        items: u32,
        reused: bool,
    ) {
        let wire = list_wire(items, 1);
        call(meter, peak, bound, reused, || patcher.start(plan, DeclIx::new(0), wire)).expect("list record");
        assert_eq!(call(meter, peak, bound, reused, || patcher.next()), Ok(Step::Done));
        call(meter, peak, bound, reused, || {
            let bytes = patcher.finish().expect("finish");
            assert_eq!(
                bytes.len(),
                usize::try_from(items.checked_add(LIST_BYTES).expect("list length fits u32")).expect("length")
            );
            assert_eq!(bytes.get(..2), Some([0, 2].as_slice()));
            assert_eq!(bytes.get(2..6), Some(items.to_be_bytes().as_slice()));
            assert!(bytes.get(6..).expect("items").iter().all(|&byte| byte == 7));
        });
    }

    fn ceiling_story(capacity: Capacity, items: u32, refusal: skein_codec::patch::Halt) {
        let bound = Patcher::worst_case_heap(capacity).expect("bound");
        let meter = Meter::new();
        let (mut patcher, plan, mut peak) = setup(&meter, capacity, bound, LIST_OLD, LIST_NEW, LIST_PATCH);
        for &length in RISING {
            list_record(&mut patcher, plan, &meter, &mut peak, bound, length, false);
        }
        list_record(&mut patcher, plan, &meter, &mut peak, bound, items, false);
        let wire = list_wire(items.checked_add(1).expect("one item past the bound fits u32"), 1);
        assert_eq!(call(&meter, &mut peak, bound, false, || patcher.start(plan, DeclIx::new(0), wire)), Err(refusal));
        list_record(&mut patcher, plan, &meter, &mut peak, bound, items, true);
        drop(patcher);
        assert_eq!(meter.held(), 0, "all machine storage released");
    }

    #[test]
    fn node_growth_reaches_the_ceiling_refuses_and_reuses() {
        use skein_codec::{patch::Halt, value::Refusal};
        const {
            assert!(LIST_BOUND >= CEILING - 1, "schema admits one node past");
        }
        ceiling_story(NODE_CAPACITY, CEILING - LIST_NODES, Halt::Decode(Refusal::Room));
    }

    #[test]
    fn byte_growth_reaches_the_ceiling_refuses_and_reuses() {
        use skein_codec::patch::Halt;
        const {
            assert!(LIST_BOUND >= CEILING - 5, "schema admits one byte past");
        }
        ceiling_story(BYTE_CAPACITY, CEILING - LIST_BYTES, Halt::Room);
    }
}
