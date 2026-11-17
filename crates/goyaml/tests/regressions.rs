//! Regression tests for divergences from Go that the differential review found. The expected
//! values are what Go 1.26.2 with yaml.v3 v3.0.1 and mapstructure v1.5.0 printed for the same
//! inputs (`time.Local` set to `time.UTC`, as in `testdata/go`); the goldens replay them too.

#[cfg(test)]
mod regressions {
    use std::{collections::BTreeMap, fmt::Debug};

    use pbs_gocompat::time::{DateFields, Location, Time};
    use pbs_gojson::jsonutil;
    use pbs_goyaml::{
        GoMapstructure, Type, Value,
        mapstructure::{self, DecodeError},
        yaml,
    };

    /// Stands for a Go struct with one field, `A int` tagged `mapstructure:"a"`.
    #[derive(Debug, Default, PartialEq, Eq, GoMapstructure)]
    #[goyaml(go_type = "main.nested")]
    struct Nested {
        #[goyaml(go = "A", mapstructure = "a")]
        a: isize,
    }

    fn ensure_eq<T: PartialEq + Debug>(got: &T, want: &T, what: &str) -> Result<(), String> {
        if got == want {
            Ok(())
        } else {
            Err(format!("{what}:\n  got:  {got:?}\n  want: {want:?}"))
        }
    }

    fn date(year: i64, month: i64, day: i64) -> Value {
        Value::Time(Time::date(
            &DateFields {
                year,
                month,
                day,
                ..DateFields::default()
            },
            Location::Utc,
        ))
    }

    fn ints() -> Value {
        Value::Slice(&Type::Int, Some(vec![Value::Int(1), Value::Int(2)]))
    }

    /// The error text of decoding `input` into a new `T`.
    fn yaml_error<T: yaml::YamlDecode + Default>(input: &str) -> Result<String, String> {
        match yaml::unmarshal(input.as_bytes(), &mut T::default()) {
            Ok(()) => Err(format!("{input:?}: no error")),
            Err(e) => Ok(String::from_utf8_lossy(&e.text()).into_owned()),
        }
    }

    fn yaml_value(input: &str) -> Result<Value, String> {
        let mut v = Value::Nil;
        yaml::unmarshal(input.as_bytes(), &mut v).map_err(|e| e.to_string())?;
        Ok(v)
    }

    /// Go: a `time.Time` source decodes into a struct through `decodeMapFromStruct`: an empty
    /// map, since its fields are unexported, so the struct is left as it was.
    #[test]
    fn time_into_struct() -> Result<(), String> {
        let mut out = Nested { a: 7 };
        mapstructure::decode(&date(2001, 12, 14), &mut out).map_err(|e| e.to_string())?;
        ensure_eq(&out, &Nested { a: 7 }, "struct")?;
        let mut ptr: Option<Box<Nested>> = None;
        mapstructure::decode(&date(2001, 12, 14), &mut ptr).map_err(|e| e.to_string())?;
        ensure_eq(&ptr, &Some(Box::default()), "nil *struct")
    }

    /// Go: a `time.Time` source decodes into a map as an empty map: a nil map is allocated,
    /// and a non-nil one is set to itself.
    #[test]
    fn time_into_map() -> Result<(), String> {
        let mut nil: Option<BTreeMap<String, String>> = None;
        mapstructure::decode(&date(2001, 12, 14), &mut nil).map_err(|e| e.to_string())?;
        ensure_eq(&nil, &Some(BTreeMap::new()), "nil map")?;
        let full = Some(BTreeMap::from([("a".to_owned(), "b".to_owned())]));
        let mut kept = full.clone();
        mapstructure::decode(&date(2001, 12, 14), &mut kept).map_err(|e| e.to_string())?;
        ensure_eq(&kept, &full, "non-nil map")
    }

    /// Go: `decodeMapFromMap` sets a non-nil target to a nil source map, which `reflect.Set`
    /// refuses (a panic) when the map types differ.
    #[test]
    fn nil_map_of_another_type() -> Result<(), String> {
        let full = Some(BTreeMap::from([("a".to_owned(), "b".to_owned())]));
        let mut other = full.clone();
        let err = mapstructure::decode(&Value::StringMap(&Type::Interface, None), &mut other);
        ensure_eq(
            &err,
            &Err(DecodeError::Panic(
                "reflect.Set: value of type map[string]interface {} is not assignable to type \
                 map[string]string"
                    .to_owned(),
            )),
            "map[string]interface {}(nil) into map[string]string",
        )?;
        let mut same = full;
        mapstructure::decode(&Value::StringMap(&Type::String, None), &mut same)
            .map_err(|e| e.to_string())?;
        ensure_eq(
            &same,
            &None,
            "map[string]string(nil) into map[string]string",
        )
    }

    /// Go: `decodeBasic` decodes into a copy of a non-nil interface's value and stores it back
    /// only on success.
    #[test]
    fn failed_decode_keeps_interface_value() -> Result<(), String> {
        let mut string = Value::String("s".to_owned());
        let string_err = mapstructure::decode(&ints(), &mut string).map(|()| String::new());
        ensure_eq(
            &string_err.unwrap_or_else(|e| e.to_string()),
            &"'' expected type 'string', got unconvertible type '[]int', value: '[1 2]'".to_owned(),
            "string error",
        )?;
        ensure_eq(&string, &Value::String("s".to_owned()), "string kept")?;
        let mut time = date(2001, 12, 14);
        let time_err = mapstructure::decode(&ints(), &mut time).map(|()| String::new());
        ensure_eq(
            &time_err.unwrap_or_else(|e| e.to_string()),
            &"'' expected a map, got 'slice'".to_owned(),
            "time error",
        )?;
        ensure_eq(&time, &date(2001, 12, 14), "time kept")
    }

    /// Go: `d.terror` writes a `!!seq` or `!!map` tag's value right after it, unquoted.
    #[test]
    fn collection_tag_on_scalar() -> Result<(), String> {
        ensure_eq(
            &yaml_error::<isize>("!!map 1h30m\n")?,
            &"yaml: unmarshal errors:\n  line 1: cannot unmarshal !!map1h30m into int".to_owned(),
            "!!map",
        )?;
        ensure_eq(
            &yaml_error::<Option<Vec<String>>>("!!seq -3.5\n")?,
            &"yaml: unmarshal errors:\n  line 1: cannot unmarshal !!seq-3.5 into []string"
                .to_owned(),
            "!!seq",
        )
    }

    /// Go: `%#v` prints a `time.Time` with its `GoString` method, and sorts `time.Time` keys by
    /// their `wall` and `ext` fields, then by location pointer (nil, UTC, first).
    #[test]
    fn invalid_map_key_with_times() -> Result<(), String> {
        let cases = [
            (
                "? {2001-12-14: a}\n: b\n",
                "yaml: invalid map key: map[interface {}]interface {}{time.Date(2001, \
                 time.December, 14, 0, 0, 0, 0, time.UTC):\"a\"}",
            ),
            (
                "? [{a: [9999-12-31T23:59:59.999999999-23:59]}]\n: x\n",
                "yaml: invalid map key: []interface {}{map[string]interface {}{\"a\":[]interface \
                 {}{time.Date(9999, time.December, 31, 23, 59, 59, 999999999, \
                 time.Location(\"\"))}}}",
            ),
            (
                "? {\"x\\u00e9\\u0001\\\"\\0\": 2001-12-14T00:00:00+05:30, \"time.Time{\": \
                 [2001-12-14]}\n: x\n",
                "yaml: invalid map key: map[string]interface {}{\"time.Time{\":[]interface \
                 {}{time.Date(2001, time.December, 14, 0, 0, 0, 0, time.UTC)}, \
                 \"x\u{e9}\\x01\\\"\\x00\":time.Date(2001, time.December, 14, 0, 0, 0, 0, \
                 time.Location(\"\"))}",
            ),
            (
                "? {2001-12-14: a, 2001-12-13T00:00:00.5Z: b, 2001-12-14T05:30:00+05:30: c, \
                 2001-12-14T00:00:00Z: d}\n: x\n",
                "yaml: invalid map key: map[interface {}]interface {}{time.Date(2001, \
                 time.December, 14, 0, 0, 0, 0, time.UTC):\"d\", time.Date(2001, time.December, \
                 14, 5, 30, 0, 0, time.Location(\"\")):\"c\", time.Date(2001, time.December, 13, \
                 0, 0, 0, 500000000, time.UTC):\"b\"}",
            ),
        ];
        for (input, want) in cases {
            ensure_eq(&yaml_error::<Value>(input)?, &want.to_owned(), input)?;
        }
        Ok(())
    }

    /// Go: timestamp keys are `==` only with the same `*time.Location`: each `time.Parse` of an
    /// offset allocates a zone, but for the unnamed whole hours from -12h to +14h, which
    /// `time.FixedZone` caches.
    #[test]
    fn timestamp_keys_compare_location_pointers() -> Result<(), String> {
        let cases = [
            ("+05:00", 1),
            ("+05:30", 2),
            ("+14:00", 1),
            ("+15:00", 2),
            ("-12:00", 1),
            ("-12:30", 2),
            ("Z", 1),
        ];
        for (zone, want) in cases {
            let input = format!("{{2001-12-14T00:00:00{zone}: a, 2001-12-14t00:00:00{zone}: b}}\n");
            ensure_eq(&map_len(&yaml_value(&input)?), &Some(want), &input)?;
        }
        let merged = yaml_value(
            "b: &b {2001-12-14T00:00:00+05:30: a}\nm: {<<: *b, 2001-12-14T00:00:00+05:30: c}\n",
        )?;
        let entries = merged.as_string_map().and_then(|m| map_len(m.get("m")?));
        ensure_eq(&entries, &Some(2), "merged +05:30 key")
    }

    /// The number of entries of a non-nil `map[interface {}]interface {}`.
    fn map_len(v: &Value) -> Option<usize> {
        if let Value::Map(_, _, Some(entries)) = v {
            Some(entries.len())
        } else {
            None
        }
    }

    /// Go: jsoniter panics on a nil map key, which ends the encoding whatever it failed on
    /// before (an unsupported NaN only records an error and goes on).
    #[test]
    fn nil_key_panic_wins() -> Result<(), String> {
        const PANIC: &str = "runtime error: invalid memory address or nil pointer dereference";
        for input in [
            "[.nan, {~: 1}]\n",
            "[{~: 1}, .nan]\n",
            "{a: .nan, b: [{~: 1}]}\n",
        ] {
            ensure_eq(&json(&yaml_value(input)?), &PANIC.to_owned(), input)?;
        }
        ensure_eq(
            &json(&yaml_value("[[.nan], {a: 1}]\n")?),
            &"[]interface {}: []interface {}: unsupported value: NaN".to_owned(),
            "NaN without a nil key",
        )
    }

    /// `jsonutil.Marshal(v)`: the JSON, or the error text.
    fn json(v: &Value) -> String {
        jsonutil::marshal(v).map_or_else(
            |e| e.message.to_string_lossy(),
            |json| String::from_utf8_lossy(&json).into_owned(),
        )
    }
}
