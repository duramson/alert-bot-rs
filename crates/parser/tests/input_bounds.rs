use chrono::{Duration, TimeZone, Utc};
use parser::{parse, Language, ParseContext};

fn context() -> ParseContext {
    ParseContext {
        now_utc: Utc.with_ymd_and_hms(2026, 5, 8, 10, 0, 0).unwrap(),
        tz: chrono_tz::Europe::Berlin,
        language: Language::De,
    }
}

#[test]
fn large_user_supplied_offsets_are_errors_not_panics() {
    let ctx = context();
    for input in [
        "4294967295Y text", "4294967295M text", "4294967295w text",
        "4294967295d text", "4294967295h text", "4294967295m text",
        "4294967295s text", "1Y4294967295w text", "1M4294967295d text",
        "4294967295d 11:00 text", "4294967295w 11:00 text",
        "*4294967295w1s text", "*4294967295d1s text", "*4294967295h text",
    ] {
        assert!(parse(input, &ctx).is_err(), "must reject {input}");
    }
}

#[test]
fn supported_boundaries_still_parse() {
    let ctx = context();
    assert!(parse("50Y text", &ctx).is_ok());
    assert!(parse("600M text", &ctx).is_ok());
    assert_eq!(parse("5m text", &ctx).unwrap().fire_at(), ctx.now_utc + Duration::minutes(5));
    assert_eq!(parse("*30m text", &ctx).unwrap().fire_at(), ctx.now_utc + Duration::minutes(30));
}
