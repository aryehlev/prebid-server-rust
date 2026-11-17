//! Ports of buger/jsonparser's own tests (`parser_test.go`, `parser_error_test.go`,
//! `escape_test.go`, `bytes_test.go`) that need no Go-side table; `tests/toolkit.rs` replays
//! every table case of `parser_test.go` against Go's output.

use core::fmt::Write as _;

use super::{
    NO_KEYS, ParserError, ValueType, array_each, delete, each_key, get, get_boolean, get_int,
    get_string, object_each, parse_int, parse_string, set, unescape,
};
use crate::GoString;

/// A `Set` case: data, value, path and the result.
type SetCase<'a> = (
    &'a [u8],
    &'a [u8],
    &'a [&'a str],
    Result<&'a [u8], ParserError>,
);

macro_rules! check {
    ($got:expr, $want:expr, $what:expr $(,)?) => {{
        let got = $got;
        let want = $want;
        if got == want {
            Ok(())
        } else {
            Err(format!("{}: got {got:?}, want {want:?}", $what))
        }
    }};
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn array_each_values() -> Result<(), String> {
    // Go: parser_test.go TestArrayEach
    let mock = br#"{"a": { "b":[{"x": 1} ,{"x":2},{ "x":3}, {"x":4} ]}}"#;
    let mut items = Vec::new();
    let (_, err) = array_each(mock, &["a", "b"], |value, _, _| items.push(text(value)));
    check!(err, None, "err")?;
    check!(
        items,
        vec![r#"{"x": 1}"#, r#"{"x":2}"#, r#"{ "x":3}"#, r#"{"x":4}"#],
        "items"
    )
}

#[test]
fn array_each_white_space() -> Result<(), String> {
    // Go: parser_test.go TestArrayEachWithWhiteSpace
    let mut items = Vec::new();
    let (_, err) = array_each(br#"    ["AAA", "BBB", "CCC"]"#, NO_KEYS, |value, _, _| {
        items.push(text(value));
    });
    check!(err, None, "array with white space")?;
    check!(items, vec!["AAA", "BBB", "CCC"], "items")?;
    for data in [&b"    1"[..], b"    "] {
        let mut called = false;
        let (_, each_err) = array_each(data, NO_KEYS, |_, _, _| called = true);
        check!((each_err.is_some(), called), (true, false), text(data))?;
    }
    Ok(())
}

#[test]
fn array_each_empty() -> Result<(), String> {
    // Go: parser_test.go TestArrayEachEmpty
    let cases: [(&[u8], &[&str], isize); 6] = [
        (b"[]", &[], 1),
        (b"[ ]", &[], 2),
        (b"[\n]", &[], 2),
        (br#"{"data": []}"#, &["data"], 10),
        (br#"{"data": [ ]}"#, &["data"], 11),
        (b"{\"data\": [\n]}", &["data"], 11),
    ];
    for (data, keys, want) in cases {
        let mut called = false;
        let got = array_each(data, keys, |_, _, _| called = true);
        check!((got, called), ((want, None), false), text(data))?;
    }
    Ok(())
}

const EACH_KEY_JSON: &str = "{
\t\"name\": \"Name\",
\t\"order\": \"Order\",
\t\"sum\": 100,
\t\"len\": 12,
\t\"isPaid\": true,
\t\"nested\": {\"a\":\"test\", \"b\":2, \"nested3\":{\"a\":\"test3\",\"b\":4}, \"c\": \"unknown\"},
\t\"nested2\": {
\t\t\"a\":\"test2\",
\t\t\"b\":3
\t},
\t\"arr\": [
\t\t{
\t\t\t\"a\":\"zxc\",
\t\t\t\"b\": 1
\t\t},
\t\t{
\t\t\t\"a\":\"123\",
\t\t\t\"b\":2
\t\t}
\t],
\t\"arrInt\": [1,2,3,4],
\t\"intPtr\": 10,
\t\"a\\n\":{
\t\t\"b\\n\":99
\t}
}";

#[test]
fn each_key_paths() -> Result<(), String> {
    // Go: parser_test.go TestEachKey
    let paths: [&[&str]; 13] = [
        &["name"],
        &["order"],
        &["nested", "a"],
        &["nested", "b"],
        &["nested2", "a"],
        &["nested", "nested3", "b"],
        &["arr", "[1]", "b"],
        &["arrInt", "[3]"],
        &["arrInt", "[5]"],
        &["nested"],
        &["arr", "["],
        &["a\n", "b\n"],
        &["nested", "b"],
    ];
    let mut found = Vec::new();
    let _end = each_key(EACH_KEY_JSON.as_bytes(), &paths, |idx, value, _, _| {
        found.push((idx, text(value)));
    });
    found.sort();
    let want: Vec<(isize, String)> = [
        (0, "Name"),
        (1, "Order"),
        (2, "test"),
        (3, "2"),
        (4, "test2"),
        (5, "4"),
        (6, "2"),
        (7, "4"),
        (
            9,
            r#"{"a":"test", "b":2, "nested3":{"a":"test3","b":4}, "c": "unknown"}"#,
        ),
        (11, "99"),
        (12, "2"),
    ]
    .into_iter()
    .map(|(i, v)| (i, v.to_owned()))
    .collect();
    check!(found, want, "keys found")
}

#[test]
fn panicking_errors() -> Result<(), String> {
    // Go: parser_error_test.go TestPanickingErrors
    let paths: [&[&str]; 4] = [&["test"], &["these"], &["keys"], &["please"]];
    let iter_err = |data: &[u8]| {
        let mut err = None;
        let _end = each_key(data, &paths, |_, _, _, e| {
            if e.is_some() {
                err = e;
            }
        });
        err
    };
    check!(
        iter_err(br#"{"test":"#).is_some(),
        true,
        "EachKey truncated"
    )?;
    check!(
        iter_err(br#"{"test":0}some":[{"these":[{"keys":"some"}]}]}some"}]}],"please":"some"}"#)
            .is_some(),
        true,
        "EachKey malformed",
    )?;
    check!(
        get(br#"{"test":"#, &["test"]).err.is_some(),
        true,
        "Get truncated"
    )?;
    check!(
        get(
            br#"{"some":0}some":[{"some":[{"some":"some"}]}]}some"}]}],"some":"some"}"#,
            &["x"]
        )
        .err
        .is_some(),
        true,
        "Get malformed",
    )
}

#[test]
fn key_depth() -> Result<(), String> {
    // Go: parser_error_test.go TestKeyDepth (and the 129-key variant)
    for n in [128_usize, 129] {
        let mut data = String::from("{");
        let mut keys = Vec::new();
        for i in 0..n {
            write!(data, r#""key{i}": {i}x,"#).map_err(|e| e.to_string())?;
            keys.push(format!("key{i}"));
        }
        data.push('}');
        let mut calls = 0_usize;
        let _end = each_key(data.as_bytes(), &[keys.as_slice()], |_, _, _, _| calls += 1);
        check!(calls, 0, "deep path")?;
    }
    Ok(())
}

#[test]
fn unescape_cases() -> Result<(), String> {
    // Go: escape_test.go unescapeTests, plus the surrogate cases of multiUnicodeEscapeTests
    // (`~` stands for a backslash).
    let cases: [(&str, Option<&str>); 36] = [
        ("", Some("")),
        ("a", Some("a")),
        ("abcde", Some("abcde")),
        ("ab~~de", Some("ab\\de")),
        ("ab~\"de", Some("ab\"de")),
        ("ab ~u00B0 de", Some("ab \u{b0} de")),
        ("ab ~uFF11 de", Some("ab \u{ff11} de")),
        ("~uFFFF", Some("\u{ffff}")),
        ("ab ~uD83D~uDE03 de", Some("ab \u{1f603} de")),
        ("~u0000~u0000~u0000~u0000~u0000", Some("\0\0\0\0\0")),
        ("~u0000 ~u0000 ~u0000 ~u0000 ~u0000", Some("\0 \0 \0 \0 \0")),
        (
            " ~u0000 ~u0000 ~u0000 ~u0000 ~u0000 ",
            Some(" \0 \0 \0 \0 \0 "),
        ),
        ("~uD800", None),
        ("abcde~", None),
        ("abcde~x", None),
        ("abcde~u", None),
        ("abcde~u1", None),
        ("abcde~u12", None),
        ("abcde~u123", None),
        ("abcde~uD800", None),
        ("ab~uD800de", None),
        ("~uD800abcde", None),
        ("~uD800~u", None),
        ("~uD800~uDC", None),
        ("~uD800~uDBFF", None),
        ("~uD800~uDC00", Some("\u{10000}")),
        // combineUTF16Surrogates is plain arithmetic on any second unit of at least DC00 (not
        // utf16.DecodeRune): a result past U+10FFFF is written as U+FFFD by utf8.EncodeRune,
        // anything else is kept, even when neither unit is the right surrogate.
        ("~uDC00~uDC00", Some("\u{fffd}")),
        ("~uDBFF~uFFFF", Some("\u{fffd}")),
        ("~uD800~uE000", Some("\u{10400}")),
        ("~uD800~uFFFD", Some("\u{123fd}")),
        ("~uDBFF~uDFFF", Some("\u{10ffff}")),
        ("~uDBFF~uE000", Some("\u{fffd}")),
        // The second escape's `\u` is not checked.
        ("~uD83DxxDE03", Some("\u{1f603}")),
        ("~u123X", None),
        ("~/~b", Some("/\u{8}")),
        ("~f~n~r~t", Some("\u{c}\n\r\t")),
    ];
    for (input, want) in cases {
        let input = input.replace('~', "\x5c");
        let got = unescape(input.as_bytes()).ok().map(|s| text(&s));
        check!(got, want.map(str::to_owned), &input)?;
    }
    Ok(())
}

#[test]
fn parse_int_cases() -> Result<(), String> {
    // Go: bytes_test.go TestBytesParseInt
    let cases: [(&str, Result<i64, ParserError>); 11] = [
        ("0", Ok(0)),
        ("1", Ok(1)),
        ("-1", Ok(-1)),
        ("12345", Ok(12345)),
        ("-12345", Ok(-12345)),
        ("9223372036854775807", Ok(i64::MAX)),
        ("-9223372036854775808", Ok(i64::MIN)),
        ("9223372036854775808", Err(ParserError::OverflowInteger)),
        ("-9223372036854775809", Err(ParserError::OverflowInteger)),
        ("18446744073709551616", Err(ParserError::OverflowInteger)),
        ("1.0", Err(ParserError::MalformedValue)),
    ];
    for (input, want) in cases {
        check!(parse_int(input.as_bytes()), want, input)?;
    }
    check!(parse_int(b"-"), Ok(0), "lone minus")?;
    check!(parse_int(b""), Err(ParserError::MalformedValue), "empty")
}

#[test]
fn typed_getters() -> Result<(), String> {
    // Go: parser_test.go getIntTests, getStringTests, getBoolTests (selected)
    let data = r#"{"i": 1, "s": "a\nb", "u": "~u00e9", "b": true, "n": null, "f": 1.5}"#
        .replace('~', "\x5c");
    let data = data.as_bytes();
    check!(get_int(data, &["i"]), Ok(1), "int")?;
    check!(
        get_int(data, &["f"]),
        Err(ParserError::MalformedValue),
        "float as int"
    )?;
    check!(
        get_int(data, &["n"]),
        Err(ParserError::NullValue),
        "null int"
    )?;
    check!(
        get_int(data, &["s"]).map_err(|e| text(&e.go_text())),
        Err(r"Value is not a number: a\nb".to_owned()),
        "string as int",
    )?;
    check!(
        get_string(data, &["s"]).map(GoString::into_bytes),
        Ok(b"a\nb".to_vec()),
        "string"
    )?;
    check!(
        get_string(data, &["u"]).map(GoString::into_bytes),
        Ok("\u{e9}".as_bytes().to_vec()),
        "unicode escape",
    )?;
    check!(get_boolean(data, &["b"]), Ok(true), "bool")?;
    check!(
        get_boolean(data, &["i"]).map_err(|e| e.to_string()),
        Err("Value is not a boolean: 1".to_owned()),
        "int as bool",
    )?;
    check!(
        parse_string(br"\x").map_err(|e| e.to_string()),
        Err(
            "Value looks like Number/Boolean/None, but can't find its end: ',' or '}' symbol"
                .to_owned()
        ),
        "bad escape",
    )
}

#[test]
fn get_offsets_and_types() -> Result<(), String> {
    let data = br#"{"a": [1, "x", {"b": null}], "c": tru}"#;
    let string_elem = get(data, &["a", "[1]"]);
    check!(
        (
            text(string_elem.value),
            string_elem.data_type,
            string_elem.offset,
            string_elem.err
        ),
        ("x".to_owned(), ValueType::String, 13, None),
        "string element",
    )?;
    let nested = get(data, &["a", "[2]", "b"]);
    check!(
        (text(nested.value), nested.data_type, nested.offset),
        ("null".to_owned(), ValueType::Null, 25),
        "nested null",
    )?;
    let unknown = get(data, &["c"]);
    check!(
        (unknown.data_type, unknown.offset, unknown.err),
        (ValueType::Unknown, 34, Some(ParserError::UnknownValueType)),
        "unknown literal",
    )?;
    let missing = get(data, &["a", "[3]"]);
    check!(
        (missing.data_type, missing.offset, missing.err),
        (ValueType::NotExist, -1, Some(ParserError::KeyPathNotFound)),
        "missing index",
    )
}

#[test]
fn object_each_entries() -> Result<(), String> {
    // Go: parser_test.go objectEachTests (selected)
    let mut entries = Vec::new();
    let res: Result<(), ParserError> = object_each(
        br#"{"a\nb": "x", "n": 1 , "o": {"z": []}}"#,
        NO_KEYS,
        |k, v, t, _| {
            entries.push((text(k), text(v), t));
            Ok(())
        },
    );
    check!(res, Ok(()), "result")?;
    check!(
        entries,
        vec![
            ("a\nb".to_owned(), "x".to_owned(), ValueType::String),
            ("n".to_owned(), "1".to_owned(), ValueType::Number),
            ("o".to_owned(), r#"{"z": []}"#.to_owned(), ValueType::Object),
        ],
        "entries",
    )?;
    let unterminated: Result<(), ParserError> =
        object_each(br#"{"a": 1"#, NO_KEYS, |_, _, _, _| Ok(()));
    check!(
        unterminated,
        Err(ParserError::MalformedArray),
        "unterminated"
    )?;
    // The callback's own error is returned as it is.
    let stopped: Result<(), ParserError> =
        object_each(br#"{"a": 1, "b": 2}"#, NO_KEYS, |k, _, _, _| {
            if k == b"b" {
                Err(ParserError::NotAString(b"stop".to_vec()))
            } else {
                Ok(())
            }
        });
    check!(
        stopped,
        Err(ParserError::NotAString(b"stop".to_vec())),
        "callback error"
    )
}

#[test]
fn set_and_delete() -> Result<(), String> {
    // Go: parser_test.go setTests and deleteTests (selected)
    let cases: [SetCase<'_>; 6] = [
        (br#"{"a":1}"#, b"2", &["a"], Ok(br#"{"a":2}"#)),
        (br#"{"a":1}"#, b"2", &["b"], Ok(br#"{"a":1,"b":2}"#)),
        (b"{}", b"2", &["b", "c"], Ok(br#"{"b":{"c":2}}"#)),
        (
            br#"{"a":[{"b":1}]}"#,
            b"3",
            &["a", "[0]", "c"],
            Ok(br#"{"a":[{"b":1,"c":3}]}"#),
        ),
        (b"[1]", b"2", &["a"], Err(ParserError::KeyPathNotFound)),
        (br#"{"a":1}"#, b"2", &[], Err(ParserError::KeyPathNotFound)),
    ];
    for (data, value, keys, want) in cases {
        check!(set(data, value, keys), want.map(<[u8]>::to_vec), text(data))?;
    }
    let delete_cases: [(&[u8], &[&str], &[u8]); 5] = [
        (br#"{"test":"input"}"#, &["test"], b"{}"),
        (
            br#"{"a": 1, "b": 2, "c": 3}"#,
            &["c"],
            br#"{"a": 1, "b": 2}"#,
        ),
        (br#"{"a":[1,2,3]}"#, &["a", "[1]"], br#"{"a":[1,3]}"#),
        (br#"{"a":1}"#, &["b"], br#"{"a":1}"#),
        (br#"{"a":1}"#, &[], b""),
    ];
    for (data, keys, want) in delete_cases {
        check!(&*delete(data, keys), want, text(data))?;
    }
    Ok(())
}

#[test]
fn delete_escaped_key_shares_go_stack_buffer() -> Result<(), String> {
    // Go: findKeyStart unescapes the search key into a 64-byte stack buffer and then every
    // escaped key of the data into the same buffer, so an escaped search key is overwritten by
    // the data keys it is compared with. Delete takes the start of the member from findKeyStart
    // and its end from searchKeys, which compares the key as given. Outputs of Go 1.25,
    // buger/jsonparser v1.1.2.
    let cases: [(&[u8], &str, &[u8]); 4] = [
        // `a\"b` unescapes to `a"b`, which `x<TAB>y` overwrites: findKeyStart stops at the
        // first member, searchKeys at the second.
        (br#"{"x\ty":1,"a\\\"b":2}"#, "a\\\"b", b"{}"),
        (br#"{"\\":1,"x\ty":5,"a\\\"b":2}"#, "a\\\"b", br#"{"\\":1}"#),
        // An unescaped search key is not in the buffer.
        (br#"{"x\ty":1,"a\"b":2}"#, "a\"b", br#"{"x\ty":1}"#),
        (br#"{"x\ty":1,"a\"b":2}"#, "x\ty", br#"{"a\"b":2}"#),
    ];
    for (data, key, want) in cases {
        check!(&*delete(data, &[key]), want, text(data))?;
    }
    Ok(())
}
