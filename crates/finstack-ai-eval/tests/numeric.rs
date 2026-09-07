//! Hand-calculated decimal, unit and tolerance cases, also in the minimal build.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use finstack_ai_eval::{numeric::*, *};

#[test]
fn currency_scale_percent_and_fraction_equivalence() {
    for (left, right) in [
        ("$1.2m", "USD 1,200,000"),
        ("2.5 billion GBP", "GBP 2500mm"),
        ("119.7 bps", "1.197%"),
        ("1.197%", "0.01197"),
        ("($1,200.00)", "USD -1.2k"),
        ("1e-3", ".001"),
        ("-0", "0%"),
    ] {
        assert!(
            ParsedNumber::parse(left)
                .unwrap()
                .equivalent(&ParsedNumber::parse(right).unwrap())
                .unwrap(),
            "{left} vs {right}"
        );
    }
    assert!(
        !ParsedNumber::parse("USD 100")
            .unwrap()
            .equivalent(&ParsedNumber::parse("EUR 100").unwrap())
            .unwrap()
    );
    assert!(
        !ParsedNumber::parse("$100")
            .unwrap()
            .equivalent(&ParsedNumber::parse("100").unwrap())
            .unwrap()
    );
}

#[test]
fn relative_partial_absolute_and_zero_target_cases() {
    let mut bands = ToleranceBands {
        full_within_ppm: 1000,
        partial_within_ppm: Some(5000),
        partial_micros: ScoreMicros::try_new(500_000).unwrap(),
        absolute: None,
    };
    let target = ParsedNumber::parse("100").unwrap();
    for (answer, expected) in [
        ("100.1", 1_000_000),
        ("100.10001", 500_000),
        ("99.5", 500_000),
        ("99.49999", 0),
    ] {
        assert_eq!(
            bands
                .grade(&ParsedNumber::parse(answer).unwrap(), &target)
                .unwrap()
                .get(),
            expected
        );
    }
    let zero = ParsedNumber::parse("0").unwrap();
    assert_eq!(
        bands
            .grade(&ParsedNumber::parse(".00001").unwrap(), &zero)
            .unwrap(),
        ScoreMicros::ZERO
    );
    assert_eq!(bands.grade(&zero, &zero).unwrap(), ScoreMicros::ONE);
    bands.absolute = Some(ParsedNumber::parse(".01").unwrap());
    assert_eq!(
        bands
            .grade(&ParsedNumber::parse("-.01").unwrap(), &zero)
            .unwrap(),
        ScoreMicros::ONE
    );
    bands.absolute = Some(ParsedNumber::parse("USD .01").unwrap());
    assert!(bands.grade(&zero, &zero).is_err());
}

#[test]
fn invalid_decimal_shapes_and_wire_bounds_fail_closed() {
    for text in [
        "",
        "NaN",
        "inf",
        "1,23",
        "1234,567",
        "1,234.5,6",
        "1e1000000",
        "USD 1%",
        "1..2",
        "--1",
        "(-1)",
        "profit 123",
        "1.2.3",
        "1e",
        "12 dollars",
        "1_000",
    ] {
        assert!(ParsedNumber::parse(text).is_err(), "accepted {text}");
    }
    let exact = ParsedNumber::parse("1.0000000000000001").unwrap();
    assert!(
        !exact
            .equivalent(&ParsedNumber::parse("1").unwrap())
            .unwrap()
    );
    let wire = serde_json::to_value(&exact).unwrap();
    assert_eq!(wire["mantissa"], "10000000000000001");
    assert_eq!(serde_json::from_value::<ParsedNumber>(wire).unwrap(), exact);
    assert!(
        serde_json::from_str::<ParsedNumber>(
            r#"{"mantissa":"1","exponent":100,"unit":{"kind":"scalar"}}"#
        )
        .is_err()
    );
    assert_eq!(
        ToleranceBands::relative(1)
            .grade(
                &ParsedNumber::parse("1e24").unwrap(),
                &ParsedNumber::parse("1e-24").unwrap()
            )
            .unwrap_err()
            .code(),
        EVAL_ARITHMETIC_OVERFLOW
    );
}
