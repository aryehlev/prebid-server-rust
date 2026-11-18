//! Adversarial `jsonutil.MergeClone` goldens from `testdata/go/mergeclone_review` (Go 1.25,
//! json-iterator 1.1.12): a struct with the field shapes the first generator leaves out (named
//! slice and map types, `config.AccountModules`-shaped maps of raw messages, pointers to raw
//! messages and maps, `[][]byte`, `**T`, `,string` pointers and slices, `json.Number`, structs
//! embedded by value and by pointer, a recursive type) and `openrtb2.Imp`, merged with generated
//! overlays (duplicate and re-cased keys, whitespace, malformed values, numbers jsoniter skips
//! but `encoding/json` rejects) over generated existing values, some of them `MergeClone`
//! results themselves, as `account/fetcher.go` merges the defaults and then the account.
//!
//! Regenerate the goldens with `go run ./mergeclone_review` in `testdata/go`.

#[cfg(test)]
mod mergeclone_review {
    use core::str::from_utf8;
    use std::{collections::BTreeMap, fmt::Write as _, fs};

    use pbs_gojson::{
        Encoder, GoEncode, GoJson, GoKind, GoType, JsonError, JsonNumber, JsoniterDecode, RawJson,
        StdDecode,
        jsoniter::Iter,
        jsonutil::{self, StringInt},
        stdjson::StdDecoder,
    };
    use pbs_openrtb::openrtb2;
    use serde_json::Value;

    /// `type Strs []string`, written out by hand as `#[derive(GoJson)]` should generate it: its
    /// jsoniter decoder goes through `jsoniter_decode_named::<Self>`, so slice errors name
    /// `main.Strs` as Go's do. (The derive still calls `jsoniter_decode` on the inner value,
    /// which names `[]string`.)
    #[derive(Clone, Debug, Default)]
    struct Strs(Option<Vec<String>>);

    impl GoType for Strs {
        const KIND: GoKind = GoKind::Slice;

        fn go_type_name(out: &mut String) {
            out.push_str("main.Strs");
        }
    }

    impl GoEncode for Strs {
        fn go_encode(&self, enc: &mut Encoder) {
            self.0.go_encode(enc);
        }

        fn go_is_empty(&self) -> bool {
            self.0.go_is_empty()
        }
    }

    impl JsoniterDecode for Strs {
        fn jsoniter_decode(&mut self, it: &mut Iter<'_>) {
            self.0.jsoniter_decode_named::<Self>(it);
        }
    }

    impl StdDecode for Strs {
        fn std_decode_as<N: GoType + ?Sized>(
            &mut self,
            d: &mut StdDecoder<'_>,
        ) -> Result<(), JsonError> {
            self.0.std_decode_as::<N>(d)
        }

        fn std_literal_as<N: GoType + ?Sized>(
            &mut self,
            item: &[u8],
            from_quoted: bool,
            d: &mut StdDecoder<'_>,
        ) -> Result<(), JsonError> {
            self.0.std_literal_as::<N>(item, from_quoted, d)
        }
    }

    /// `type RawMap map[string]json.RawMessage`.
    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.RawMap")]
    struct RawMap(Option<BTreeMap<String, RawJson>>);

    /// `type Modules map[string]map[string]json.RawMessage` (`config.AccountModules`).
    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Modules")]
    struct Modules(Option<BTreeMap<String, Option<BTreeMap<String, RawJson>>>>);

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Leaf")]
    struct Leaf {
        #[gojson(go = "ID", json = "id,omitempty")]
        id: String,
        #[gojson(go = "N", json = "n,omitempty")]
        n: Option<isize>,
        #[gojson(json = "ext,omitempty")]
        ext: RawJson,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Emb")]
    struct Emb {
        #[gojson(go = "E", json = "e,omitempty")]
        e: String,
        #[gojson(go = "EX", json = "ex,omitempty")]
        ex: RawJson,
        #[gojson(go = "ES", json = "es,omitempty")]
        es: Option<Vec<String>>,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.EmbPtr")]
    struct EmbPtr {
        #[gojson(go = "P", json = "p,omitempty")]
        p: Option<Box<Leaf>>,
        #[gojson(go = "PS", json = "ps,omitempty")]
        ps: Option<Vec<isize>>,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Node")]
    struct Node {
        #[gojson(go = "ID", json = "id,omitempty")]
        id: String,
        #[gojson(json = "kids,omitempty")]
        kids: Option<Vec<NodePtr>>,
        #[gojson(json = "ext,omitempty")]
        ext: RawJson,
    }

    /// `*Node` (named so that `Node`'s derived impls, which cannot use `Self`, need not
    /// repeat `Node`).
    type NodePtr = Option<Box<Node>>;

    /// The generator's `main.Review`.
    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Review")]
    struct Review {
        #[gojson(go = "Emb", embedded)]
        emb: Emb,
        #[gojson(go = "EmbPtr", embedded)]
        emb_ptr: Option<Box<EmbPtr>>,
        #[gojson(json = "named")]
        named: Strs,
        #[gojson(json = "namedptr")]
        named_ptr: Option<Strs>,
        #[gojson(json = "rawmap")]
        raw_map: RawMap,
        #[gojson(json = "modules")]
        modules: Modules,
        #[gojson(json = "rawptrs")]
        raw_ptrs: Option<Vec<Option<RawJson>>>,
        #[gojson(json = "maprawptr")]
        map_raw_ptr: Option<BTreeMap<String, Option<RawJson>>>,
        #[gojson(json = "ptrraws")]
        ptr_raws: Option<Box<Option<Vec<RawJson>>>>,
        #[gojson(json = "rawraws")]
        raw_raws: Option<Vec<Option<Vec<RawJson>>>>,
        #[gojson(json = "ptrmap")]
        ptr_map: Option<Box<Option<BTreeMap<String, isize>>>>,
        #[gojson(json = "mapmap")]
        map_map: Option<BTreeMap<String, Option<BTreeMap<String, isize>>>>,
        #[gojson(json = "slicemap")]
        slice_map: Option<Vec<Option<BTreeMap<String, isize>>>>,
        #[gojson(json = "bytess")]
        bytess: Option<Vec<Option<Vec<u8>>>>,
        #[gojson(json = "ptrptr")]
        ptr_ptr: Option<Box<Option<Box<Leaf>>>>,
        #[gojson(json = "strnumptr,string")]
        str_num_ptr: Option<isize>,
        #[gojson(json = "strslice,string")]
        str_slice: Option<Vec<isize>>,
        #[gojson(json = "number")]
        number: JsonNumber,
        #[gojson(json = "numptr")]
        num_ptr: Option<JsonNumber>,
        #[gojson(json = "leafs")]
        leafs: Option<Vec<Leaf>>,
        #[gojson(json = "leafmap")]
        leaf_map: Option<BTreeMap<String, Leaf>>,
        #[gojson(json = "leafptrmap")]
        leaf_ptr_map: Option<BTreeMap<String, Option<Box<Leaf>>>>,
        #[gojson(json = "any")]
        any: Value,
        #[gojson(json = "anys")]
        anys: Option<Vec<Value>>,
        #[gojson(json = "anymap")]
        any_map: Option<BTreeMap<String, Value>>,
        #[gojson(json = "flag")]
        flag: bool,
        #[gojson(json = "f")]
        f: f64,
        #[gojson(go = "U8", json = "u8")]
        u8: u8,
        #[gojson(json = "kids")]
        kids: Option<Vec<Option<Box<Node>>>>,
        #[gojson(json = "leaf")]
        leaf: Option<Box<Leaf>>,
        #[gojson(json = "leafval")]
        leaf_val: Leaf,
        #[gojson(go = "SIntPtr", json = "sintptr")]
        s_int_ptr: Option<StringInt>,
        #[gojson(json = "raw")]
        raw: RawJson,
        #[gojson(json = "rawomit,omitempty")]
        raw_omit: RawJson,
        #[gojson(json = "maprawraw")]
        map_raw_raw: Option<BTreeMap<String, Option<Vec<RawJson>>>>,
    }

    /// A `MergeClone` target of the goldens, with the raw message the generator sets.
    trait Target: JsoniterDecode + GoEncode + Default {
        fn set_raw(&mut self, raw: RawJson);
    }

    impl Target for Review {
        fn set_raw(&mut self, raw: RawJson) {
            self.raw = raw;
        }
    }

    impl Target for openrtb2::Imp {
        fn set_raw(&mut self, raw: RawJson) {
            self.ext = raw;
        }
    }

    /// One case: the existing value (decoded or merged into a zero value), its raw message,
    /// `MergeClone` of the overlay, and `jsonutil.Marshal`.
    fn run<T: Target>(
        mode: &[u8],
        existing: &[u8],
        raw: &[u8],
        overlay: &[u8],
    ) -> Result<(Vec<u8>, Vec<u8>), String> {
        let mut v = T::default();
        let decoded = if mode == b"U" {
            jsonutil::unmarshal(existing, &mut v)
        } else {
            jsonutil::merge_clone(&mut v, existing)
        };
        decoded.map_err(|e| format!("existing {}: {e}", quote(existing)))?;
        match raw {
            b"-" => {}
            b"nil" => v.set_raw(RawJson::nil()),
            _ => {
                let bytes = raw
                    .strip_prefix(b"=")
                    .ok_or_else(|| format!("bad raw field {}", quote(raw)))?;
                v.set_raw(RawJson::from_slice(bytes));
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

    /// A quoted, ASCII-only rendering of `bytes` for failure messages.
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

    #[test]
    fn mergeclone_review_goldens() -> Result<(), String> {
        let path = format!(
            "{}/testdata/go/mergeclone_review/mergeclone_review.txt",
            env!("CARGO_MANIFEST_DIR")
        );
        let text = fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
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
            let [kind, target, mode, existing, raw, overlay, want, want_err] = fields.as_slice()
            else {
                return Err(format!("bad row {line}"));
            };
            if kind != b"R" {
                return Err(format!("bad row {line}"));
            }
            cases += 1;
            let (got, got_err) = match target.as_slice() {
                b"main.Review" => run::<Review>(mode, existing, raw, overlay)?,
                b"openrtb2.Imp" => run::<openrtb2::Imp>(mode, existing, raw, overlay)?,
                _ => return Err(format!("unknown target {}", quote(target))),
            };
            if got != *want || got_err != *want_err {
                failures.push(format!(
                    "{} {} existing {} raw {} overlay {}\n  got  {} err {}\n  want {} err {}",
                    quote(target),
                    quote(mode),
                    quote(existing),
                    quote(raw),
                    quote(overlay),
                    quote(&got),
                    quote(&got_err),
                    quote(want),
                    quote(want_err),
                ));
            }
        }
        if cases < 4000 {
            return Err(format!("only {cases} cases"));
        }
        if failures.is_empty() {
            return Ok(());
        }
        let shown: Vec<&str> = failures.iter().take(25).map(String::as_str).collect();
        Err(format!(
            "{} of {cases} MergeClone cases differ from Go:\n{}",
            failures.len(),
            shown.join("\n")
        ))
    }

    /// `MergeClone` of `overlay` into `existing`: the result and the error text.
    fn merge<T: JsoniterDecode + GoEncode>(
        existing: &mut T,
        overlay: &[u8],
    ) -> Result<(String, String), String> {
        let err = jsonutil::merge_clone(existing, overlay)
            .err()
            .map_or_else(String::new, |e| e.to_string());
        let out = jsonutil::marshal(existing).map_err(|e| e.to_string())?;
        Ok((String::from_utf8_lossy(&out).into_owned(), err))
    }

    fn check(got: &(String, String), want: (&str, &str), what: &str) -> Result<(), String> {
        if got.0 == want.0 && got.1 == want.1 {
            Ok(())
        } else {
            Err(format!("{what}: got {got:?}, want {want:?}"))
        }
    }

    /// Go's `ptrCloneDecoder` reads a `null` itself before `OptionalDecoder` reads it again,
    /// and after an error (when `unreadByte` does nothing) each of those reads consumes a byte.
    /// Here the ext merge fails, `"banner"`'s pointer decoder eats two bytes, the parse
    /// desyncs and `"id"` is never decoded.
    #[test]
    fn pointer_clone_reads_null_first() -> Result<(), String> {
        // Go 1.25, json-iterator 1.1.12: util/jsonutil/merge.go ptrCloneDecoder.Decode
        let mut imp = openrtb2::Imp::default();
        jsonutil::unmarshal(b"{\t \"bidfloor\":\n2E-2}", &mut imp).map_err(|e| e.to_string())?;
        imp.ext = RawJson::from("{");
        let overlay = concat!(
            "{ \"ext\":{\"a\":{},\r\n  \"c\"\t :{\"c\"\t :\"x\"},\"prebid\":[]}\n,",
            "\"banner\"\t :{\t },\"id\":\r\n  \"<&>\"}"
        )
        .as_bytes();
        check(
            &merge(&mut imp, overlay)?,
            (
                r#"{"id":"","banner":{},"bidfloor":0.02,"ext":}"#,
                "cannot unmarshal Ext: invalid json on existing object",
            ),
            "openrtb2.Imp",
        )
    }

    /// `type W struct { Named Strs; NamedPtr *Strs }`.
    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.W")]
    struct W {
        #[gojson(json = "named")]
        named: Strs,
        #[gojson(json = "namedptr")]
        named_ptr: Option<Strs>,
    }

    /// jsoniter's `sliceDecoder` for a named slice type wraps its errors with that name, which
    /// `JsoniterDecode::jsoniter_decode_named` models (in `Unmarshal` and `MergeClone`, and
    /// through a pointer).
    #[test]
    fn named_slice_errors_name_the_type() -> Result<(), String> {
        // Go 1.25, json-iterator 1.1.12: reflect_slice.go sliceDecoder.Decode
        let elem = "cannot unmarshal main.Strs: expects \" or n, but found 1";
        let top: [(&[u8], &str, &str); 4] = [
            (b"[1]", r#"[""]"#, elem),
            (br#"["a",1]"#, r#"["a",""]"#, elem),
            (b"{}", "null", "decode slice: expect [ or n, but found {"),
            (
                br#"["a""#,
                r#"["a"]"#,
                "decode slice: expect ], but found \0",
            ),
        ];
        for (doc, want, want_err) in top {
            let what = String::from_utf8_lossy(doc).into_owned();
            let mut s = Strs::default();
            let err = jsonutil::unmarshal(doc, &mut s)
                .err()
                .map_or_else(String::new, |e| e.to_string());
            let out = jsonutil::marshal(&s).map_err(|e| e.to_string())?;
            check(
                &(String::from_utf8_lossy(&out).into_owned(), err),
                (want, want_err),
                &format!("Unmarshal {what}"),
            )?;
            let mut base = Strs(Some(vec!["x".to_owned(), "y".to_owned()]));
            check(
                &merge(&mut base, doc)?,
                (want, want_err),
                &format!("MergeClone {what}"),
            )?;
        }
        let fields: [(&[u8], &str, &str); 3] = [
            (
                br#"{"named":[1]}"#,
                r#"{"named":[""],"namedptr":null}"#,
                elem,
            ),
            (
                br#"{"namedptr":[1]}"#,
                r#"{"named":null,"namedptr":[""]}"#,
                elem,
            ),
            (
                br#"{"named":{}}"#,
                r#"{"named":null,"namedptr":null}"#,
                "cannot unmarshal main.W.Named: decode slice: expect [ or n, but found {",
            ),
        ];
        for (doc, want, want_err) in fields {
            let what = String::from_utf8_lossy(doc).into_owned();
            let mut w = W::default();
            let err = jsonutil::unmarshal(doc, &mut w)
                .err()
                .map_or_else(String::new, |e| e.to_string());
            let out = jsonutil::marshal(&w).map_err(|e| e.to_string())?;
            check(
                &(String::from_utf8_lossy(&out).into_owned(), err),
                (want, want_err),
                &format!("Unmarshal {what}"),
            )?;
            let mut base = W::default();
            check(
                &merge(&mut base, doc)?,
                (want, want_err),
                &format!("MergeClone {what}"),
            )?;
        }
        Ok(())
    }

    /// Go's `mapCloneDecoder` reads a `null` before `mapDecoder` runs. After the missing `:`
    /// it consumes a byte, so the struct decoder stops where it still wraps the error with
    /// the struct's name; without that read the parse goes on to a `,` and the name is lost.
    #[test]
    fn map_clone_reads_null_first() -> Result<(), String> {
        // Go 1.25, json-iterator 1.1.12: util/jsonutil/merge.go mapCloneDecoder.Decode (one of
        // the goldens)
        let mut review = Review::default();
        jsonutil::unmarshal(
            concat!(
                "{\"number\"\n:\"\",\"number\":\"x\",\"es\":[],",
                "\"MODULES\":{\"acme\":\t {\"x\":\t -2},\"acme\"\t :{\"y\":\r\n  1,\"x\"\n:\n{}}}}"
            )
            .as_bytes(),
            &mut review,
        )
        .map_err(|e| e.to_string())?;
        let (_out, err) = merge(
            &mut review,
            b"{\"leafmap\" \t {},\"flag\":true,\"p\":\nnull}",
        )?;
        if err == "cannot unmarshal main.Review.LeafMap: expect : after object field, but found {" {
            Ok(())
        } else {
            Err(format!("error {err:?}"))
        }
    }
}
