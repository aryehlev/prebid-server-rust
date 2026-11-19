//! `merge_clone` against Go's `jsonutil.MergeClone` (`testdata/golden/merge_clone.json`):
//! `merge_test.go`'s cases with the values they build, the edges of a type with a field of every
//! kind Go's merge decoders treat differently (null into each, the custom decoders, raw merges,
//! repeated, unknown and mistyped members), and 50,000 seeded pairs of an existing value and a
//! patch compared through their batches' digests.

#[cfg(test)]
pub mod seeded;
#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use pbs_jsonutil::{
        IntString, MergeClone, MergeCloneError, StringInt, is_nil_or_empty, is_zero, marshal,
        merge_clone, merge_field, merge_struct, null_default, raw_compact, unmarshal,
    };
    use serde::{Deserialize, Serialize, de::DeserializeOwned};
    use serde_json::{Value, value::RawValue};

    use super::{
        seeded::{SplitMix64, check_digests},
        support::{TestResult, ensure_eq, golden, list, text},
    };

    /// Implements `MergeClone` for a test type as typegen does for a generated one: each key
    /// merges into its field, which errors name as Go names it.
    macro_rules! merge_clone_fields {
        (
            $ty:ident,
            $go:literal,
            { $($key:literal => $field:ident as $go_field:literal),+ $(,)? }
        ) => {
            impl MergeClone for $ty {
                fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
                    merge_struct(patch, |key, value| match key {
                        $($key => merge_field(&mut self.$field, value, $go, $go_field),)+
                        _ => Ok(()),
                    })
                }
            }
        };
    }

    /// Go's `openrtb2.Banner`, the field the cases use.
    #[derive(Default, Serialize, Deserialize)]
    #[serde(default)]
    struct Banner {
        #[serde(
            skip_serializing_if = "String::is_empty",
            deserialize_with = "null_default"
        )]
        id: String,
    }

    merge_clone_fields!(Banner, "openrtb2.Banner", { "id" => id as "ID" });

    /// Go's `openrtb2.Video`, the fields the cases use.
    #[derive(Default, Serialize, Deserialize)]
    #[serde(default)]
    struct Video {
        mimes: Option<Vec<String>>,
        #[serde(
            skip_serializing_if = "String::is_empty",
            deserialize_with = "null_default"
        )]
        podid: String,
    }

    merge_clone_fields!(Video, "openrtb2.Video", {
        "mimes" => mimes as "MIMEs",
        "podid" => podid as "PodID",
    });

    /// Go's `openrtb2.Imp`, the fields the cases use.
    #[derive(Default, Serialize, Deserialize)]
    #[serde(default)]
    struct Imp {
        #[serde(deserialize_with = "null_default")]
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        banner: Option<Banner>,
        #[serde(skip_serializing_if = "Option::is_none")]
        video: Option<Video>,
        #[serde(
            skip_serializing_if = "String::is_empty",
            deserialize_with = "null_default"
        )]
        tagid: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        clickbrowser: Option<i8>,
        #[serde(
            skip_serializing_if = "Vec::is_empty",
            deserialize_with = "null_default"
        )]
        iframebuster: Vec<String>,
    }

    merge_clone_fields!(Imp, "openrtb2.Imp", {
        "id" => id as "ID",
        "banner" => banner as "Banner",
        "video" => video as "Video",
        "tagid" => tagid as "TagID",
        "clickbrowser" => clickbrowser as "ClickBrowser",
        "iframebuster" => iframebuster as "IframeBuster",
    });

    /// Go's `openrtb2.BidRequest`, the fields the cases use.
    #[derive(Default, Serialize, Deserialize)]
    #[serde(default)]
    struct BidRequest {
        #[serde(deserialize_with = "null_default")]
        id: String,
        imp: Option<Vec<Imp>>,
        #[serde(
            skip_serializing_if = "Option::is_none",
            serialize_with = "raw_compact"
        )]
        ext: Option<Box<RawValue>>,
    }

    merge_clone_fields!(BidRequest, "openrtb2.BidRequest", {
        "id" => id as "ID",
        "imp" => imp as "Imp",
        "ext" => ext as "Ext",
    });

    /// Go's `struct { Imps []*openrtb2.Imp }`.
    #[derive(Default, Serialize, Deserialize)]
    #[serde(default)]
    struct ImpSlice {
        imps: Option<Vec<Option<Imp>>>,
    }

    merge_clone_fields!(ImpSlice, "main.impSlice", { "imps" => imps as "Imps" });

    /// Go's `struct { Imp *openrtb2.Imp }`.
    #[derive(Default, Serialize, Deserialize)]
    #[serde(default)]
    struct ImpPtr {
        imp: Option<Imp>,
    }

    merge_clone_fields!(ImpPtr, "main.impPtr", { "imp" => imp as "Imp" });

    /// Go's `struct { Imps map[string]*openrtb2.Imp }`.
    #[derive(Default, Serialize, Deserialize)]
    #[serde(default)]
    struct ImpMap {
        imps: Option<BTreeMap<String, Option<Imp>>>,
    }

    merge_clone_fields!(ImpMap, "main.impMap", { "imps" => imps as "Imps" });

    /// Go's `fooMap`.
    #[derive(Default, Serialize, Deserialize)]
    #[serde(default)]
    struct FooMap {
        foo: Option<BTreeMap<String, i64>>,
    }

    merge_clone_fields!(FooMap, "main.fooMap", { "foo" => foo as "Foo" });

    /// Go's `fooNum`.
    #[derive(Default, Serialize, Deserialize)]
    #[serde(default)]
    struct FooNum {
        foo: Option<BTreeMap<i64, String>>,
    }

    merge_clone_fields!(FooNum, "main.fooNum", { "foo" => foo as "Foo" });

    /// Go's `struct { Foo *json.RawMessage }`.
    #[derive(Default, Serialize, Deserialize)]
    #[serde(default)]
    struct FooRaw {
        #[serde(serialize_with = "raw_compact")]
        foo: Option<Box<RawValue>>,
    }

    merge_clone_fields!(FooRaw, "main.fooRaw", { "foo" => foo as "Foo" });

    /// Go's `Inner`, the struct reached through pointers, slices and maps.
    #[derive(Default, Serialize, Deserialize)]
    #[serde(default)]
    struct Inner {
        #[serde(
            skip_serializing_if = "String::is_empty",
            deserialize_with = "null_default"
        )]
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        n: Option<i64>,
        #[serde(
            skip_serializing_if = "Vec::is_empty",
            deserialize_with = "null_default"
        )]
        tags: Vec<String>,
        #[serde(
            skip_serializing_if = "Option::is_none",
            serialize_with = "raw_compact"
        )]
        ext: Option<Box<RawValue>>,
    }

    merge_clone_fields!(Inner, "main.Inner", {
        "id" => id as "ID",
        "n" => n as "N",
        "tags" => tags as "Tags",
        "ext" => ext as "Ext",
    });

    /// Go's `Shapes`, a field of every kind `MergeClone`'s decoders treat differently.
    #[derive(Default, Serialize, Deserialize)]
    #[serde(default)]
    struct Shapes {
        #[serde(rename = "str", deserialize_with = "null_default")]
        text: String,
        #[serde(deserialize_with = "null_default")]
        int: i64,
        #[serde(skip_serializing_if = "is_zero", deserialize_with = "null_default")]
        int8: i8,
        #[serde(skip_serializing_if = "is_zero", deserialize_with = "null_default")]
        uint: u32,
        #[serde(skip_serializing_if = "is_zero", deserialize_with = "null_default")]
        float: f64,
        #[serde(rename = "bool", deserialize_with = "null_default")]
        flag: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        intptr: Option<i64>,
        strptr: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        inner: Option<Inner>,
        #[serde(deserialize_with = "null_default")]
        innerval: Inner,
        strs: Option<Vec<String>>,
        #[serde(
            skip_serializing_if = "Vec::is_empty",
            deserialize_with = "null_default"
        )]
        inners: Vec<Inner>,
        #[serde(
            skip_serializing_if = "Vec::is_empty",
            deserialize_with = "null_default"
        )]
        innerptrs: Vec<Option<Inner>>,
        #[serde(
            skip_serializing_if = "Vec::is_empty",
            deserialize_with = "null_default"
        )]
        nested: Vec<Option<Vec<i64>>>,
        #[serde(skip_serializing_if = "is_nil_or_empty")]
        mapint: Option<BTreeMap<String, i64>>,
        #[serde(skip_serializing_if = "is_nil_or_empty")]
        mapnum: Option<BTreeMap<i64, String>>,
        #[serde(skip_serializing_if = "is_nil_or_empty")]
        mapinner: Option<BTreeMap<String, Option<Inner>>>,
        #[serde(
            skip_serializing_if = "is_nil_or_empty",
            serialize_with = "raw_compact"
        )]
        mapraw: Option<BTreeMap<String, Option<Box<RawValue>>>>,
        #[serde(
            skip_serializing_if = "Option::is_none",
            serialize_with = "raw_compact"
        )]
        raw: Option<Box<RawValue>>,
        #[serde(
            skip_serializing_if = "Vec::is_empty",
            deserialize_with = "null_default",
            serialize_with = "raw_compact"
        )]
        raws: Vec<Option<Box<RawValue>>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        any: Option<Value>,
        #[serde(skip_serializing_if = "is_zero")]
        num: StringInt,
        #[serde(skip_serializing_if = "is_zero")]
        intstr: IntString,
    }

    merge_clone_fields!(Shapes, "main.Shapes", {
        "str" => text as "Str",
        "int" => int as "Int",
        "int8" => int8 as "Int8",
        "uint" => uint as "Uint",
        "float" => float as "Float",
        "bool" => flag as "Bool",
        "intptr" => intptr as "IntPtr",
        "strptr" => strptr as "StrPtr",
        "inner" => inner as "Inner",
        "innerval" => innerval as "InnerVal",
        "strs" => strs as "Strs",
        "inners" => inners as "Inners",
        "innerptrs" => innerptrs as "InnerPtrs",
        "nested" => nested as "Nested",
        "mapint" => mapint as "MapInt",
        "mapnum" => mapnum as "MapNum",
        "mapinner" => mapinner as "MapInner",
        "mapraw" => mapraw as "MapRaw",
        "raw" => raw as "Raw",
        "raws" => raws as "Raws",
        "any" => any as "Any",
        "num" => num as "Num",
        "intstr" => intstr as "IntStr",
    });

    /// Go's `mergeCase`: `patch` merged into the `T` that `existing` (Go's write of the value the
    /// case builds) decodes to, then written, or the error's text.
    fn merge<T>(existing: &str, patch: &str) -> TestResult<Result<String, String>>
    where
        T: MergeClone + Serialize + DeserializeOwned,
    {
        let mut value: T = unmarshal(existing.as_bytes())?;
        Ok(match merge_clone(&mut value, patch.as_bytes()) {
            Ok(()) => Ok(String::from_utf8(marshal(&value)?)?),
            Err(error) => Err(error.into_message()),
        })
    }

    /// A curated case merged into the type its test builds.
    fn merge_case(
        test: &str,
        name: &str,
        existing: &str,
        patch: &str,
    ) -> TestResult<Result<String, String>> {
        match (test, name) {
            ("Shapes", _) => merge::<Shapes>(existing, patch),
            ("TestMergeCloneExt", _) => merge::<BidRequest>(existing, patch),
            ("TestMergeCloneMap", "null" | "key-string") => merge::<FooMap>(existing, patch),
            ("TestMergeCloneMap", _) => merge::<FooNum>(existing, patch),
            (_, "replace-not-overlay" | "slice-of-ptr") => merge::<ImpSlice>(existing, patch),
            (_, "jsonrawmessage-ptr") => merge::<FooRaw>(existing, patch),
            (_, "struct-ptr") => merge::<ImpPtr>(existing, patch),
            (_, "map-of-ptrs") => merge::<ImpMap>(existing, patch),
            _ => merge::<Imp>(existing, patch),
        }
    }

    /// The `Type.Field` a `cannot unmarshal` text names.
    fn go_field(message: &str) -> Option<&str> {
        message
            .strip_prefix("cannot unmarshal ")?
            .split(": ")
            .next()
    }

    /// Where Go fails, the port fails too; when the patch is JSON it names the field Go names,
    /// and Prebid's own text (`invalid type`) is exact. The library's words differ
    /// (`json.error-texts`).
    fn check_failure(
        what: &str,
        patch: &str,
        go: &str,
        rust: Result<String, String>,
    ) -> TestResult {
        let rust = rust
            .err()
            .ok_or_else(|| format!("{what}: merged where Go failed with {go}"))?;
        if serde_json::from_str::<&RawValue>(patch).is_ok()
            && let Some(field) = go_field(go)
        {
            ensure_eq(&go_field(&rust), &Some(field), what)?;
        }
        if go.ends_with(": invalid type") {
            ensure_eq(&rust.as_str(), &go, what)?;
        }
        Ok(())
    }

    /// `merge_test.go`'s cases and the `Shapes` edges give Go's merged value byte for byte, and
    /// fail where Go fails (see `check_failure`). Three cases differ, each ledgered: a key that
    /// names a field only case-insensitively merges nothing (`json.key-case`), and `StringInt`
    /// reads `-` and `+5` as Rust's integer parser does (`json.string-int-parse`). One case is
    /// Go's alone: an existing `json.RawMessage` that is not JSON, which a `RawValue` cannot hold.
    // Go: util/jsonutil/merge.go:31-176 (MergeClone), merge_test.go:13-470
    #[test]
    fn merge_test_cases_match_go() -> TestResult {
        let doc = golden("merge_clone.json")?;
        let cases = list(&doc, "/curated")?;
        let mut compared = 0;
        for case in cases {
            let (test, name) = (text(case, "/test")?, text(case, "/name")?);
            let (existing, patch) = (text(case, "/existing")?, text(case, "/patch")?);
            let (go, go_err) = (text(case, "/result")?, text(case, "/err")?);
            let what = format!("{test} {name}");
            if existing.is_empty() {
                ensure_eq(&name, &"existing-malformed", &what)?;
                continue;
            }
            let rust = merge_case(test, name, existing, patch)?;
            if name.ends_with("/iframeBuster") && go_err.is_empty() {
                ensure_eq(&rust, &Ok(existing.to_owned()), &what)?;
            } else if patch == r#"{"num":"-"}"# {
                ensure_eq(&(go_err.is_empty(), rust.is_err()), &(true, true), &what)?;
            } else if patch == r#"{"num":"+5"}"# {
                ensure_eq(&go_err.is_empty(), &false, &what)?;
                let five = existing.replace(r#""num":9"#, r#""num":5"#);
                ensure_eq(&rust, &Ok(five), &what)?;
            } else if go_err.is_empty() {
                ensure_eq(&rust, &Ok(go.to_owned()), &what)?;
            } else {
                check_failure(&what, patch, go_err, rust)?;
            }
            compared += 1;
        }
        ensure_eq(&compared, &(cases.len() - 1), "cases compared")
    }

    /// A `json.RawMessage` merge fails where the document or the patch holds a number beyond
    /// `f64`'s range (`json.merge-patch-limits`): an existing value with Go's text for one it
    /// cannot merge into, a patch with evanphx's; Go merges both.
    // Go: util/jsonutil/merge.go:144-176 (extMergeDecoder), evanphx/json-patch v5.9.0 MergePatch
    #[test]
    fn raw_merge_limits_fail() -> TestResult {
        let doc = golden("merge_patch.json")?;
        let mut seen = 0;
        for case in list(&doc, "/curated")? {
            let (kind, existing, patch) = (
                text(case, "/kind")?,
                text(case, "/doc")?,
                text(case, "/patch")?,
            );
            if kind != "limits" || !(existing.contains("1e400") || patch.contains("1e400")) {
                continue;
            }
            ensure_eq(&text(case, "/err")?, &"", "Go merges")?;
            let rust = merge::<FooRaw>(
                &format!("{{\"foo\":{existing}}}"),
                &format!("{{\"foo\":{patch}}}"),
            )?;
            let want = if existing.contains("1e400") {
                "cannot unmarshal main.fooRaw.Foo: invalid json on existing object"
            } else {
                "cannot unmarshal main.fooRaw.Foo: Invalid JSON Patch"
            };
            ensure_eq(&rust, &Err(want.to_owned()), existing)?;
            seen += 1;
        }
        ensure_eq(&seen, &2, "limit cases with 1e400")
    }

    /// Go's `mcStrings`.
    const STRINGS: [&str; 4] = ["a", "b", "x y", ""];
    /// Go's `mcKeys`.
    const KEYS: [&str; 3] = ["a", "b", "c"];
    /// Go's `mcFloats`.
    const FLOATS: [&str; 5] = ["0.5", "1.25", "-2.75", "0.125", "3.5"];
    /// Go's `mcNumKeys`.
    const NUM_KEYS: [&str; 6] = ["-1", "0", "1", "2", "7", "9"];
    /// Go's `mcFields`.
    const FIELDS: [&str; 23] = [
        "str",
        "int",
        "int8",
        "uint",
        "float",
        "bool",
        "intptr",
        "strptr",
        "inner",
        "innerval",
        "strs",
        "inners",
        "innerptrs",
        "nested",
        "mapint",
        "mapnum",
        "mapinner",
        "mapraw",
        "raw",
        "raws",
        "any",
        "num",
        "intstr",
    ];
    /// Go's `mcInnerFld`.
    const INNER_FIELDS: [&str; 4] = ["id", "n", "tags", "ext"];
    /// Go's `mcWrong`'s list.
    const WRONG: [&str; 8] = [r#""x""#, "[]", "{}", "1.5", "true", "300", "-1", r#""12""#];
    /// Go: the `num` values.
    const NUMS: [&str; 5] = [r#""12""#, "7", r#""""#, r#""-30""#, "0"];
    /// Go: the `intstr` values.
    const INT_STRINGS: [&str; 5] = [r#""ab""#, "30", r#""7""#, "1.5", "-2"];
    /// Go: the `bool` values.
    const BOOLS: [&str; 2] = ["true", "false"];

    /// `n - by` in decimal.
    fn offset(n: usize, by: usize) -> String {
        n.checked_sub(by)
            .map_or_else(|| format!("-{}", by - n), |up| up.to_string())
    }

    /// Go's `mcString`.
    fn quoted(rng: &mut SplitMix64) -> String {
        format!("\"{}\"", rng.pick(&STRINGS))
    }

    /// Go's `mcInt`.
    fn int(rng: &mut SplitMix64) -> String {
        offset(rng.below(2000), 1000)
    }

    /// Go's `mcScalar`.
    fn scalar(rng: &mut SplitMix64) -> String {
        match rng.below(5) {
            0 => quoted(rng),
            1 => int(rng),
            2 => rng.pick(&FLOATS).to_owned(),
            3 => "true".to_owned(),
            _ => "false".to_owned(),
        }
    }

    /// Go's `mcArray`.
    fn array<F: FnMut(&mut SplitMix64) -> String>(rng: &mut SplitMix64, mut item: F) -> String {
        let n = rng.below(4);
        let parts: Vec<String> = (0..n).map(|_| item(rng)).collect();
        format!("[{}]", parts.join(","))
    }

    /// Go's `mcObject`.
    fn object<F>(rng: &mut SplitMix64, keys: &[&str], mut value: F) -> String
    where
        F: FnMut(&mut SplitMix64) -> String,
    {
        let n = rng.below(4);
        let parts: Vec<String> = (0..n)
            .map(|_| {
                let key = rng.pick(keys);
                format!("\"{key}\":{}", value(rng))
            })
            .collect();
        format!("{{{}}}", parts.join(","))
    }

    /// Go's `mcRaw`.
    fn raw(rng: &mut SplitMix64, depth: usize) -> String {
        let n = rng.below(4);
        let mut used = BTreeSet::new();
        let mut parts = Vec::new();
        for _ in 0..n {
            let key = rng.pick(&KEYS);
            if !used.insert(key) {
                continue;
            }
            parts.push(format!("\"{key}\":{}", raw_member(rng, depth)));
        }
        parts.sort();
        format!("{{{}}}", parts.join(","))
    }

    /// Go's `mcRawValue`.
    fn raw_member(rng: &mut SplitMix64, depth: usize) -> String {
        let mut n = rng.below(6);
        if depth >= 2 && n >= 4 {
            n = rng.below(4);
        }
        match n {
            0 | 1 => scalar(rng),
            2 => "null".to_owned(),
            3 => array(rng, scalar),
            _ => raw(rng, depth + 1),
        }
    }

    /// Go's `mcAny`.
    fn any(rng: &mut SplitMix64, depth: usize) -> String {
        let mut n = rng.below(8);
        if depth >= 2 && n >= 6 {
            n = rng.below(6);
        }
        match n {
            0 => "null".to_owned(),
            1..=5 => scalar(rng),
            6 => array(rng, |s| any(s, depth + 1)),
            _ => object(rng, &KEYS, |s| any(s, depth + 1)),
        }
    }

    /// Go's `mcNonNullRaw`.
    fn non_null_raw(rng: &mut SplitMix64) -> String {
        match rng.below(3) {
            0 => scalar(rng),
            1 => array(rng, scalar),
            _ => raw(rng, 1),
        }
    }

    /// Go's `mcMaybe`.
    fn maybe<F>(rng: &mut SplitMix64, patch: bool, n: usize, value: F) -> String
    where
        F: FnOnce(&mut SplitMix64) -> String,
    {
        if patch && rng.below(n) == 0 {
            "null".to_owned()
        } else {
            value(rng)
        }
    }

    /// Go's `mcInnerValue`.
    fn inner_value(rng: &mut SplitMix64, field: &str, patch: bool) -> String {
        match field {
            "id" => maybe(rng, patch, 6, quoted),
            "n" => maybe(rng, patch, 4, int),
            "tags" => maybe(rng, patch, 4, |s| array(s, quoted)),
            _ => {
                if patch && rng.below(4) == 0 {
                    "null".to_owned()
                } else {
                    raw(rng, 1)
                }
            }
        }
    }

    /// Go's `mcInnerObject`.
    fn inner_object(rng: &mut SplitMix64, patch: bool) -> String {
        let n = rng.below(4);
        let mut used = BTreeSet::new();
        let mut parts = Vec::new();
        for _ in 0..n {
            let field = rng.pick(&INNER_FIELDS);
            if !used.insert(field) && !patch {
                continue;
            }
            parts.push(format!("\"{field}\":{}", inner_value(rng, field, patch)));
        }
        format!("{{{}}}", parts.join(","))
    }

    /// `mcMaybe(patch, n, mcInnerObject)`.
    fn inner_or_null(rng: &mut SplitMix64, patch: bool, n: usize, inner_patch: bool) -> String {
        maybe(rng, patch, n, |s| inner_object(s, inner_patch))
    }

    /// Go's `mcValue`.
    fn value(rng: &mut SplitMix64, field: &str, patch: bool) -> String {
        if patch && field != "raw" && field != "float" && rng.below(25) == 0 {
            return rng.pick(&WRONG).to_owned();
        }
        match field {
            "str" => maybe(rng, patch, 6, quoted),
            "int" => maybe(rng, patch, 6, int),
            "int8" => maybe(rng, patch, 6, |s| offset(s.below(256), 128)),
            "uint" => maybe(rng, patch, 6, |s| s.below(100_000).to_string()),
            "float" => maybe(rng, patch, 6, |s| s.pick(&FLOATS).to_owned()),
            "bool" => maybe(rng, patch, 6, |s| s.pick(&BOOLS).to_owned()),
            "intptr" => maybe(rng, true, 4, int),
            "strptr" => maybe(rng, true, 4, quoted),
            "inner" | "innerval" => inner_or_null(rng, true, 5, patch),
            "strs" => maybe(rng, true, 5, |s| array(s, quoted)),
            "inners" => maybe(rng, true, 5, |s| {
                array(s, |t| inner_or_null(t, patch, 5, patch))
            }),
            "innerptrs" => maybe(rng, true, 5, |s| {
                array(s, |t| inner_or_null(t, true, 4, patch))
            }),
            "nested" => maybe(rng, true, 5, |s| {
                array(s, |t| maybe(t, true, 4, |u| array(u, int)))
            }),
            "mapint" => maybe(rng, true, 5, |s| object(s, &KEYS, int)),
            "mapnum" => maybe(rng, true, 5, |s| object(s, &NUM_KEYS, quoted)),
            "mapinner" => maybe(rng, true, 5, |s| {
                object(s, &KEYS, |t| inner_or_null(t, true, 4, patch))
            }),
            "mapraw" => maybe(rng, true, 5, |s| object(s, &KEYS, non_null_raw)),
            "raw" => maybe(rng, patch, 4, |s| raw(s, 1)),
            "raws" => maybe(rng, true, 5, |s| array(s, non_null_raw)),
            "any" => any(rng, 1),
            "num" => rng.pick(&NUMS).to_owned(),
            _ => rng.pick(&INT_STRINGS).to_owned(),
        }
    }

    /// Go's `mcDocument`.
    fn document(rng: &mut SplitMix64, patch: bool) -> String {
        let n = rng.below(6);
        let mut used = BTreeSet::new();
        let mut parts = Vec::new();
        for _ in 0..n {
            if patch && rng.below(15) == 0 {
                parts.push(format!("\"zz\":{}", any(rng, 1)));
                continue;
            }
            let field = rng.pick(&FIELDS);
            if !used.insert(field) && !patch {
                continue;
            }
            parts.push(format!("\"{field}\":{}", value(rng, field, patch)));
        }
        format!("{{{}}}", parts.join(","))
    }

    /// Go's `mcPatch`.
    fn patch_document(rng: &mut SplitMix64) -> String {
        match rng.below(40) {
            0 => "null".to_owned(),
            1 => "[]".to_owned(),
            2 => r#""x""#.to_owned(),
            3 => "{}".to_owned(),
            _ => document(rng, true),
        }
    }

    /// Go's `mergeCloneOutcome`.
    fn outcome(existing: &str, patch: &str) -> TestResult<String> {
        let mut shapes: Shapes =
            unmarshal(existing.as_bytes()).map_err(|e| format!("{existing}: {e}"))?;
        Ok(match merge_clone(&mut shapes, patch.as_bytes()) {
            Ok(()) => String::from_utf8(marshal(&shapes)?)?,
            Err(_) => "error".to_owned(),
        })
    }

    /// 50,000 seeded existing values and patches over `Shapes` give Go's merged value byte for
    /// byte, and fail where Go fails.
    // Go: util/jsonutil/merge.go:31-176; testdata/go/gen/mergeclone.go (mergeCloneCorpus)
    #[test]
    fn seeded_merges_match_go() -> TestResult {
        let doc = golden("merge_clone.json")?;
        check_digests(&doc, "/seeded", "merge_clone", |rng| {
            let existing = document(rng, false);
            let patch = patch_document(rng);
            let merged = outcome(&existing, &patch)?;
            Ok((format!("{existing}\t{patch}"), merged))
        })
    }
}
