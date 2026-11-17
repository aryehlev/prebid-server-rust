//! Ports of the `util/jsonutil` Go tests that exercise the JSON codecs (`TestDropElement` covers a
//! function this crate does not port; the `MergeClone` tests are in `tests/mergeclone.rs`).

#[cfg(test)]
mod jsonutil_tests {
    use core::fmt::Debug;

    use pbs_gojson::{
        GoEncode as _, GoJson, RawJson,
        jsonutil::{self, IntString, StringInt},
    };

    fn check<T: PartialEq + Debug>(got: &T, want: &T, what: &str) -> Result<(), String> {
        if got == want {
            Ok(())
        } else {
            Err(format!("{what}: got {got:?}, want {want:?}"))
        }
    }

    /// The test-local `Item` of `TestStringIntUnmarshalJSON`.
    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "jsonutil.Item")]
    struct StringIntItem {
        #[gojson(go = "ItemId", json = "item_id")]
        item_id: StringInt,
    }

    /// The test-local `Item` of `TestIntStringUnmarshalJSON`.
    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "jsonutil.Item")]
    struct IntStringItem {
        #[gojson(go = "ItemId", json = "item_id")]
        item_id: IntString,
    }

    #[test]
    fn string_int_unmarshal_json() -> Result<(), String> {
        // Go: util/jsonutil/stringInt_test.go TestStringIntUnmarshalJSON
        let decode = |data: &[u8]| {
            let mut item = StringIntItem::default();
            let result = jsonutil::unmarshal_valid(data, &mut item);
            (
                item.item_id.0,
                result.map_err(|e| e.message.to_string_lossy()),
            )
        };
        check(&decode(br#"{"item_id":"30"}"#), &(30, Ok(())), "string")?;
        check(&decode(br#"{"item_id":30}"#), &(30, Ok(())), "int")?;
        check(&decode(br#"{"item_id": ""}"#).1, &Ok(()), "empty_id")?;
        check(
            &decode(br#"{"item_id":true}"#).1,
            &Err(
                "cannot unmarshal jsonutil.Item.ItemId: Value looks like Number/Boolean/None, \
                 but can't find its end: ',' or '}' symbol"
                    .to_owned(),
            ),
            "invalid_input",
        )
    }

    #[test]
    fn int_string_unmarshal_json() -> Result<(), String> {
        // Go: util/jsonutil/intstring_test.go TestIntStringUnmarshalJSON
        let cases: [(&str, &[u8], &str, bool); 4] = [
            ("null", br#"{"item_id": null}"#, "", true),
            ("string", br#"{"item_id": "30"}"#, "30", false),
            ("int", br#"{"item_id": 30}"#, "30", false),
            ("error", br#"{"item_id": []"#, "", true),
        ];
        for (name, data, want, expect_error) in cases {
            let mut item = IntStringItem::default();
            let result = jsonutil::unmarshal_valid(data, &mut item);
            check(&item.item_id.0.to_string_lossy().as_str(), &want, name)?;
            check(&result.is_err(), &expect_error, name)?;
        }
        Ok(())
    }

    #[test]
    fn raw_message_codec_encode() -> Result<(), String> {
        // Go: util/jsonutil/jsonutil_test.go TestEncode (rawMessageCodec)
        let nil = RawJson::nil();
        check(
            &jsonutil::marshal(&nil).map_err(|e| e.message),
            &Ok(Vec::new()),
            "Nil_pointer",
        )?;
        check(&nil.go_is_empty(), &true, "Nil_pointer IsEmpty")?;
        let blob = RawJson::from(
            r#"{
  "properties": {
    "string": "Blanks spaces in between words to not be removed if compacted",
    "integer": 5,
    "string_array": [
      "string array elem one",
      "string array elem two"
    ]
  }
}"#,
        );
        let compact = concat!(
            r#"{"properties":{"string":"Blanks spaces in between words to not be removed if "#,
            r#"compacted","integer":5,"string_array":["string array elem one","#,
            r#""string array elem two"]}}"#,
        );
        check(
            &jsonutil::marshal(&blob).map_err(|e| e.message),
            &Ok(compact.as_bytes().to_vec()),
            "json.RawMessage_compact_JSON",
        )?;
        check(
            &blob.go_is_empty(),
            &false,
            "json.RawMessage_compact_JSON IsEmpty",
        )
    }
}
