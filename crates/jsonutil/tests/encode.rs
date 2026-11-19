//! Writing against Go (`testdata/golden/decode.json`): `json.RawMessage` compacted as Prebid's
//! `RawMessageExtension` writes it, in a field and outside one, nil raw values, integer map keys,
//! and edits of a document's members.

#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pbs_jsonutil::{compact, marshal, marshal_raw, raw_compact, unmarshal};
    use serde::{Deserialize, Serialize};
    use serde_json::{Value, value::RawValue};

    use super::support::{TestResult, ensure_eq, golden, list, text};

    /// Go's `rawOmit`, written through `raw_compact`.
    #[derive(Default, Serialize, Deserialize)]
    #[serde(default)]
    struct RawOmit {
        #[serde(
            skip_serializing_if = "Option::is_none",
            serialize_with = "raw_compact"
        )]
        ext: Option<Box<RawValue>>,
    }

    /// Go's `rawPlain`, written through `raw_compact`.
    #[derive(Default, Serialize)]
    struct RawPlain {
        #[serde(serialize_with = "raw_compact")]
        ext: Option<Box<RawValue>>,
    }

    /// `rawOmit` without `raw_compact`: the `RawValue` as `serde_json` writes it.
    #[derive(Default, Serialize, Deserialize)]
    #[serde(default)]
    struct RawVerbatim {
        #[serde(skip_serializing_if = "Option::is_none")]
        ext: Option<Box<RawValue>>,
    }

    /// `marshal`'s output as text.
    fn written<T: Serialize + ?Sized>(value: &T) -> TestResult<String> {
        Ok(String::from_utf8(marshal(value)?)?)
    }

    /// The `(input, result)` of each row of the section `name`.
    fn rows<'a>(doc: &'a Value, name: &str) -> TestResult<Vec<(&'a str, &'a str)>> {
        list(doc, name)?
            .iter()
            .map(|row| Ok((text(row, "/input")?, text(row, "/result")?)))
            .collect()
    }

    /// `TestCreateEncoder` and `TestEncode`: a `json.RawMessage` is written compacted, its
    /// whitespace between tokens gone and its spellings, escapes and HTML characters kept, as
    /// `jsonutil.Marshal` writes it: through `raw_compact` by any writer, and by `marshal` without
    /// it. `serde_json`'s own writer keeps a `RawValue`'s whitespace, which is what `raw_compact`
    /// is for (`json.raw-not-compacted`, restored).
    // Go: util/jsonutil/jsonutil.go:224-255 (RawMessageExtension), jsonutil_test.go:249-324
    #[test]
    fn raw_messages_compacted() -> TestResult {
        let doc = golden("decode.json")?;
        let mut compared = 0;
        for (input, go) in rows(&doc, "/raw_compact")? {
            if input.contains("\\ud800") {
                continue;
            }
            let omit: RawOmit = unmarshal(input.as_bytes())?;
            ensure_eq(&written(&omit)?.as_str(), &go, input)?;
            ensure_eq(&serde_json::to_string(&omit)?.as_str(), &go, input)?;
            let verbatim: RawVerbatim = unmarshal(input.as_bytes())?;
            ensure_eq(&written(&verbatim)?.as_str(), &go, input)?;
            let raw = verbatim.ext.as_deref().map(RawValue::get).ok_or("no ext")?;
            ensure_eq(
                &serde_json::to_string(&verbatim)?,
                &format!("{{\"ext\":{raw}}}"),
                input,
            )?;
            compared += 1;
        }
        ensure_eq(&compared, &4, "inputs")?;
        let blob = rows(&doc, "/raw_compact")?
            .into_iter()
            .find(|(input, _)| input.contains("properties"))
            .ok_or("no TestEncode row")?;
        let inner = blob
            .0
            .strip_prefix("{\"ext\":")
            .and_then(|rest| rest.strip_suffix('}'));
        let want = blob
            .1
            .strip_prefix("{\"ext\":")
            .and_then(|rest| rest.strip_suffix('}'));
        ensure_eq(
            &inner.map(compact).as_deref(),
            &want,
            "TestEncode's document",
        )
    }

    /// Outside a struct field too, `marshal` writes every `json.RawMessage` compacted, as
    /// `jsonutil.Marshal` does: a map value, a slice element, the value itself, and the values of a
    /// decoded `map[string]json.RawMessage`, which is how `RequestWrapper` writes its ext maps and
    /// the exchange an imp's sanitized ext; `marshal_raw` holds the same bytes.
    // Go: util/jsonutil/jsonutil.go:224-255 (RawMessageExtension); gen/decode.go rawContainers
    #[test]
    fn raw_containers_compacted() -> TestResult {
        let doc = golden("decode.json")?;
        let rows = rows(&doc, "/raw_containers")?;
        let [map, slice, top, decoded] = rows.as_slice() else {
            return Err(format!("want four rows, got {}", rows.len()).into());
        };
        let raw = RawValue::from_string(map.0.to_owned())?;
        let rust = [
            written(&BTreeMap::from([("k", raw.clone())]))?,
            written(&vec![
                raw.clone(),
                RawValue::from_string(" [ 1 ] ".to_owned())?,
            ])?,
            written(&raw)?,
            written(&unmarshal::<BTreeMap<String, Box<RawValue>>>(
                decoded.0.as_bytes(),
            )?)?,
        ];
        let go = [map.1, slice.1, top.1, decoded.1];
        ensure_eq(
            &rust.as_slice(),
            &go.map(str::to_owned).as_slice(),
            "marshal",
        )?;
        ensure_eq(
            &marshal_raw(&raw)?.get(),
            &top.1,
            "marshal_raw of the value itself",
        )?;
        ensure_eq(
            &serde_json::to_string(&raw)?.as_str(),
            &top.0,
            "serde_json's own writer",
        )?;
        ensure_eq(&slice.0, &top.0, "the inputs")
    }

    /// A nil `json.RawMessage` that `omitempty` does not drop is written as `null`, as a field, a
    /// map value, a slice element and on its own; Go's `RawMessageExtension` writes nothing there,
    /// which is not JSON (`json.nil-raw-written`). Under `omitempty` both drop it.
    // Go: util/jsonutil/jsonutil.go:242-255 (rawMessageCodec.Encode, IsEmpty); gen/decode.go nilRaw
    #[test]
    fn nil_raw_written_as_null() -> TestResult {
        let doc = golden("decode.json")?;
        let nothing: Option<Box<RawValue>> = None;
        let map = BTreeMap::from([("a", nothing.clone())]);
        let slice = vec![
            nothing.clone(),
            Some(RawValue::from_string(r#"{"x":1}"#.to_owned())?),
        ];
        let rust = [
            written(&RawPlain::default())?,
            written(&map)?,
            written(&slice)?,
            written(&nothing)?,
            written(&RawOmit::default())?,
        ];
        let go: Vec<&str> = list(&doc, "/nil_raw")?
            .iter()
            .map(|row| text(row, "/result"))
            .collect::<TestResult<_>>()?;
        ensure_eq(
            &go,
            &vec![r#"{"ext":}"#, r#"{"a":}"#, r#"[,{"x":1}]"#, "", "{}"],
            "Go's writes",
        )?;
        ensure_eq(
            &rust.to_vec(),
            &[
                r#"{"ext":null}"#,
                r#"{"a":null}"#,
                r#"[null,{"x":1}]"#,
                "null",
                "{}",
            ]
            .map(str::to_owned)
            .to_vec(),
            "the port's",
        )
    }

    /// A map with integer keys is written in numeric key order (`BTreeMap`); Go sorts the keys as
    /// strings, so `"10"` comes before `"9"` (`json.int-map-key-order`). The members agree.
    // Go: jsoniter's map encoder (sortKeys under ConfigCompatibleWithStandardLibrary);
    // gen/decode.go intMapKeyOrder
    #[test]
    fn int_keys_numeric_order() -> TestResult {
        let doc = golden("decode.json")?;
        let (_, go) = rows(&doc, "/int_map_key_order")?
            .into_iter()
            .next()
            .ok_or("no row")?;
        let map = BTreeMap::from([(10, "a"), (9, "b"), (-1, "c"), (100, "d")]);
        let rust = written(&map)?;
        ensure_eq(
            &rust.as_str(),
            &r#"{"-1":"c","9":"b","10":"a","100":"d"}"#,
            "the port",
        )?;
        ensure_eq(&go, &r#"{"-1":"c","10":"a","100":"d","9":"b"}"#, "Go")?;
        ensure_eq(
            &serde_json::from_str::<Value>(&rust)?,
            &serde_json::from_str::<Value>(go)?,
            "the members",
        )
    }

    /// A member set in a document through a `serde_json::Value` puts every member in key order;
    /// Go's sjson and jsonparser edit the bytes in place and keep the document's order
    /// (`json.member-order`). The members agree.
    // Go: github.com/tidwall/sjson SetBytes, github.com/buger/jsonparser Set; gen/decode.go
    // memberOrder
    #[test]
    fn value_edits_sort_members() -> TestResult {
        let doc = golden("decode.json")?;
        let rows = rows(&doc, "/member_order")?;
        let edits = [("c", 3), ("c", 3), ("b", 5)];
        ensure_eq(&rows.len(), &edits.len(), "edits")?;
        for ((input, go), (key, value)) in rows.into_iter().zip(edits) {
            let mut edited: Value = serde_json::from_str(input)?;
            let _previous = edited
                .as_object_mut()
                .ok_or("not an object")?
                .insert(key.to_owned(), value.into());
            let rust = edited.to_string();
            ensure_eq(&(rust == go), &false, input)?;
            ensure_eq(&serde_json::from_str::<Value>(go)?, &edited, input)?;
            ensure_eq(
                &serde_json::from_str::<Value>(&rust)?.to_string(),
                &rust,
                input,
            )?;
        }
        Ok(())
    }
}
