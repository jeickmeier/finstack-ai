use super::{EventPublishError, validate_event_sequences};

struct SequenceCase {
    name: &'static str,
    sequences: &'static [u64],
    start: Option<u64>,
    expected: Result<Option<u64>, &'static str>,
}

#[test]
fn source_sequence_table_matches_native_codes() {
    let cases = [
        SequenceCase {
            name: "empty keeps cursor",
            sequences: &[],
            start: Some(4),
            expected: Ok(Some(4)),
        },
        SequenceCase {
            name: "empty starts unset",
            sequences: &[],
            start: None,
            expected: Ok(None),
        },
        SequenceCase {
            name: "first batch sets cursor",
            sequences: &[3, 4],
            start: None,
            expected: Ok(Some(5)),
        },
        SequenceCase {
            name: "continues from cursor",
            sequences: &[5],
            start: Some(5),
            expected: Ok(Some(6)),
        },
        SequenceCase {
            name: "gap is mismatch",
            sequences: &[2],
            start: Some(1),
            expected: Err("event_sequence_mismatch"),
        },
        SequenceCase {
            name: "internal gap is mismatch",
            sequences: &[1, 3],
            start: None,
            expected: Err("event_sequence_mismatch"),
        },
        SequenceCase {
            name: "u64 overflow is exhausted",
            sequences: &[u64::MAX],
            start: Some(u64::MAX),
            expected: Err("event_sequence_exhausted"),
        },
    ];
    for case in cases {
        let mut next = case.start;
        let result = validate_event_sequences(case.sequences.iter().copied(), &mut next);
        match case.expected {
            Ok(cursor) => {
                assert_eq!(result, Ok(()), "{}", case.name);
                assert_eq!(next, cursor, "{}", case.name);
            }
            Err(code) => {
                assert_eq!(result, Err(EventPublishError { code }), "{}", case.name);
            }
        }
    }
}
