//! Glue (Rule B (d)): gojsonschema's `decodeJSONUsingNumber`, on `serde_json`.

use pbs_jsonutil::replace_lone_surrogates;
use serde::Deserialize as _;
use serde_json::{Deserializer, Value};

/// Go: gojsonschema `decodeJSONUsingNumber`: the first JSON value of `document`.
///
/// Go's `json.Decoder.Decode` reads one value and leaves what follows it, so `{}x` and `1 2`
/// decode as `{}` and `1`; this reads the value the same way. An unpaired surrogate escape
/// (`"\ud800"`) is U+FFFD, as Go decodes it (ruling 7, json.lone-surrogates).
///
/// # Errors
///
/// `serde_json`'s message when `document` does not start with a JSON value
/// (jsonschema.decode-errors): Go's decoder words its errors its own way.
pub fn decode_json_using_number(document: &[u8]) -> Result<Value, String> {
    let bytes = replace_lone_surrogates(document);
    Value::deserialize(&mut Deserializer::from_slice(&bytes)).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::decode_json_using_number;

    #[test]
    fn reads_the_first_value_like_go() -> Result<(), String> {
        let cases = [
            (&b"{}x"[..], json!({})),
            (b"1 2", json!(1)),
            (b"truex", json!(true)),
            (b"[1]]", json!([1])),
            (br#""\ud800""#, json!("\u{FFFD}")),
            (br#"{"a":"\udc00x"}"#, json!({"a": "\u{FFFD}x"})),
        ];
        for (document, want) in cases {
            let got = decode_json_using_number(document)?;
            if got != want {
                return Err(format!(
                    "{}: got {got}, want {want}",
                    String::from_utf8_lossy(document)
                ));
            }
        }
        for document in [&b""[..], b"malformedJSON", b"{", b"01", b"1e400"] {
            if decode_json_using_number(document).is_ok() {
                return Err(format!("{} decodes", String::from_utf8_lossy(document)));
            }
        }
        Ok(())
    }
}
