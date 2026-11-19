//! `merge_patch` (RFC 7386 through `json-patch`) against evanphx/json-patch v5.9.0 `MergePatch`
//! (`testdata/golden/merge_patch.json`): RFC 7386's appendix, every stored-request, stored-imp,
//! account, first-party data, ext and event merge input drawn from the upstream corpora, 50,000
//! seeded pairs, and the cases where evanphx departs from the RFC or keeps an input's spelling,
//! each pinned for its ledger entry.

#[cfg(test)]
pub mod seeded;
#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use pbs_jsonutil::{MergePatchError, merge_patch, replace_lone_surrogates};
    use serde_json::Value;

    use super::{
        seeded::{SplitMix64, check_digests},
        support::{TestResult, ensure_eq, golden, list, text},
    };

    /// Go's `mpKeys`.
    const KEYS: [&str; 3] = ["a", "b", "c"];

    /// `merge_patch`'s output or error as one line, as Go's `mergePatchResult` writes it.
    fn result(doc: &str, patch: &str) -> String {
        match merge_patch(doc.as_bytes(), patch.as_bytes()) {
            Ok(out) => format!("ok {}", String::from_utf8_lossy(&out)),
            Err(error) => format!("error {error}"),
        }
    }

    /// JSON text as a value; lone surrogate escapes, which evanphx writes back as it read them,
    /// read as U+FFFD.
    fn parsed(json: &str) -> TestResult<Value> {
        Ok(serde_json::from_slice(&replace_lone_surrogates(
            json.as_bytes(),
        ))?)
    }

    /// A recorded merge: its inputs, the quirk Go's generator found and Go's output or error.
    struct Case<'a> {
        what: String,
        doc: &'a str,
        patch: &'a str,
        quirk: &'a str,
        go: String,
    }

    fn cases<'a>(doc: &'a Value, name: &str) -> TestResult<Vec<Case<'a>>> {
        list(doc, name)?
            .iter()
            .map(|case| {
                let err = text(case, "/err")?;
                Ok(Case {
                    what: format!(
                        "{} {} {}",
                        text(case, "/kind")?,
                        text(case, "/doc")?,
                        text(case, "/patch")?
                    ),
                    doc: text(case, "/doc")?,
                    patch: text(case, "/patch")?,
                    quirk: text(case, "/quirk")?,
                    go: if err.is_empty() {
                        format!("ok {}", text(case, "/out")?)
                    } else {
                        format!("error {err}")
                    },
                })
            })
            .collect()
    }

    /// RFC 7386's appendix and the edges no quirk touches give Go's bytes; every merge input of
    /// the corpora gives Go's value, its bytes differing at most in spelling
    /// (`json.merge-patch-spelling`); and no case of either meets a quirk
    /// (`json.merge-patch-quirks`).
    // Go: gopkg.in/evanphx/json-patch.v5 v5.9.0 merge.go (MergePatch), as
    // endpoints/openrtb2/auction.go:1711-1755, firstpartydata/first_party_data.go:255-384,
    // util/jsonutil/merge.go:165 and exchange/events.go:98 call it
    #[test]
    fn evanphx_goldens() -> TestResult {
        let doc = golden("merge_patch.json")?;
        let mut exact = 0;
        for case in cases(&doc, "/curated")? {
            if !case.quirk.is_empty()
                || ["spelling", "limits", "member-order"]
                    .iter()
                    .any(|kind| case.what.starts_with(kind))
            {
                continue;
            }
            ensure_eq(&result(case.doc, case.patch), &case.go, &case.what)?;
            exact += 1;
        }
        ensure_eq(&exact, &23, "curated cases compared byte for byte")?;
        let mut kinds = Vec::new();
        for case in cases(&doc, "/corpus")? {
            ensure_eq(&case.quirk, &"", &case.what)?;
            let go = case
                .go
                .strip_prefix("ok ")
                .ok_or_else(|| format!("{}: {}", case.what, case.go))?;
            let out = merge_patch(case.doc.as_bytes(), case.patch.as_bytes())
                .map_err(|e| format!("{}: {e}", case.what))?;
            ensure_eq(&parsed(&String::from_utf8(out)?)?, &parsed(go)?, &case.what)?;
            let kind = case.what.split(' ').next().unwrap_or_default().to_owned();
            if !kinds.contains(&kind) {
                kinds.push(kind);
            }
        }
        kinds.sort();
        ensure_eq(
            &kinds,
            &[
                "account",
                "event",
                "ext",
                "fpd",
                "stored-imp",
                "stored-request",
            ]
            .map(str::to_owned)
            .to_vec(),
            "corpus kinds",
        )
    }

    /// 50,000 seeded pairs of objects, arrays, nulls, numbers and strings outside the quirks give
    /// Go's bytes; the pairs inside them are classified alike on both sides.
    // Go: gopkg.in/evanphx/json-patch.v5 v5.9.0 merge.go; testdata/go/gen/mergepatch.go
    #[test]
    fn seeded_pairs_match_go() -> TestResult {
        let doc = golden("merge_patch.json")?;
        check_digests(&doc, "/seeded", "merge_patch", |rng| {
            let target = top(rng);
            let patch = top(rng);
            let quirk = quirk(&target, &patch)?;
            let outcome = if quirk.is_empty() {
                result(&target, &patch)
            } else {
                format!("quirk {quirk}")
            };
            Ok((format!("{target}\t{patch}"), outcome))
        })
    }

    /// The RFC 7386 result where evanphx meets a quirk, by (document, patch).
    const RFC: [(&str, &str, &str); 15] = [
        (r#"{"a":"foo"}"#, "null", "ok null"),
        (r#"{"a":"foo"}"#, r#""bar""#, r#"ok "bar""#),
        (r#"{"a":1}"#, "1", "ok 1"),
        ("[1]", r#""x""#, r#"ok "x""#),
        (r#"{"a":1}"#, "true", "ok true"),
        (r#""x""#, "null", "ok null"),
        ("null", r#"{"a":1}"#, r#"ok {"a":1}"#),
        ("null", "{}", "ok {}"),
        ("null", r#""x""#, r#"ok "x""#),
        ("null", "null", "ok null"),
        ("{}", r#"{"a":[{"b":null}]}"#, r#"ok {"a":[{"b":null}]}"#),
        (
            r#"{"a":[1]}"#,
            r#"{"a":[{"b":null}]}"#,
            r#"ok {"a":[{"b":null}]}"#,
        ),
        (
            r#"{"a":{"x":1}}"#,
            r#"{"a":[{"b":null}]}"#,
            r#"ok {"a":[{"b":null}]}"#,
        ),
        (
            r#"{"a":1}"#,
            r#"{"a":[null,{"b":null}]}"#,
            r#"ok {"a":[null,{"b":null}]}"#,
        ),
        (
            r#"{"a":1}"#,
            r#"[null,{"b":null}]"#,
            r#"ok [null,{"b":null}]"#,
        ),
    ];

    /// Every curated case of `quirk` gives the RFC's result here and Go's recorded one there, and
    /// at least one of them differs.
    fn check_quirk(quirk: &str, extra: &[(&str, &str, &str)]) -> TestResult {
        let doc = golden("merge_patch.json")?;
        let mut differ = 0;
        let mut seen = 0;
        for case in cases(&doc, "/curated")? {
            if case.quirk != quirk {
                continue;
            }
            let want = RFC
                .iter()
                .chain(extra)
                .find(|(d, p, _)| *d == case.doc && *p == case.patch)
                .map(|(_, _, want)| *want)
                .ok_or_else(|| format!("no RFC result for {}", case.what))?;
            let got = result(case.doc, case.patch);
            ensure_eq(&got.as_str(), &want, &case.what)?;
            if got != case.go {
                differ += 1;
            }
            seen += 1;
        }
        ensure_eq(
            &(seen > 0 && differ > 0),
            &true,
            &format!("{quirk}: {seen} cases, {differ} differ"),
        )
    }

    /// A null or scalar patch replaces the document (RFC 7386); evanphx rejects it (`Invalid JSON
    /// Patch`) when the document is not null (`json.merge-patch-scalar-patch`, decision-pending).
    // Go: gopkg.in/evanphx/json-patch.v5 v5.9.0 merge.go:131-133 and 141-149 (doMergePatch:
    // ErrBadJSONPatch)
    #[test]
    fn scalar_patch_replaces_document() -> TestResult {
        check_quirk("scalar-patch", &[])
    }

    /// A null document takes the patch (RFC 7386); evanphx rejects it (`Invalid JSON Document`)
    /// whatever the patch, before it looks at the patch's type (`json.merge-patch-null-document`,
    /// decision-pending).
    // Go: gopkg.in/evanphx/json-patch.v5 v5.9.0 merge.go:127-129 (doMergePatch: docErr == nil &&
    // *doc == nil)
    #[test]
    fn null_document_takes_patch() -> TestResult {
        check_quirk("null-document", &[])
    }

    /// The null members of objects inside a patch's arrays are kept (RFC 7386: an array replaces
    /// as it is); evanphx drops them where the array is new to the document, replaces a value
    /// that is not an object or is the patch itself (`json.merge-patch-array-nulls`).
    // Go: gopkg.in/evanphx/json-patch.v5 v5.9.0 merge.go:10-91 and 151 (merge, mergeDocs,
    // pruneNulls, pruneAryNulls)
    #[test]
    fn array_nulls_kept() -> TestResult {
        check_quirk(
            "array-nulls",
            &[(
                r#"{"a":1}"#,
                r#"{"a":{"b":null,"c":[{"d":null}]}}"#,
                r#"ok {"a":{"c":[{"d":null}]}}"#,
            )],
        )
    }

    /// Values are written as `serde_json` writes them: numbers in their shortest form, `<`, `>`
    /// and `&` as they are, escapes decoded and lone surrogates as U+FFFD; evanphx keeps each
    /// value's spelling and escapes HTML (`json.merge-patch-spelling`). The values agree.
    // Go: gopkg.in/evanphx/json-patch.v5 v5.9.0 merge.go (lazyNode keeps each value's bytes;
    // json.Marshal escapes HTML)
    #[test]
    fn spelling_rewritten() -> TestResult {
        let doc = golden("merge_patch.json")?;
        let mut seen = 0;
        for case in cases(&doc, "/curated")? {
            if !case.what.starts_with("spelling") {
                continue;
            }
            let got = result(case.doc, case.patch);
            ensure_eq(&(got == case.go), &false, &case.what)?;
            let go = case
                .go
                .strip_prefix("ok ")
                .ok_or_else(|| case.what.clone())?;
            let rust = got.strip_prefix("ok ").ok_or_else(|| case.what.clone())?;
            ensure_eq(&parsed(rust)?, &parsed(go)?, &case.what)?;
            seen += 1;
        }
        ensure_eq(&seen, &5, "spelling cases")
    }

    /// Every object is written with its members in key order and a repeated key's last value;
    /// evanphx writes a value it copies without merging into it as it was written, member order
    /// and repeated keys included (`json.merge-patch-member-order`). The values agree.
    // Go: gopkg.in/evanphx/json-patch.v5 v5.9.0 merge.go (lazyNode keeps a copied value's bytes)
    #[test]
    fn members_sorted() -> TestResult {
        let doc = golden("merge_patch.json")?;
        let (mut seen, mut differ) = (0, 0);
        for case in cases(&doc, "/curated")? {
            if !case.what.starts_with("member-order") {
                continue;
            }
            let got = result(case.doc, case.patch);
            let go = case
                .go
                .strip_prefix("ok ")
                .ok_or_else(|| case.what.clone())?;
            let rust = got.strip_prefix("ok ").ok_or_else(|| case.what.clone())?;
            ensure_eq(&parsed(rust)?, &parsed(go)?, &case.what)?;
            ensure_eq(&parsed(rust)?.to_string().as_str(), &rust, &case.what)?;
            if rust != go {
                differ += 1;
            }
            seen += 1;
        }
        ensure_eq(
            &(seen, differ),
            &(4, 2),
            "member-order cases, and those whose bytes differ",
        )
    }

    /// A number beyond `f64`'s range and a document nested deeper than 128 levels are rejected
    /// here; evanphx keeps the number's bytes and merges both (`json.merge-patch-limits`).
    // Go: gopkg.in/evanphx/json-patch.v5 v5.9.0 merge.go (lazyNode) and encoding/json's 10,000
    // level limit
    #[test]
    fn limits_rejected() -> TestResult {
        let doc = golden("merge_patch.json")?;
        let mut seen = Vec::new();
        for case in cases(&doc, "/curated")? {
            if !case.what.starts_with("limits") {
                continue;
            }
            ensure_eq(&case.go.starts_with("ok "), &true, &case.what)?;
            let got = merge_patch(case.doc.as_bytes(), case.patch.as_bytes());
            let want = if case.doc.len() > case.patch.len() || case.doc.contains("1e400") {
                MergePatchError::BadDoc
            } else {
                MergePatchError::BadPatch
            };
            ensure_eq(&got, &Err(want), &case.what)?;
            seen.push(want);
        }
        ensure_eq(&seen.len(), &4, "limit cases")
    }

    /// Go's `mpValue`.
    fn value(rng: &mut SplitMix64, depth: usize) -> String {
        let mut n = rng.below(10);
        if depth >= 3 && n >= 6 {
            n = rng.below(6);
        }
        match n {
            0 | 5 => "null".to_owned(),
            1 => "true".to_owned(),
            2 => rng.below(3).to_string(),
            3 => format!("\"{}\"", rng.pick(&KEYS)),
            4 => "1.5".to_owned(),
            6 | 7 => object(rng, depth + 1),
            _ => {
                let parts: Vec<String> = (0..rng.below(3)).map(|_| value(rng, depth + 1)).collect();
                format!("[{}]", parts.join(","))
            }
        }
    }

    /// Go's `mpObject`, its keys distinct and in key order.
    fn object(rng: &mut SplitMix64, depth: usize) -> String {
        let mut used = Vec::new();
        let mut parts = Vec::new();
        for _ in 0..rng.below(4) {
            let key = rng.pick(&KEYS);
            if used.contains(&key) {
                continue;
            }
            used.push(key);
            parts.push(format!("\"{key}\":{}", value(rng, depth)));
        }
        parts.sort();
        format!("{{{}}}", parts.join(","))
    }

    /// Go's `mpTop`.
    fn top(rng: &mut SplitMix64) -> String {
        if rng.below(10) == 0 {
            value(rng, 1)
        } else {
            object(rng, 1)
        }
    }

    /// Go's `mergePatchQuirk`, in the order evanphx checks the inputs.
    fn quirk(doc: &str, patch: &str) -> TestResult<&'static str> {
        let target: Value = serde_json::from_str(doc)?;
        let changes: Value = serde_json::from_str(patch)?;
        Ok(if target.is_null() {
            "null-document"
        } else if !changes.is_object() && !changes.is_array() {
            "scalar-patch"
        } else if null_in_array_object(&changes, false) {
            "array-nulls"
        } else {
            ""
        })
    }

    /// Go's `nullInArrayObject`.
    fn null_in_array_object(value: &Value, in_array: bool) -> bool {
        match value {
            Value::Object(members) => members.values().any(|member| {
                (member.is_null() && in_array) || null_in_array_object(member, in_array)
            }),
            Value::Array(elements) => elements
                .iter()
                .any(|element| null_in_array_object(element, true)),
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
        }
    }
}
