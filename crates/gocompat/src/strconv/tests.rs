//! Replays the Go goldens in `testdata/go/golden/strconv_*.txt`.

use super::*;
use crate::{
    golden::{self, SplitMix64, Sweep, err_text},
    testutil::ensure_eq,
};

const FORMATS: [u8; 8] = *b"eEfgGbxX";
const PRECS: [i64; 12] = [-1, 0, 1, 2, 3, 4, 5, 6, 10, 17, 20, 25];

fn parse_i64(s: &str) -> Result<i64, String> {
    s.parse().map_err(|e| format!("{s}: {e}"))
}

fn float_bits(s: &str) -> Result<f64, String> {
    u64::from_str_radix(s, 16)
        .map(f64::from_bits)
        .map_err(|e| format!("{s}: {e}"))
}

fn first_byte(s: &str) -> Result<u8, String> {
    s.bytes().next().ok_or_else(|| "empty format".to_owned())
}

fn pair_err_text<T>(err: Option<&NumError>, _: T) -> String {
    err.map_or_else(String::new, |e| format!("error: {e}"))
}

#[test]
fn format_float_rows_match_go() -> Result<(), String> {
    for row in golden::rows(include_str!(
        "../../testdata/go/golden/strconv_format_float.txt"
    ))? {
        let [bits, fmt, prec, bit_size, want] = golden::fields::<5>(&row)?;
        let got = format_float(
            float_bits(&bits)?,
            first_byte(&fmt)?,
            parse_i64(&prec)?,
            parse_i64(&bit_size)?,
        );
        ensure_eq(
            got.as_str(),
            want.as_str(),
            &format!("FormatFloat({bits}, {fmt}, {prec}, {bit_size})"),
        )?;
    }
    Ok(())
}

#[test]
fn format_float_matrix_matches_go() -> Result<(), String> {
    let values = golden::rows(include_str!(
        "../../testdata/go/golden/strconv_float_values.txt"
    ))?;
    let mut sweep = Sweep::default();
    for row in &values {
        let [bits] = golden::fields::<1>(row)?;
        let v = float_bits(&bits)?;
        for bit_size in [32, 64] {
            for fmt in FORMATS {
                for prec in PRECS {
                    sweep.add(&format_float(v, fmt, prec, bit_size));
                }
            }
        }
    }
    let sums = include_str!("../../testdata/go/golden/strconv_sweeps.txt");
    ensure_eq(
        sweep.sum().as_str(),
        golden::checksum(sums, "format_float_matrix")?.as_str(),
        "matrix",
    )
}

#[test]
fn format_float_sweeps_match_go() -> Result<(), String> {
    let sums = include_str!("../../testdata/go/golden/strconv_sweeps.txt");
    let mut rng = SplitMix64(1);
    let mut bits_sweep = Sweep::default();
    for _ in 0..100_000 {
        let v = f64::from_bits(rng.next_u64());
        for fmt in *b"efg" {
            for prec in [-1, 0, 3, 17] {
                if fmt == b'f' && v.abs() > 1e30 {
                    continue;
                }
                bits_sweep.add(&format_float(v, fmt, prec, 64));
            }
        }
        bits_sweep.add(&format_float(v, b'g', -1, 32));
    }
    ensure_eq(
        bits_sweep.sum().as_str(),
        golden::checksum(sums, "format_float_bits")?.as_str(),
        "bits",
    )?;

    let mut price_sweep = Sweep::default();
    for _ in 0..100_000 {
        let cents = u32::try_from(rng.next_u64() % 10_000_000).map_err(|e| e.to_string())?;
        let v = f64::from(cents) / 1000.0;
        for prec in [-1, 0, 1, 2, 3, 4] {
            price_sweep.add(&format_float(v, b'f', prec, 64));
        }
        price_sweep.add(&format_float(v * 1.1, b'f', -1, 64));
        price_sweep.add(&format_float(v * 1.1, b'g', 3, 64));
    }
    ensure_eq(
        price_sweep.sum().as_str(),
        golden::checksum(sums, "format_float_prices")?.as_str(),
        "prices",
    )?;

    let mut parse_sweep = Sweep::default();
    for _ in 0..100_000 {
        let v = f64::from_bits(rng.next_u64());
        for s in [
            format_float(v, b'g', -1, 64),
            format_float(v, b'e', 25, 64),
            format_float(v, b'g', 8, 64),
        ] {
            let (f64v, e64) = parse_float_pair(&s, 64);
            let (f32v, e32) = parse_float_pair(&s, 32);
            parse_sweep.add(&format!(
                "{:016x} {} {:016x} {}",
                f64v.to_bits(),
                pair_err_text(e64.as_ref(), ()),
                f32v.to_bits(),
                pair_err_text(e32.as_ref(), ())
            ));
        }
    }
    ensure_eq(
        parse_sweep.sum().as_str(),
        golden::checksum(sums, "parse_float")?.as_str(),
        "parse",
    )
}

#[test]
fn parse_float_rows_match_go() -> Result<(), String> {
    for row in golden::rows(include_str!(
        "../../testdata/go/golden/strconv_parse_float.txt"
    ))? {
        let [input, bit_size, want_bits, want_err] = golden::fields::<4>(&row)?;
        let (v, err) = parse_float_pair(&input, parse_i64(&bit_size)?);
        let what = format!("ParseFloat({input:?}, {bit_size})");
        ensure_eq(
            format!("{:016x}", v.to_bits()).as_str(),
            want_bits.as_str(),
            &what,
        )?;
        ensure_eq(
            pair_err_text(err.as_ref(), ()).as_str(),
            want_err.as_str(),
            &what,
        )?;
    }
    Ok(())
}

fn int_fields(input: &str, base: i64, bits: i64) -> [String; 7] {
    let (iv, ierr) = parse_int_pair(input, base, bits);
    let (uv, uerr) = parse_uint_pair(input, base, bits);
    [
        input.to_owned(),
        base.to_string(),
        bits.to_string(),
        iv.to_string(),
        pair_err_text(ierr.as_ref(), ()),
        uv.to_string(),
        pair_err_text(uerr.as_ref(), ()),
    ]
}

#[test]
fn parse_int_rows_match_go() -> Result<(), String> {
    for row in golden::rows(include_str!(
        "../../testdata/go/golden/strconv_parse_int.txt"
    ))? {
        let fields = golden::fields::<7>(&row)?;
        let [input, base, bits, ..] = &fields;
        if base == "atoi" {
            let (v, err) = atoi_pair(input);
            let got = [
                input.clone(),
                base.clone(),
                String::new(),
                v.to_string(),
                pair_err_text(err.as_ref(), ()),
                String::new(),
                String::new(),
            ];
            ensure_eq(&got, &fields, &format!("Atoi({input:?})"))?;
            continue;
        }
        let got = int_fields(input, parse_i64(base)?, parse_i64(bits)?);
        ensure_eq(
            &got,
            &fields,
            &format!("ParseInt({input:?}, {base}, {bits})"),
        )?;
    }
    Ok(())
}

#[test]
fn parse_int_matrix_matches_go() -> Result<(), String> {
    let inputs = golden::rows(include_str!(
        "../../testdata/go/golden/strconv_int_inputs.txt"
    ))?;
    let mut sweep = Sweep::default();
    for row in &inputs {
        let [input] = golden::fields::<1>(row)?;
        for base in [0, 2, 8, 10, 16, 36, 1, 37, -1] {
            for bits in [0, 8, 16, 32, 64, 65, -1] {
                sweep.add(&int_fields(&input, base, bits).join("|"));
            }
        }
    }
    let sums = include_str!("../../testdata/go/golden/strconv_parse_int_matrix.txt");
    ensure_eq(
        sweep.sum().as_str(),
        golden::checksum(sums, "parse_int_matrix")?.as_str(),
        "matrix",
    )
}

#[test]
fn format_int_rows_match_go() -> Result<(), String> {
    for row in golden::rows(include_str!(
        "../../testdata/go/golden/strconv_format_int.txt"
    ))? {
        let [input, base, signed, unsigned] = golden::fields::<4>(&row)?;
        let v = parse_i64(&input)?;
        let base = u32::try_from(parse_i64(&base)?).map_err(|e| e.to_string())?;
        ensure_eq(format_int(v, base).as_str(), signed.as_str(), "FormatInt")?;
        ensure_eq(
            format_uint(v.cast_unsigned(), base).as_str(),
            unsigned.as_str(),
            "FormatUint",
        )?;
    }
    ensure_eq(itoa(-42).as_str(), "-42", "Itoa")
}

#[test]
fn parse_bool_rows_match_go() -> Result<(), String> {
    for row in golden::rows(include_str!(
        "../../testdata/go/golden/strconv_parse_bool.txt"
    ))? {
        let [input, want, want_err] = golden::fields::<3>(&row)?;
        let got = parse_bool(&input);
        ensure_eq(
            format_bool(*got.as_ref().unwrap_or(&false)),
            want.as_str(),
            &input,
        )?;
        ensure_eq(err_text(&got).as_str(), want_err.as_str(), &input)?;
    }
    Ok(())
}

#[test]
fn quote_rows_match_go() -> Result<(), String> {
    for row in golden::rows_bytes(include_str!("../../testdata/go/golden/strconv_quote.txt"))? {
        let [input, q, ascii, graphic, kind] =
            <[Vec<u8>; 5]>::try_from(row).map_err(|r| format!("{r:?}"))?;
        let text = |b: &[u8]| String::from_utf8(b.to_vec()).map_err(|e| e.to_string());
        if kind == b"rune" {
            let r = text(&input)?.chars().next().ok_or("empty rune")?;
            ensure_eq(quote_rune(r).as_bytes(), q.as_slice(), "QuoteRune")?;
            ensure_eq(
                quote_rune_to_ascii(r).as_bytes(),
                ascii.as_slice(),
                "QuoteRuneToASCII",
            )?;
            ensure_eq(
                quote_rune_to_graphic(r).as_bytes(),
                graphic.as_slice(),
                "QuoteRuneToGraphic",
            )?;
            continue;
        }
        ensure_eq(quote_bytes(&input).as_bytes(), q.as_slice(), "Quote")?;
        if let Ok(s) = str::from_utf8(&input) {
            ensure_eq(quote(s).as_bytes(), q.as_slice(), "Quote str")?;
            ensure_eq(
                quote_to_ascii(s).as_bytes(),
                ascii.as_slice(),
                "QuoteToASCII",
            )?;
            ensure_eq(
                quote_to_graphic(s).as_bytes(),
                graphic.as_slice(),
                "QuoteToGraphic",
            )?;
            ensure_eq(
                can_backquote(s).to_string().as_bytes(),
                kind.as_slice(),
                "CanBackquote",
            )?;
        }
    }
    Ok(())
}

#[test]
fn quote_every_rune_matches_go() -> Result<(), String> {
    let mut sweep = Sweep::default();
    for c in (0..=0x0010_FFFF_u32).filter_map(char::from_u32) {
        let s = c.to_string();
        sweep.add(&format!(
            "{}{}{}",
            quote(&s),
            quote_to_ascii(&s),
            quote_to_graphic(&s)
        ));
    }
    let sums = include_str!("../../testdata/go/golden/strconv_quote_sweep.txt");
    ensure_eq(
        sweep.sum().as_str(),
        golden::checksum(sums, "quote_all_runes")?.as_str(),
        "runes",
    )
}

#[test]
fn unquote_rows_match_go() -> Result<(), String> {
    for row in golden::rows_bytes(include_str!("../../testdata/go/golden/strconv_unquote.txt"))? {
        let [input, want, want_err] =
            <[Vec<u8>; 3]>::try_from(row).map_err(|r| format!("{r:?}"))?;
        let input = String::from_utf8(input).map_err(|e| e.to_string())?;
        let got = unquote_bytes(&input);
        ensure_eq(got.as_deref().unwrap_or_default(), want.as_slice(), &input)?;
        ensure_eq(err_text(&got).as_bytes(), want_err.as_slice(), &input)?;
    }
    Ok(())
}

#[test]
fn num_error_text() {
    let err = parse_int("12a", 10, 64).err().map(|e| e.to_string());
    assert_eq!(
        err.as_deref(),
        Some(r#"strconv.ParseInt: parsing "12a": invalid syntax"#),
        "text"
    );
    let range = atoi("99999999999999999999").err().map(|e| e.to_string());
    assert_eq!(
        range.as_deref(),
        Some(r#"strconv.Atoi: parsing "99999999999999999999": value out of range"#),
        "range"
    );
}
