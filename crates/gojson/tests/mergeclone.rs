//! `jsonutil.MergeClone` into structs: the Go package's own tests (`util/jsonutil/merge_test.go`)
//! on the real openrtb2 types, and the differential goldens that `testdata/go/mergeclone` writes
//! with Go 1.25 and json-iterator 1.1.12 (`MergeClone` of corpus openrtb2 `Imp`, `Site`, `App`,
//! `User`, `Device` and `BidRequest` values and of a struct with every field shape, then
//! `jsonutil.Marshal`), replayed byte for byte.
//!
//! Regenerate the goldens with `go run ./mergeclone` in `testdata/go`, a Go module that builds
//! against the upstream checkout.

#[cfg(test)]
mod mergeclone {
    use core::str::from_utf8;
    use std::{collections::BTreeMap, fmt::Write as _, fs};

    use pbs_gojson::{
        GoEncode, GoJson, JsoniterDecode, RawJson,
        jsonutil::{self, StringInt},
    };
    use pbs_openrtb::openrtb2;
    use serde_json::Value;

    // ---- merge_test.go -------------------------------------------------------------------

    fn check(got: &str, want: &str, what: &str) -> Result<(), String> {
        if got == want {
            Ok(())
        } else {
            Err(format!("{what}: got {got:?}, want {want:?}"))
        }
    }

    fn marshaled<T: GoEncode + ?Sized>(v: &T) -> Result<String, String> {
        let out = jsonutil::marshal(v).map_err(|e| e.to_string())?;
        String::from_utf8(out).map_err(|e| e.to_string())
    }

    /// Runs `jsonutil.MergeClone(v, data)`, checks the error text (empty for none) and, when
    /// `want` is given, the `jsonutil.Marshal` output, and returns the merged value.
    fn merged<T: JsoniterDecode + GoEncode>(
        name: &str,
        mut v: T,
        data: &[u8],
        want: Option<&str>,
        want_err: &str,
    ) -> Result<T, String> {
        let err = jsonutil::merge_clone(&mut v, data)
            .err()
            .map_or_else(String::new, |e| e.to_string());
        check(&err, want_err, name)?;
        if let Some(want) = want {
            check(&marshaled(&v)?, want, name)?;
        }
        Ok(v)
    }

    fn imp_with_banner(id: &str) -> openrtb2::Imp {
        openrtb2::Imp {
            banner: Some(Box::new(openrtb2::Banner {
                id: id.to_owned(),
                ..openrtb2::Banner::default()
            })),
            ..openrtb2::Imp::default()
        }
    }

    fn imp_with_id(id: &str) -> Box<openrtb2::Imp> {
        Box::new(openrtb2::Imp {
            id: id.to_owned(),
            ..openrtb2::Imp::default()
        })
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn merge_clone_ptr() -> Result<(), String> {
        // Go: util/jsonutil/merge_test.go TestMergeClonePtr (the reference checks have no Rust
        // counterpart: values are never shared)
        let _root = merged(
            "root",
            imp_with_banner("1"),
            br#"{"banner":{"id":"4"}}"#,
            Some(r#"{"id":"","banner":{"id":"4"}}"#),
            "",
        )?;

        let with_video = || openrtb2::Imp {
            video: Some(Box::new(openrtb2::Video {
                podid: "a".to_owned(),
                ..openrtb2::Video::default()
            })),
            ..imp_with_banner("1")
        };
        let _embedded_nil = merged(
            "embedded-nil",
            with_video(),
            br#"{"banner":null}"#,
            Some(r#"{"id":"","video":{"mimes":null,"podid":"a"}}"#),
            "",
        )?;
        let _embedded_struct = merged(
            "embedded-struct",
            with_video(),
            br#"{"banner":{"id":"2"}}"#,
            Some(r#"{"id":"","banner":{"id":"2"},"video":{"mimes":null,"podid":"a"}}"#),
            "",
        )?;
        let embedded_int = merged(
            "embedded-int",
            openrtb2::Imp {
                clickbrowser: Some(1),
                ..openrtb2::Imp::default()
            },
            br#"{"clickbrowser":2}"#,
            None,
            "",
        )?;
        check(
            &format!("{:?}", embedded_int.clickbrowser),
            "Some(2)",
            "embedded-int",
        )?;

        let _invalid_null = merged(
            "invalid-null",
            imp_with_banner("1"),
            br#"{"banner":nul}"#,
            None,
            "cannot unmarshal openrtb2.Imp.Banner: expect ull",
        )?;
        let _invalid_malformed = merged(
            "invalid-malformed",
            imp_with_banner("1"),
            br#"{"banner":malformed}"#,
            None,
            "cannot unmarshal openrtb2.Imp.Banner: expect { or n, but found m",
        )?;
        Ok(())
    }

    /// `struct { Imps []*openrtb2.Imp `json:"imps"` }`.
    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(
        go_type = "struct { Imps []*openrtb2.Imp \"json:\\\"imps\\\"\" }",
        anonymous
    )]
    struct ImpsTest {
        #[gojson(json = "imps")]
        imps: Option<Vec<Option<Box<openrtb2::Imp>>>>,
    }

    /// A `TestMergeCloneSlice` case: name, existing `IframeBuster`, data, expected
    /// `jsonutil.Marshal` output and expected error.
    type SliceCase<'a> = (&'a str, &'a [&'a str], &'a [u8], &'a str, &'a str);

    #[test]
    fn merge_clone_slice() -> Result<(), String> {
        // Go: util/jsonutil/merge_test.go TestMergeCloneSlice
        let cases: [SliceCase<'_>; 5] = [
            (
                "null",
                &["a", "b"],
                br#"{"iframeBuster":null}"#,
                r#"{"id":""}"#,
                "",
            ),
            (
                "one",
                &["a"],
                br#"{"iframeBuster":["b"]}"#,
                r#"{"id":"","iframebuster":["b"]}"#,
                "",
            ),
            (
                "many",
                &["a"],
                br#"{"iframeBuster":["b", "c"]}"#,
                r#"{"id":"","iframebuster":["b","c"]}"#,
                "",
            ),
            (
                "invalid-null",
                &["a"],
                br#"{"iframeBuster":nul}"#,
                r#"{"id":""}"#,
                "cannot unmarshal openrtb2.Imp.IframeBuster: expect ull",
            ),
            (
                "invalid-malformed",
                &["a"],
                br#"{"iframeBuster":malformed}"#,
                r#"{"id":""}"#,
                concat!(
                    "cannot unmarshal openrtb2.Imp.IframeBuster: ",
                    "decode slice: expect [ or n, but found m"
                ),
            ),
        ];
        for (name, existing, data, want, want_err) in cases {
            let imp = openrtb2::Imp {
                iframebuster: Some(strings(existing)),
                ..openrtb2::Imp::default()
            };
            let _merged = merged(name, imp, data, Some(want), want_err)?;
        }

        // replace-not-overlay: the slice is replaced, so the original id is gone.
        let _replaced = merged(
            "replace-not-overlay",
            ImpsTest {
                imps: Some(vec![Some(imp_with_id("1"))]),
            },
            br#"{"imps":[{"tagid":"2"}]}"#,
            Some(r#"{"imps":[{"id":"","tagid":"2"}]}"#),
            "",
        )?;
        Ok(())
    }

    /// `struct { Foo map[string]int `json:"foo"` }`.
    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(
        go_type = "struct { Foo map[string]int \"json:\\\"foo\\\"\" }",
        anonymous
    )]
    struct StringMapTest {
        #[gojson(json = "foo")]
        foo: Option<BTreeMap<String, isize>>,
    }

    /// `struct { Foo map[int]string `json:"foo"` }`.
    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(
        go_type = "struct { Foo map[int]string \"json:\\\"foo\\\"\" }",
        anonymous
    )]
    struct IntMapTest {
        #[gojson(json = "foo")]
        foo: Option<BTreeMap<isize, String>>,
    }

    #[test]
    fn merge_clone_map() -> Result<(), String> {
        // Go: util/jsonutil/merge_test.go TestMergeCloneMap
        let ab = || StringMapTest {
            foo: Some(BTreeMap::from([("a".to_owned(), 1), ("b".to_owned(), 2)])),
        };
        let one_two = || IntMapTest {
            foo: Some(BTreeMap::from([(1, "a".to_owned()), (2, "b".to_owned())])),
        };
        let _null = merged(
            "null",
            ab(),
            br#"{"foo":null}"#,
            Some(r#"{"foo":null}"#),
            "",
        )?;
        let _key_string = merged(
            "key-string",
            ab(),
            br#"{"foo":{"c":3}}"#,
            Some(r#"{"foo":{"a":1,"b":2,"c":3}}"#),
            "",
        )?;
        let _key_numeric = merged(
            "key-numeric",
            one_two(),
            br#"{"foo":{"3":"c"}}"#,
            Some(r#"{"foo":{"1":"a","2":"b","3":"c"}}"#),
            "",
        )?;
        let _invalid_null = merged(
            "invalid-null",
            one_two(),
            br#"{"foo":nul}"#,
            None,
            "cannot unmarshal Foo: expect ull",
        )?;
        let _invalid_malformed = merged(
            "invalid-malformed",
            one_two(),
            br#"{"foo":malformed}"#,
            None,
            "cannot unmarshal Foo: expect { or n, but found m",
        )?;
        Ok(())
    }

    /// A `TestMergeCloneExt` case: name, existing ext (`None` is nil), incoming ext (`None`
    /// omits it), expected ext, and the expected error substring.
    type ExtCase<'a> = (
        &'a str,
        Option<&'a str>,
        Option<&'a str>,
        Option<&'a str>,
        &'a str,
    );

    #[test]
    fn merge_clone_ext() -> Result<(), String> {
        // Go: util/jsonutil/merge_test.go TestMergeCloneExt
        let cases: [ExtCase<'_>; 12] = [
            (
                "both-populated",
                Some(r#"{"a":1,"b":2}"#),
                Some(r#"{"b":200,"c":3}"#),
                Some(r#"{"a":1,"b":200,"c":3}"#),
                "",
            ),
            ("both-omitted", None, None, None, ""),
            ("both-nil", None, Some("null"), None, ""),
            ("both-empty", None, Some("{}"), Some("{}"), ""),
            (
                "ext-omitted",
                Some(r#"{"b":2}"#),
                None,
                Some(r#"{"b":2}"#),
                "",
            ),
            (
                "ext-nil",
                Some(r#"{"b":2}"#),
                Some("null"),
                Some(r#"{"b":2}"#),
                "",
            ),
            (
                "ext-empty",
                Some(r#"{"b":2}"#),
                Some("{}"),
                Some(r#"{"b":2}"#),
                "",
            ),
            (
                "ext-malformed",
                Some(r#"{"b":2}"#),
                Some("malformed"),
                Some(r#"{"b":2}"#),
                "openrtb2.BidRequest.Ext",
            ),
            (
                "existing-nil",
                None,
                Some(r#"{"a":1}"#),
                Some(r#"{"a":1}"#),
                "",
            ),
            (
                "existing-empty",
                Some("{}"),
                Some(r#"{"a":1}"#),
                Some(r#"{"a":1}"#),
                "",
            ),
            (
                "existing-omitted",
                None,
                Some(r#"{"b":2}"#),
                Some(r#"{"b":2}"#),
                "",
            ),
            (
                "existing-malformed",
                Some("malformed"),
                Some(r#"{"a":1}"#),
                Some("malformed"),
                "cannot unmarshal openrtb2.BidRequest.Ext: invalid json on existing object",
            ),
        ];
        for (name, existing, incoming, want, want_err) in cases {
            let mut request = openrtb2::BidRequest {
                ext: existing.map_or_else(RawJson::nil, RawJson::from),
                ..openrtb2::BidRequest::default()
            };
            let data =
                incoming.map_or_else(|| "{}".to_owned(), |ext| format!(r#"{{"ext":{ext}}}"#));
            let err = jsonutil::merge_clone(&mut request, data.as_bytes())
                .err()
                .map_or_else(String::new, |e| e.to_string());
            if want_err.is_empty() != err.is_empty() || !err.contains(want_err) {
                return Err(format!("{name}: error {err:?}, want {want_err:?}"));
            }
            let got =
                (!request.ext.is_nil()).then(|| String::from_utf8_lossy(request.ext.as_bytes()));
            if got.as_deref() != want {
                return Err(format!("{name}: ext {got:?}, want {want:?}"));
            }
        }
        Ok(())
    }

    /// `struct { Foo *json.RawMessage `json:"foo"` }`.
    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(
        go_type = "struct { Foo *json.RawMessage \"json:\\\"foo\\\"\" }",
        anonymous
    )]
    struct RawPtrTest {
        #[gojson(json = "foo")]
        foo: Option<RawJson>,
    }

    /// `struct { Imp *openrtb2.Imp `json:"imp"` }`.
    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(
        go_type = "struct { Imp *openrtb2.Imp \"json:\\\"imp\\\"\" }",
        anonymous
    )]
    struct ImpPtrTest {
        #[gojson(json = "imp")]
        imp: Option<Box<openrtb2::Imp>>,
    }

    /// `struct { Imps map[string]*openrtb2.Imp `json:"imps"` }`.
    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(
        go_type = "struct { Imps map[string]*openrtb2.Imp \"json:\\\"imps\\\"\" }",
        anonymous
    )]
    struct ImpMapTest {
        #[gojson(json = "imps")]
        imps: Option<BTreeMap<String, Option<Box<openrtb2::Imp>>>>,
    }

    #[test]
    fn merge_clone_combinations() -> Result<(), String> {
        // Go: util/jsonutil/merge_test.go TestMergeCloneCombinations
        let _slice_of_ptr = merged(
            "slice-of-ptr",
            ImpsTest {
                imps: Some(vec![Some(imp_with_id("1"))]),
            },
            br#"{"imps":[{"id":"2"}]}"#,
            Some(r#"{"imps":[{"id":"2"}]}"#),
            "",
        )?;
        let raw_ptr = merged(
            "jsonrawmessage-ptr",
            RawPtrTest {
                foo: Some(RawJson::from(r#"{"a":1}"#)),
            },
            br#"{"foo":{"b":2}}"#,
            None,
            "",
        )?;
        check(
            &String::from_utf8_lossy(raw_ptr.foo.unwrap_or_default().as_bytes()),
            r#"{"a":1,"b":2}"#,
            "jsonrawmessage-ptr",
        )?;
        let _struct_ptr = merged(
            "struct-ptr",
            ImpPtrTest {
                imp: Some(imp_with_id("1")),
            },
            br#"{"imp":{"id":"2"}}"#,
            Some(r#"{"imp":{"id":"2"}}"#),
            "",
        )?;
        let _map_of_ptrs = merged(
            "map-of-ptrs",
            ImpMapTest {
                imps: Some(BTreeMap::from([("a".to_owned(), Some(imp_with_id("1")))])),
            },
            br#"{"imps":{"a":{"id":"2"}}}"#,
            Some(r#"{"imps":{"a":{"id":"2"}}}"#),
            "",
        )?;
        Ok(())
    }

    #[test]
    fn merge_clone_roots_and_shapes() -> Result<(), String> {
        // A root slice is reset too (Go decorates the root []T decoder); jsonutil.Unmarshal
        // decodes into the existing elements.
        let one = || vec![*imp_with_id("1")];
        let _cloned = merged(
            "root slice",
            one(),
            br#"[{"tagid":"t"}]"#,
            Some(r#"[{"id":"","tagid":"t"}]"#),
            "",
        )?;
        let mut decoded = one();
        jsonutil::unmarshal(br#"[{"tagid":"t"}]"#, &mut decoded).map_err(|e| e.to_string())?;
        check(
            &marshaled(&decoded)?,
            r#"[{"id":"1","tagid":"t"}]"#,
            "root slice unmarshal",
        )?;

        // []byte from an array, and a malformed null, which MergeClone reads before the slice
        // decoder, so the error is not prefixed with the slice type as jsonutil.Unmarshal's is.
        let bytes = merged(
            "bytes",
            Some(vec![9_u8, 9, 9]),
            b"[1]",
            Some("\"AQ==\""),
            "",
        )?;
        let _bytes_nul = merged("bytes nul", bytes, b"nul", Some("null"), "expect ull")?;
        let mut strs = Some(strings(&["a"]));
        let err = jsonutil::unmarshal(b"nul", &mut strs)
            .err()
            .map_or_else(String::new, |e| e.to_string());
        check(
            &err,
            "cannot unmarshal []string: expect ull",
            "unmarshal nul",
        )?;

        // A root json.RawMessage, and a nested []json.RawMessage whose elements start nil.
        let raw = merged("raw", RawJson::from(r#"{"a":1}"#), br#"{"b":2}"#, None, "")?;
        check(
            &String::from_utf8_lossy(raw.as_bytes()),
            r#"{"a":1,"b":2}"#,
            "raw",
        )?;
        let _raws = merged(
            "raws",
            Some(vec![RawJson::from(r#"{"a":1}"#)]),
            br#"[{"b":2},null]"#,
            Some(r#"[{"b":2},]"#),
            "",
        )?;
        Ok(())
    }

    // ---- goldens -------------------------------------------------------------------------

    /// A quoted, ASCII-only rendering of `bytes` in the style of Go's `strconv.QuoteToASCII`,
    /// for failure messages.
    fn quote(bytes: &[u8]) -> String {
        let mut out = String::from("\"");
        for chunk in bytes.utf8_chunks() {
            for c in chunk.valid().chars() {
                let code = u32::from(c);
                let _written = match c {
                    '"' => write!(out, "\\\""),
                    '\\' => write!(out, "\\\\"),
                    ' '..='~' => write!(out, "{c}"),
                    _ if code <= 0xFFFF => write!(out, "\\u{code:04x}"),
                    _ => write!(out, "\\U{code:08x}"),
                };
            }
            for &b in chunk.invalid() {
                let _written = write!(out, "\\x{b:02x}");
            }
        }
        out.push('"');
        out
    }

    /// Go's `strconv.Unquote` for `QuoteToASCII` output.
    fn unquote(field: &str) -> Result<Vec<u8>, String> {
        let inner = field
            .strip_prefix('"')
            .and_then(|f| f.strip_suffix('"'))
            .ok_or_else(|| format!("not a quoted field: {field}"))?
            .as_bytes();
        let mut out = Vec::with_capacity(inner.len());
        let mut i = 0;
        while let Some(&b) = inner.get(i) {
            if b != b'\\' {
                out.push(b);
                i += 1;
                continue;
            }
            let escape = inner.get(i + 1).copied().unwrap_or(0);
            let simple = match escape {
                b'a' => Some(7),
                b'b' => Some(8),
                b'f' => Some(12),
                b'n' => Some(b'\n'),
                b'r' => Some(b'\r'),
                b't' => Some(b'\t'),
                b'v' => Some(11),
                b'\\' => Some(b'\\'),
                b'"' => Some(b'"'),
                _ => None,
            };
            if let Some(s) = simple {
                out.push(s);
                i += 2;
                continue;
            }
            let width = match escape {
                b'x' => 2,
                b'u' => 4,
                b'U' => 8,
                _ => return Err(format!("bad escape in {field}")),
            };
            let digits = inner.get(i + 2..i + 2 + width).unwrap_or_default();
            let value = from_utf8(digits)
                .ok()
                .and_then(|d| u32::from_str_radix(d, 16).ok())
                .ok_or_else(|| format!("bad hex in {field}"))?;
            if escape == b'x' {
                out.push(u8::try_from(value).map_err(|e| e.to_string())?);
            } else {
                let c = char::from_u32(value).ok_or_else(|| format!("bad rune in {field}"))?;
                let mut buf = [0_u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
            i += 2 + width;
        }
        Ok(out)
    }

    /// `Base`, embedded through a pointer in `Shapes`.
    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Base")]
    struct Base {
        #[gojson(go = "B", json = "b,omitempty")]
        b: String,
        #[gojson(go = "BX", json = "bx,omitempty")]
        bx: RawJson,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Inner")]
    struct Inner {
        #[gojson(go = "ID", json = "id,omitempty")]
        id: String,
        #[gojson(json = "tags,omitempty")]
        tags: Option<Vec<String>>,
        #[gojson(go = "N", json = "n,omitempty")]
        n: Option<isize>,
        #[gojson(json = "ext,omitempty")]
        ext: RawJson,
    }

    /// The generator's `Shapes`: a field of every shape the merge-clone decoders treat
    /// differently.
    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Shapes")]
    struct Shapes {
        #[gojson(go = "Base", embedded)]
        base: Option<Box<Base>>,
        #[gojson(json = "strs")]
        strs: Option<Vec<String>>,
        #[gojson(json = "imps")]
        imps: Option<Vec<Option<Box<openrtb2::Imp>>>>,
        #[gojson(json = "inners")]
        inners: Option<Vec<Inner>>,
        #[gojson(json = "nested")]
        nested: Option<Vec<Option<Vec<isize>>>>,
        #[gojson(go = "PtrSlice", json = "ptrslice")]
        ptr_slice: Option<Box<Option<Vec<String>>>>,
        #[gojson(json = "bytes")]
        bytes: Option<Vec<u8>>,
        #[gojson(go = "MapInt", json = "mapint")]
        map_int: Option<BTreeMap<String, isize>>,
        #[gojson(go = "MapNum", json = "mapnum")]
        map_num: Option<BTreeMap<isize, String>>,
        #[gojson(go = "MapInner", json = "mapinner")]
        map_inner: Option<BTreeMap<String, Option<Box<Inner>>>>,
        #[gojson(go = "MapRaw", json = "mapraw")]
        map_raw: Option<BTreeMap<String, RawJson>>,
        #[gojson(go = "MapSlice", json = "mapslice")]
        map_slice: Option<BTreeMap<String, Option<Vec<String>>>>,
        #[gojson(json = "raws")]
        raws: Option<Vec<RawJson>>,
        #[gojson(go = "RawPtr", json = "rawptr")]
        raw_ptr: Option<RawJson>,
        #[gojson(json = "raw")]
        raw: RawJson,
        #[gojson(json = "any")]
        any: Value,
        #[gojson(go = "AnyMap", json = "anymap")]
        any_map: Option<BTreeMap<String, Value>>,
        #[gojson(go = "Int8Ptr", json = "int8ptr")]
        int8_ptr: Option<i8>,
        #[gojson(go = "StrNum", json = "strnum,string")]
        str_num: isize,
        #[gojson(json = "num")]
        num: StringInt,
        #[gojson(json = "inner")]
        inner: Option<Box<Inner>>,
        #[gojson(go = "InnerVal", json = "innerval")]
        inner_val: Inner,
    }

    /// A `MergeClone` target of the goldens.
    trait Target: JsoniterDecode + GoEncode + Default {
        /// Sets the `Ext` field (the openrtb2 types have one).
        fn set_ext(&mut self, ext: RawJson) -> Result<(), String>;
    }

    macro_rules! with_ext {
        ($($ty:ty),*) => {
            $(
                impl Target for $ty {
                    fn set_ext(&mut self, ext: RawJson) -> Result<(), String> {
                        self.ext = ext;
                        Ok(())
                    }
                }
            )*
        };
    }

    with_ext!(
        openrtb2::Imp,
        openrtb2::Site,
        openrtb2::App,
        openrtb2::User,
        openrtb2::Device,
        openrtb2::BidRequest
    );

    impl Target for Shapes {
        fn set_ext(&mut self, _ext: RawJson) -> Result<(), String> {
            Err("main.Shapes has no Ext".to_owned())
        }
    }

    /// One case: decode the corpus value, set its ext, `MergeClone` the overlay, and marshal.
    /// Returns the marshaled result (or `MARSHAL: <error>`) and the `MergeClone` error text.
    fn run<T: Target>(
        existing: &[u8],
        ext: &[u8],
        overlay: &[u8],
    ) -> Result<(Vec<u8>, Vec<u8>), String> {
        let mut v = T::default();
        jsonutil::unmarshal(existing, &mut v).map_err(|e| format!("corpus: {e}"))?;
        match ext {
            b"-" => {}
            b"nil" => v.set_ext(RawJson::nil())?,
            _ => {
                let bytes = ext
                    .strip_prefix(b"=")
                    .ok_or_else(|| format!("bad ext field {}", quote(ext)))?;
                v.set_ext(RawJson::from_slice(bytes))?;
            }
        }
        let err = jsonutil::merge_clone(&mut v, overlay)
            .err()
            .map_or_else(Vec::new, |e| e.message.into_bytes());
        let result = match jsonutil::marshal(&v) {
            Ok(out) => out,
            Err(e) => [b"MARSHAL: ".as_slice(), e.message.as_bytes()].concat(),
        };
        Ok((result, err))
    }

    fn run_named(
        name: &[u8],
        existing: &[u8],
        ext: &[u8],
        overlay: &[u8],
    ) -> Result<(Vec<u8>, Vec<u8>), String> {
        match name {
            b"openrtb2.Imp" => run::<openrtb2::Imp>(existing, ext, overlay),
            b"openrtb2.Site" => run::<openrtb2::Site>(existing, ext, overlay),
            b"openrtb2.App" => run::<openrtb2::App>(existing, ext, overlay),
            b"openrtb2.User" => run::<openrtb2::User>(existing, ext, overlay),
            b"openrtb2.Device" => run::<openrtb2::Device>(existing, ext, overlay),
            b"openrtb2.BidRequest" => run::<openrtb2::BidRequest>(existing, ext, overlay),
            b"main.Shapes" => run::<Shapes>(existing, ext, overlay),
            _ => Err(format!("unknown target {}", quote(name))),
        }
    }

    #[test]
    fn mergeclone_goldens() -> Result<(), String> {
        let path = format!(
            "{}/testdata/go/mergeclone/mergeclone.txt",
            env!("CARGO_MANIFEST_DIR")
        );
        let text = fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
        let mut corpus: BTreeMap<(Vec<u8>, Vec<u8>), Vec<u8>> = BTreeMap::new();
        let mut cases = 0_usize;
        let mut failures = Vec::new();
        for line in text
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let fields = line
                .split('\t')
                .map(unquote)
                .collect::<Result<Vec<_>, _>>()?;
            match fields.as_slice() {
                [kind, name, index, existing] if kind == b"C" => {
                    let _replaced = corpus.insert((name.clone(), index.clone()), existing.clone());
                }
                [kind, name, index, ext, overlay, want, want_err] if kind == b"M" => {
                    cases += 1;
                    let existing = corpus
                        .get(&(name.clone(), index.clone()))
                        .ok_or_else(|| format!("no corpus value {line}"))?;
                    let (got, got_err) = run_named(name, existing, ext, overlay)?;
                    if got != *want || got_err != *want_err {
                        failures.push(format!(
                            "{} #{} ext {} overlay {}\n  got  {} err {}\n  want {} err {}",
                            quote(name),
                            quote(index),
                            quote(ext),
                            quote(overlay),
                            quote(&got),
                            quote(&got_err),
                            quote(want),
                            quote(want_err),
                        ));
                    }
                }
                _ => return Err(format!("bad row {line}")),
            }
        }
        if cases < 3000 {
            return Err(format!("only {cases} cases"));
        }
        if failures.is_empty() {
            return Ok(());
        }
        let shown: Vec<&String> = failures.iter().take(20).collect();
        Err(format!(
            "{} of {cases} MergeClone cases differ from Go:\n{}",
            failures.len(),
            shown
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        ))
    }
}
