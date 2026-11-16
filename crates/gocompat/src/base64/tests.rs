//! Replays `base64.txt` and `base64_sweeps.txt` from `testdata/go/golden/base64.go`.

use super::*;
use crate::{
    golden::{self, SplitMix64, Sweep},
    strconv::quote_bytes,
    testutil::ensure_eq,
};

const ENCODINGS: [(&str, Encoding); 4] = [
    ("std", STD_ENCODING),
    ("url", URL_ENCODING),
    ("rawstd", RAW_STD_ENCODING),
    ("rawurl", RAW_URL_ENCODING),
];

fn encoding(name: &[u8]) -> Result<Encoding, String> {
    ENCODINGS
        .iter()
        .find(|(n, _)| n.as_bytes() == name)
        .map(|&(_, e)| e)
        .ok_or_else(|| format!("unknown encoding {name:?}"))
}

/// Go: `errText` of a decode error.
fn err_text(err: Option<CorruptInputError>) -> String {
    err.map_or_else(String::new, |e| format!("error: {e}"))
}

#[test]
fn rows_match_go() -> Result<(), String> {
    let rows = golden::rows_bytes(include_str!("../../testdata/go/golden/base64.txt"))?;
    for row in &rows {
        match row.as_slice() {
            [kind, name, input, decoded, err] if kind == b"decode" => {
                let enc = encoding(name)?;
                let text = str::from_utf8(input).map_err(|e| e.to_string())?;
                let (got, got_err) = enc.decode_string_pair(text);
                let what = format!("{text:?}");
                ensure_eq(got.as_slice(), decoded.as_slice(), &what)?;
                ensure_eq(err_text(got_err).as_bytes(), err.as_slice(), &what)?;
            }
            [kind, name, input, encoded] if kind == b"encode" => {
                let enc = encoding(name)?;
                ensure_eq(
                    enc.encode_to_string(input).as_bytes(),
                    encoded.as_slice(),
                    "encode",
                )?;
            }
            other => return Err(format!("unknown row {other:?}")),
        }
    }
    ensure_eq(&rows.len(), &(4 * (36 + 9)), "row count")
}

const ALPHABET: &[u8] = b"ABZaz09+/-_=\n\r!. ";

#[test]
fn random_sweep_matches_go() -> Result<(), String> {
    let mut rng = SplitMix64(3);
    let mut sweep = Sweep::default();
    let len = u64::try_from(ALPHABET.len()).map_err(|e| e.to_string())?;
    for _ in 0..50_000 {
        let n = rng.next_u64() % 24;
        let mut raw = Vec::new();
        let mut text = String::new();
        for _ in 0..n {
            raw.push(rng.next_u64().to_le_bytes().first().copied().unwrap_or(0));
            let pick = usize::try_from(rng.next_u64() % len).map_err(|e| e.to_string())?;
            text.push(char::from(ALPHABET.get(pick).copied().unwrap_or(b'A')));
        }
        for (_, enc) in ENCODINGS {
            let (decoded, err) = enc.decode_string_pair(&text);
            sweep.add(&format!(
                "{}\t{}\t{}",
                enc.encode_to_string(&raw),
                quote_bytes(&decoded),
                err_text(err)
            ));
        }
    }
    let sums = include_str!("../../testdata/go/golden/base64_sweeps.txt");
    ensure_eq(
        sweep.sum().as_str(),
        golden::checksum(sums, "base64_random")?.as_str(),
        "random",
    )
}

#[test]
fn decode_string_drops_partial_output_on_error() -> Result<(), String> {
    ensure_eq(
        &URL_ENCODING.decode_string("Zm9v"),
        &Ok(b"foo".to_vec()),
        "ok",
    )?;
    ensure_eq(
        &STD_ENCODING.decode_string("Zg=").map_err(|e| e.to_string()),
        &Err("illegal base64 data at input byte 3".to_owned()),
        "short padding",
    )
}
