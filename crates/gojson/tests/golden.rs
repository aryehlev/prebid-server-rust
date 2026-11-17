//! Differential replay of the goldens that `testdata/go` writes with Go 1.26.2 and
//! json-iterator 1.1.12: every output and error must match Go byte for byte.
//!
//! Regenerate the goldens from the repository root with `go run ./rust/crates/gojson/testdata/go`.

#[cfg(test)]
mod golden {
    use core::str::from_utf8;
    use std::{collections::BTreeMap, fmt::Write as _, fs, thread};

    use pbs_gojson::{
        GoEncode, GoJson, GoMarshaler, GoString, GoUnmarshaler, JsonError, JsonNumber,
        JsoniterDecode, RawJson, StdDecode,
        gotype::go_type_name,
        jsoniter,
        jsonutil::{self, IntString, StringInt},
        stdjson,
    };
    use serde_json::Value;

    // ---- golden file support -------------------------------------------------------------

    /// Go's `strconv.QuoteToASCII`.
    fn quote_to_ascii(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::from("\"");
        let hex = |buf: &mut String, b: u8| {
            buf.push_str("\\x");
            buf.push(char::from(
                HEX.get(usize::from(b >> 4)).copied().unwrap_or(b'0'),
            ));
            buf.push(char::from(
                HEX.get(usize::from(b & 0xF)).copied().unwrap_or(b'0'),
            ));
        };
        for chunk in bytes.utf8_chunks() {
            for c in chunk.valid().chars() {
                match c {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\u{7}' => out.push_str("\\a"),
                    '\u{8}' => out.push_str("\\b"),
                    '\u{c}' => out.push_str("\\f"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    '\u{b}' => out.push_str("\\v"),
                    ' '..='~' => out.push(c),
                    '\0'..='\u{1f}' | '\u{7f}' => hex(&mut out, u8::try_from(c).unwrap_or(0)),
                    '\u{80}'..='\u{ffff}' => {
                        let _written = write!(out, "\\u{:04x}", u32::from(c));
                    }
                    '\u{10000}'.. => {
                        let _written = write!(out, "\\U{:08x}", u32::from(c));
                    }
                }
            }
            for &b in chunk.invalid() {
                hex(&mut out, b);
            }
        }
        out.push('"');
        out
    }

    fn hex_value(digits: &[u8]) -> Option<u32> {
        let text = from_utf8(digits).ok()?;
        u32::from_str_radix(text, 16).ok()
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
            let value = hex_value(digits).ok_or_else(|| format!("bad hex in {field}"))?;
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

    /// The rows of a golden file (comment lines skipped), each a list of quoted fields.
    fn rows(name: &str) -> Result<Vec<Vec<String>>, String> {
        let path = format!("{}/testdata/golden/{name}", env!("CARGO_MANIFEST_DIR"));
        let text = fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
        Ok(text
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .map(|line| line.split('\t').map(str::to_owned).collect())
            .collect())
    }

    fn field(row: &[String], index: usize) -> Result<Vec<u8>, String> {
        unquote(row.get(index).ok_or_else(|| format!("short row {row:?}"))?)
    }

    /// Compares Rust's outputs with the quoted Go outputs starting at column `first`.
    fn compare(
        failures: &mut Vec<String>,
        what: &str,
        row: &[String],
        first: usize,
        names: &[&str],
        actual: &[Vec<u8>],
    ) {
        for (i, (name, got)) in names.iter().zip(actual).enumerate() {
            let want = row.get(first + i).map_or("<missing>", String::as_str);
            let got = quote_to_ascii(got);
            if got != want {
                failures.push(format!(
                    "{what}\n    {name}:\n      go:   {want}\n      rust: {got}"
                ));
            }
        }
    }

    fn finish(kind: &str, total: usize, failures: &[String]) -> Result<(), String> {
        if failures.is_empty() {
            return Ok(());
        }
        let mut report = format!(
            "{kind}: {} of {total} outputs differ from Go\n",
            failures.len()
        );
        for failure in failures.iter().take(60) {
            report.push_str(failure);
            report.push('\n');
        }
        Err(report)
    }

    fn err_text<E, F: Fn(E) -> Vec<u8>>(result: Result<(), E>, text: F) -> Vec<u8> {
        result.err().map_or_else(Vec::new, text)
    }

    fn output(result: Result<Vec<u8>, Vec<u8>>) -> Vec<u8> {
        result.unwrap_or_else(|err| {
            let mut out = b"!".to_vec();
            out.extend_from_slice(&err);
            out
        })
    }

    /// encoding/json's rendering of a decoded value: the Go generator marshals the pointer it
    /// decoded into (`*T`).
    fn value_text<T: GoEncode + Clone>(v: &T) -> Vec<u8> {
        output(stdjson::marshal(&Some(v.clone())).map_err(|e| e.go_text()))
    }

    // ---- Go types of the generator (types.go) ---------------------------------------------

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Small")]
    struct Small {
        #[gojson(go = "ID", json = "id")]
        id: GoString,
        #[gojson(json = "w,omitempty")]
        w: Option<i64>,
        #[gojson(json = "h,omitempty")]
        h: i64,
        #[gojson(json = "price")]
        price: f64,
        #[gojson(json = "tags,omitempty")]
        tags: Option<Vec<GoString>>,
        #[gojson(json = "ext,omitempty")]
        ext: RawJson,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Wide")]
    struct Wide {
        #[gojson(json = "a")]
        a: GoString,
        #[gojson(json = "b")]
        b: isize,
        #[gojson(json = "c")]
        c: u8,
        #[gojson(json = "d")]
        d: i8,
        #[gojson(json = "e")]
        e: f32,
        #[gojson(json = "f")]
        f: bool,
        #[gojson(json = "g")]
        g: Option<GoString>,
        #[gojson(json = "h")]
        h: Option<Vec<isize>>,
        #[gojson(json = "i")]
        i: Option<BTreeMap<GoString, isize>>,
        #[gojson(json = "j")]
        j: Value,
        #[gojson(json = "k")]
        k: Option<Vec<Small>>,
        #[gojson(json = "l")]
        l: Option<Box<Small>>,
        #[gojson(json = "m")]
        m: RawJson,
        #[gojson(json = "n")]
        n: Option<Vec<u8>>,
        #[gojson(json = "o")]
        o: Option<BTreeMap<i64, GoString>>,
        #[gojson(json = "p")]
        p: Option<RawJson>,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Upper")]
    struct Upper {
        #[gojson(go = "ID", json = "ID")]
        id: GoString,
        name: String,
        #[gojson(json = "count,omitempty")]
        count: isize,
        #[gojson(go = "internal")]
        internal: isize,
        #[gojson(json = "-")]
        skipped: GoString,
        #[gojson(json = "-,")]
        dash: GoString,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Base")]
    struct Base {
        #[gojson(go = "ID", json = "id")]
        id: GoString,
        #[gojson(json = "name")]
        name: GoString,
        #[gojson(json = "dup")]
        dup: isize,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Deep")]
    struct Deep {
        #[gojson(json = "level")]
        level: isize,
        #[gojson(json = "name")]
        name: GoString,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Extra")]
    struct Extra {
        #[gojson(json = "dup")]
        dup: isize,
        #[gojson(json = "note,omitempty")]
        note: GoString,
        #[gojson(embedded)]
        deep: Deep,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Embeds")]
    struct Embeds {
        #[gojson(embedded)]
        base: Base,
        #[gojson(embedded)]
        extra: Option<Box<Extra>>,
        #[gojson(json = "name")]
        name: GoString,
        #[gojson(json = "own")]
        own: isize,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Quoted")]
    struct Quoted {
        #[gojson(json = ",string")]
        n: isize,
        #[gojson(json = "s,string")]
        s: GoString,
        #[gojson(json = "p,string")]
        p: Option<isize>,
        #[gojson(json = "b,string")]
        b: bool,
        #[gojson(json = "f,string,omitempty")]
        f: f64,
        #[gojson(json = "u,string")]
        u: u16,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Validated", unmarshaler)]
    struct Validated {
        n: isize,
    }

    impl GoUnmarshaler for Validated {
        fn unmarshal_json(&mut self, data: &[u8]) -> Result<(), JsonError> {
            // Go: strconv.Atoi
            let n = from_utf8(data)
                .ok()
                .and_then(|s| s.parse::<isize>().ok())
                .ok_or_else(|| JsonError::msg("validated value must be an integer"))?;
            if n > 100 {
                return Err(JsonError::msg("request.v must be at most 100"));
            }
            self.n = n;
            Ok(())
        }
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Custom")]
    struct Custom {
        #[gojson(go = "SI", json = "si")]
        si: StringInt,
        #[gojson(go = "IS", json = "is")]
        is: IntString,
        #[gojson(go = "V", json = "v")]
        v: Validated,
        #[gojson(go = "PV", json = "pv")]
        pv: Option<Validated>,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Shout", marshaler)]
    struct Shout(GoString);

    impl GoMarshaler for Shout {
        fn marshal_json(&self) -> Result<Vec<u8>, JsonError> {
            match self.0.as_bytes() {
                b"" => Err(JsonError::msg("empty shout")),
                b"bad" => Ok(b"{bad".to_vec()),
                s => {
                    let mut out = b" \"".to_vec();
                    out.extend(s.to_ascii_uppercase());
                    out.extend_from_slice(b"\"\n");
                    Ok(out)
                }
            }
        }
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Loud")]
    struct Loud {
        #[gojson(json = "s")]
        s: Shout,
        #[gojson(json = "t,omitempty")]
        t: Shout,
        #[gojson(json = "p,omitempty")]
        p: Option<Shout>,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Maps")]
    struct Maps {
        #[gojson(json = "smalls")]
        smalls: Option<BTreeMap<GoString, Small>>,
        #[gojson(json = "ptrs")]
        ptrs: Option<BTreeMap<GoString, Option<isize>>>,
        #[gojson(json = "bools")]
        bools: Option<BTreeMap<u8, bool>>,
        #[gojson(json = "raw")]
        raw: Option<BTreeMap<GoString, RawJson>>,
    }

    #[derive(Clone, Copy, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Empty")]
    struct Empty;

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.OnlyPrivate")]
    struct OnlyPrivate {
        #[gojson(go = "a")]
        a: isize,
        #[gojson(go = "b")]
        b: GoString,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, GoJson)]
    #[gojson(go_type = "main.Key", map_key)]
    struct Key(GoString);

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, GoJson)]
    #[gojson(go_type = "main.Slot", map_key)]
    struct Slot(i8);

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Numbers")]
    struct Numbers {
        #[gojson(json = "n")]
        n: JsonNumber,
        #[gojson(json = "o,omitempty")]
        o: JsonNumber,
        #[gojson(json = "s,string,omitempty")]
        s: JsonNumber,
        #[gojson(json = "p,omitempty")]
        p: Option<JsonNumber>,
        #[gojson(json = "l,omitempty")]
        l: Option<Vec<JsonNumber>>,
    }

    // The unnamed struct types the generator spells with aliases: go_type is their
    // reflect.Type.String(), checked against the golden type names in decode_by_name.

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(
        go_type = "struct { Count int \"json:\\\"count,omitempty\\\"\"; \
                   Price float64 \"json:\\\"price,omitempty\\\"\" }",
        anonymous
    )]
    struct AnonMeta {
        #[gojson(json = "count,omitempty")]
        count: isize,
        #[gojson(json = "price,omitempty")]
        price: f64,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(
        go_type = "struct { Type string \"json:\\\"type\\\"\"; Meta *struct { \
                   Count int \"json:\\\"count,omitempty\\\"\"; \
                   Price float64 \"json:\\\"price,omitempty\\\"\" } \
                   \"json:\\\"meta,omitempty\\\"\" }",
        anonymous
    )]
    struct AnonPrebid {
        #[gojson(go = "Type", json = "type")]
        kind: GoString,
        #[gojson(json = "meta,omitempty")]
        meta: Option<AnonMeta>,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(
        go_type = "struct { A int \"json:\\\"a\\\"\"; B []string \"json:\\\"b,omitempty\\\"\" }",
        anonymous
    )]
    struct AnonTop {
        #[gojson(json = "a")]
        a: isize,
        #[gojson(json = "b,omitempty")]
        b: Option<Vec<GoString>>,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Anon")]
    struct Anon {
        #[gojson(json = "prebid")]
        prebid: AnonPrebid,
        #[gojson(json = "list,omitempty")]
        list: Option<Vec<AnonTop>>,
    }

    // ---- types of the review cases (review.go) -------------------------------------------

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Ten")]
    struct Ten {
        #[gojson(go = "A", json = "a")]
        a: isize,
        #[gojson(go = "B", json = "b")]
        b: GoString,
        #[gojson(go = "C", json = "c")]
        c: Option<isize>,
        #[gojson(go = "D", json = "d")]
        d: Option<Vec<isize>>,
        #[gojson(go = "S", json = "s")]
        s: bool,
        #[gojson(go = "F", json = "f")]
        f: f64,
        #[gojson(go = "K", json = "k")]
        k: GoString,
        #[gojson(go = "P", json = "p")]
        p: Option<Box<Small>>,
        #[gojson(go = "M", json = "m")]
        m: Option<BTreeMap<GoString, isize>>,
        #[gojson(go = "G", json = "\u{3c3}")]
        g: GoString,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Eleven")]
    struct Eleven {
        #[gojson(go = "A", json = "a")]
        a: isize,
        #[gojson(go = "B", json = "b")]
        b: GoString,
        #[gojson(go = "C", json = "c")]
        c: Option<isize>,
        #[gojson(go = "D", json = "d")]
        d: Option<Vec<isize>>,
        #[gojson(go = "S", json = "s")]
        s: bool,
        #[gojson(go = "F", json = "f")]
        f: f64,
        #[gojson(go = "K", json = "k")]
        k: GoString,
        #[gojson(go = "P", json = "p")]
        p: Option<Box<Small>>,
        #[gojson(go = "M", json = "m")]
        m: Option<BTreeMap<GoString, isize>>,
        #[gojson(go = "G", json = "\u{3c3}")]
        g: GoString,
        #[gojson(go = "X", json = "x")]
        x: i8,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Ints")]
    struct Ints {
        #[gojson(go = "I8", json = "i8")]
        v_i8: i8,
        #[gojson(go = "I16", json = "i16")]
        v_i16: i16,
        #[gojson(go = "I32", json = "i32")]
        v_i32: i32,
        #[gojson(go = "I64", json = "i64")]
        v_i64: i64,
        #[gojson(go = "I", json = "i")]
        v_i: isize,
        #[gojson(go = "U8", json = "u8")]
        v_u8: u8,
        #[gojson(go = "U16", json = "u16")]
        v_u16: u16,
        #[gojson(go = "U32", json = "u32")]
        v_u32: u32,
        #[gojson(go = "U64", json = "u64")]
        v_u64: u64,
        #[gojson(go = "U", json = "u")]
        v_u: usize,
        #[gojson(go = "Pad", json = "pad")]
        pad: GoString,
    }

    /// `main.Tags`: names that `encoding/json`'s `isValidTag` rejects (x², नाम, Ⅳ and U+A7DC,
    /// unassigned in Go's Unicode 15.0.0) beside a Greek letter and an Arabic-Indic digit.
    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Tags")]
    struct Tags {
        #[gojson(go = "Sup", json = "x\u{b2}")]
        sup: isize,
        #[gojson(go = "Mark", json = "\u{928}\u{93e}\u{92e}")]
        mark: isize,
        #[gojson(go = "Roman", json = "\u{2163}")]
        roman: isize,
        #[gojson(go = "New", json = "\u{a7dc}")]
        new: isize,
        #[gojson(go = "Greek", json = "\u{3c3}")]
        greek: isize,
        #[gojson(go = "Digit", json = "x\u{663}")]
        digit: isize,
    }

    /// `main.Lambda`: U+A7DC has no case mapping in Go's Unicode 15.0.0 (Rust's newer tables
    /// make it the upper case of U+019B).
    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "main.Lambda")]
    struct Lambda {
        #[gojson(go = "L", json = "\u{a7dc}")]
        l: isize,
        #[gojson(go = "S", json = "\u{3c3}")]
        s: isize,
    }

    // ---- real Prebid types used by the adapter fixtures ---------------------------------

    /// `openrtb_ext.ExtImpAdtarget`; `ExtImpAdtelligent` has the same shape.
    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "openrtb_ext.ExtImpAdtarget")]
    struct ExtImpAdtarget {
        #[gojson(go = "SourceId", json = "aid")]
        source_id: JsonNumber,
        #[gojson(go = "PlacementId", json = "placementId,omitempty")]
        placement_id: isize,
        #[gojson(go = "SiteId", json = "siteId,omitempty")]
        site_id: isize,
        #[gojson(go = "BidFloor", json = "bidFloor,omitempty")]
        bid_floor: f64,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "openrtb_ext.ExtImpAdtelligent")]
    struct ExtImpAdtelligent {
        #[gojson(go = "SourceId", json = "aid")]
        source_id: JsonNumber,
        #[gojson(go = "PlacementId", json = "placementId,omitempty")]
        placement_id: isize,
        #[gojson(go = "SiteId", json = "siteId,omitempty")]
        site_id: isize,
        #[gojson(go = "BidFloor", json = "bidFloor,omitempty")]
        bid_floor: f64,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "openrtb_ext.ExtImpBidmatic")]
    struct ExtImpBidmatic {
        #[gojson(go = "SourceId", json = "source")]
        source_id: JsonNumber,
        #[gojson(go = "PlacementId", json = "placementId,omitempty")]
        placement_id: isize,
        #[gojson(go = "SiteId", json = "siteId,omitempty")]
        site_id: isize,
        #[gojson(go = "BidFloor", json = "bidFloor,omitempty")]
        bid_floor: f64,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "openrtb_ext.ExtImpPulsePoint")]
    struct ExtImpPulsePoint {
        #[gojson(go = "PubID", json = "cp")]
        pub_id: StringInt,
        #[gojson(go = "TagID", json = "ct")]
        tag_id: StringInt,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "openrtb_ext.ExtIntenze")]
    struct ExtIntenze {
        #[gojson(go = "AccountID", json = "accountId")]
        account_id: GoString,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "openrtb_ext.ExtImpTheTradeDesk")]
    struct ExtImpTheTradeDesk {
        #[gojson(go = "PublisherId", json = "publisherId")]
        publisher_id: GoString,
        #[gojson(go = "SupplySourceId", json = "supplySourceId")]
        supply_source_id: GoString,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "openrtb_ext.ExtImpImds")]
    struct ExtImpImds {
        #[gojson(go = "SeatId", json = "seatId")]
        seat_id: GoString,
        #[gojson(go = "TagId", json = "tagId")]
        tag_id: GoString,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "openrtb_ext.ExtImpIx")]
    struct ExtImpIx {
        #[gojson(go = "SiteId", json = "siteId")]
        site_id: GoString,
        #[gojson(json = "size")]
        size: Option<Vec<isize>>,
        #[gojson(json = "sid")]
        sid: GoString,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "openrtb_ext.ExtImpAdkernelAdn")]
    struct ExtImpAdkernelAdn {
        #[gojson(go = "PublisherID", json = "pubId")]
        publisher_id: isize,
    }

    #[derive(Clone, Copy, Debug, Default, GoJson)]
    #[gojson(go_type = "openrtb3.NoBidReason")]
    struct NoBidReason(i64);

    #[derive(Clone, Copy, Debug, Default, GoJson)]
    #[gojson(go_type = "adcom1.CategoryTaxonomy")]
    struct CategoryTaxonomy(i64);

    #[derive(Clone, Copy, Debug, Default, GoJson)]
    #[gojson(go_type = "adcom1.CreativeAttribute")]
    struct CreativeAttribute(i64);

    #[derive(Clone, Copy, Debug, Default, GoJson)]
    #[gojson(go_type = "adcom1.APIFramework")]
    struct ApiFramework(i64);

    #[derive(Clone, Copy, Debug, Default, GoJson)]
    #[gojson(go_type = "adcom1.MediaCreativeSubtype")]
    struct MediaCreativeSubtype(i8);

    #[derive(Clone, Copy, Debug, Default, GoJson)]
    #[gojson(go_type = "adcom1.MediaRating")]
    struct MediaRating(i8);

    #[derive(Clone, Copy, Debug, Default, GoJson)]
    #[gojson(go_type = "openrtb2.MarkupType")]
    struct MarkupType(i8);

    #[derive(Clone, Copy, Debug, Default, GoJson)]
    #[gojson(go_type = "adcom1.SlotPositionInPod")]
    struct SlotPositionInPod(i8);

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "openrtb2.Bid")]
    struct Bid {
        #[gojson(go = "ID", json = "id")]
        id: GoString,
        #[gojson(go = "ImpID", json = "impid")]
        imp_id: GoString,
        #[gojson(json = "price")]
        price: f64,
        #[gojson(go = "NURL", json = "nurl,omitempty")]
        nurl: GoString,
        #[gojson(go = "BURL", json = "burl,omitempty")]
        burl: GoString,
        #[gojson(go = "LURL", json = "lurl,omitempty")]
        lurl: GoString,
        #[gojson(go = "AdM", json = "adm,omitempty")]
        adm: GoString,
        #[gojson(go = "AdID", json = "adid,omitempty")]
        ad_id: GoString,
        #[gojson(go = "ADomain", json = "adomain,omitempty")]
        adomain: Option<Vec<GoString>>,
        #[gojson(json = "bundle,omitempty")]
        bundle: GoString,
        #[gojson(go = "IURL", json = "iurl,omitempty")]
        iurl: GoString,
        #[gojson(go = "CID", json = "cid,omitempty")]
        cid: GoString,
        #[gojson(go = "CrID", json = "crid,omitempty")]
        crid: GoString,
        #[gojson(json = "tactic,omitempty")]
        tactic: GoString,
        #[gojson(go = "CatTax", json = "cattax,omitempty")]
        cat_tax: CategoryTaxonomy,
        #[gojson(json = "cat,omitempty")]
        cat: Option<Vec<GoString>>,
        #[gojson(json = "attr,omitempty")]
        attr: Option<Vec<CreativeAttribute>>,
        #[gojson(go = "APIs", json = "apis,omitempty")]
        apis: Option<Vec<ApiFramework>>,
        #[gojson(go = "API", json = "api,omitempty")]
        api: ApiFramework,
        #[gojson(json = "protocol,omitempty")]
        protocol: MediaCreativeSubtype,
        #[gojson(go = "QAGMediaRating", json = "qagmediarating,omitempty")]
        qag_media_rating: MediaRating,
        #[gojson(json = "language,omitempty")]
        language: GoString,
        #[gojson(go = "LangB", json = "langb,omitempty")]
        lang_b: GoString,
        #[gojson(go = "DealID", json = "dealid,omitempty")]
        deal_id: GoString,
        #[gojson(json = "w,omitempty")]
        w: i64,
        #[gojson(json = "h,omitempty")]
        h: i64,
        #[gojson(go = "WRatio", json = "wratio,omitempty")]
        w_ratio: i64,
        #[gojson(go = "HRatio", json = "hratio,omitempty")]
        h_ratio: i64,
        #[gojson(json = "exp,omitempty")]
        exp: i64,
        #[gojson(json = "dur,omitempty")]
        dur: i64,
        #[gojson(go = "MType", json = "mtype,omitempty")]
        mtype: MarkupType,
        #[gojson(go = "SlotInPod", json = "slotinpod,omitempty")]
        slot_in_pod: SlotPositionInPod,
        #[gojson(json = "ext,omitempty")]
        ext: RawJson,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "openrtb2.SeatBid")]
    struct SeatBid {
        #[gojson(json = "bid")]
        bid: Option<Vec<Bid>>,
        #[gojson(json = "seat,omitempty")]
        seat: GoString,
        #[gojson(json = "group,omitempty")]
        group: i8,
        #[gojson(json = "ext,omitempty")]
        ext: RawJson,
    }

    #[derive(Clone, Debug, Default, GoJson)]
    #[gojson(go_type = "openrtb2.BidResponse")]
    struct BidResponse {
        #[gojson(go = "ID", json = "id")]
        id: GoString,
        #[gojson(go = "SeatBid", json = "seatbid,omitempty")]
        seat_bid: Option<Vec<SeatBid>>,
        #[gojson(go = "BidID", json = "bidid,omitempty")]
        bid_id: GoString,
        #[gojson(json = "cur,omitempty")]
        cur: GoString,
        #[gojson(go = "CustomData", json = "customdata,omitempty")]
        custom_data: GoString,
        #[gojson(go = "NBR", json = "nbr,omitempty")]
        nbr: Option<NoBidReason>,
        #[gojson(json = "ext,omitempty")]
        ext: RawJson,
    }

    // ---- decoding ---------------------------------------------------------------------------

    const DECODE_OUTPUTS: [&str; 5] = [
        "jsoniter error",
        "jsoniter value",
        "jsonutil.Unmarshal error",
        "encoding/json error",
        "encoding/json value",
    ];

    fn decode_outputs<T>(prefill: &[u8], input: &[u8]) -> Vec<Vec<u8>>
    where
        T: Clone + Default + JsoniterDecode + StdDecode + GoEncode,
    {
        let mut v = T::default();
        if !prefill.is_empty() {
            let _prefilled = jsoniter::unmarshal(prefill, &mut v);
        }
        let iter_err = err_text(jsoniter::unmarshal(input, &mut v), |e| e.go_text());
        let iter_val = value_text(&v);
        let mut u = T::default();
        if !prefill.is_empty() {
            let _prefilled = jsonutil::unmarshal(prefill, &mut u);
        }
        let util_err = err_text(jsonutil::unmarshal(input, &mut u), |e| {
            e.message.into_bytes()
        });
        let mut s = T::default();
        if !prefill.is_empty() {
            let _prefilled = stdjson::unmarshal(prefill, &mut s);
        }
        let std_err = err_text(stdjson::unmarshal(input, &mut s), |e| e.go_text());
        vec![iter_err, iter_val, util_err, std_err, value_text(&s)]
    }

    fn decode_by_name(name: &str, prefill: &[u8], input: &[u8]) -> Option<Vec<Vec<u8>>> {
        let p = prefill;
        let i = input;
        if name == go_type_name::<AnonTop>() {
            return Some(decode_outputs::<AnonTop>(p, i));
        }
        if name == go_type_name::<Option<AnonMeta>>() {
            return Some(decode_outputs::<Option<AnonMeta>>(p, i));
        }
        Some(match name {
            "int" => decode_outputs::<isize>(p, i),
            "int8" => decode_outputs::<i8>(p, i),
            "int16" => decode_outputs::<i16>(p, i),
            "int32" => decode_outputs::<i32>(p, i),
            "int64" => decode_outputs::<i64>(p, i),
            "uint" => decode_outputs::<usize>(p, i),
            "uint8" => decode_outputs::<u8>(p, i),
            "uint16" => decode_outputs::<u16>(p, i),
            "uint32" => decode_outputs::<u32>(p, i),
            "uint64" => decode_outputs::<u64>(p, i),
            "float32" => decode_outputs::<f32>(p, i),
            "float64" => decode_outputs::<f64>(p, i),
            "bool" => decode_outputs::<bool>(p, i),
            "string" => decode_outputs::<GoString>(p, i),
            "*int" => decode_outputs::<Option<isize>>(p, i),
            "*string" => decode_outputs::<Option<GoString>>(p, i),
            "[]int" => decode_outputs::<Option<Vec<isize>>>(p, i),
            "[]uint8" => decode_outputs::<Option<Vec<u8>>>(p, i),
            "[]string" => decode_outputs::<Option<Vec<GoString>>>(p, i),
            "[]*int" => decode_outputs::<Option<Vec<Option<isize>>>>(p, i),
            "map[string]int" => decode_outputs::<Option<BTreeMap<GoString, isize>>>(p, i),
            "map[int]string" => decode_outputs::<Option<BTreeMap<isize, GoString>>>(p, i),
            "map[string]interface {}" => decode_outputs::<Option<BTreeMap<GoString, Value>>>(p, i),
            "interface {}" => decode_outputs::<Value>(p, i),
            "json.RawMessage" => decode_outputs::<RawJson>(p, i),
            "*json.RawMessage" => decode_outputs::<Option<RawJson>>(p, i),
            "[]json.RawMessage" => decode_outputs::<Option<Vec<RawJson>>>(p, i),
            "map[string]json.RawMessage" => {
                decode_outputs::<Option<BTreeMap<GoString, RawJson>>>(p, i)
            }
            "jsonutil.StringInt" => decode_outputs::<StringInt>(p, i),
            "jsonutil.IntString" => decode_outputs::<IntString>(p, i),
            "main.Small" => decode_outputs::<Small>(p, i),
            "main.Wide" => decode_outputs::<Wide>(p, i),
            "main.Upper" => decode_outputs::<Upper>(p, i),
            "main.Embeds" => decode_outputs::<Embeds>(p, i),
            "main.Quoted" => decode_outputs::<Quoted>(p, i),
            "main.Custom" => decode_outputs::<Custom>(p, i),
            "main.Loud" => decode_outputs::<Loud>(p, i),
            "main.Maps" => decode_outputs::<Maps>(p, i),
            "main.Empty" => decode_outputs::<Empty>(p, i),
            "main.OnlyPrivate" => decode_outputs::<OnlyPrivate>(p, i),
            "[]main.Small" => decode_outputs::<Option<Vec<Small>>>(p, i),
            "map[string]main.Small" => decode_outputs::<Option<BTreeMap<GoString, Small>>>(p, i),
            "*main.Small" => decode_outputs::<Option<Small>>(p, i),
            "[]main.Validated" => decode_outputs::<Option<Vec<Validated>>>(p, i),
            "json.Number" => decode_outputs::<JsonNumber>(p, i),
            "*json.Number" => decode_outputs::<Option<JsonNumber>>(p, i),
            "main.Numbers" => decode_outputs::<Numbers>(p, i),
            "map[main.Key]int" => decode_outputs::<Option<BTreeMap<Key, isize>>>(p, i),
            "map[main.Slot]string" => decode_outputs::<Option<BTreeMap<Slot, GoString>>>(p, i),
            "main.Anon" => decode_outputs::<Anon>(p, i),
            "main.Ten" => decode_outputs::<Ten>(p, i),
            "main.Eleven" => decode_outputs::<Eleven>(p, i),
            "main.Ints" => decode_outputs::<Ints>(p, i),
            "main.Tags" => decode_outputs::<Tags>(p, i),
            "main.Lambda" => decode_outputs::<Lambda>(p, i),
            "openrtb2.BidResponse" => decode_outputs::<BidResponse>(p, i),
            "[]int8" => decode_outputs::<Option<Vec<i8>>>(p, i),
            "[]int64" => decode_outputs::<Option<Vec<i64>>>(p, i),
            "[]uint64" => decode_outputs::<Option<Vec<u64>>>(p, i),
            "[]float32" => decode_outputs::<Option<Vec<f32>>>(p, i),
            "[]float64" => decode_outputs::<Option<Vec<f64>>>(p, i),
            "[]interface {}" => decode_outputs::<Option<Vec<Value>>>(p, i),
            _ => return None,
        })
    }

    /// Decode outputs that differ from Go by design, as (Go type, input, output index, reason).
    /// Each must still be present in the goldens and still differ, so the list cannot go stale.
    const DECODE_KNOWN_GAPS: [(&str, &[u8], usize, &str); 1] = [(
        "interface {}",
        b"\"a\xffb\"",
        1,
        "jsoniter keeps the invalid byte in the Go string and json.Marshal escapes it as \\ufffd; \
         serde_json::Value can only hold the replaced U+FFFD, which is written raw",
    )];

    #[test]
    fn decode_goldens() -> Result<(), String> {
        replay_decode("decode.txt", &DECODE_KNOWN_GAPS)
    }

    #[test]
    fn review_decode_goldens() -> Result<(), String> {
        on_big_stack(|| replay_decode("review_decode.txt", &[]))
    }

    /// Runs `f` on a thread with a large stack: the review cases nest values 10000 deep (both
    /// Go libraries' limit), and `serde_json::Value` clones and drops them recursively.
    fn on_big_stack(f: fn() -> Result<(), String>) -> Result<(), String> {
        thread::Builder::new()
            .stack_size(256 << 20)
            .spawn(f)
            .map_err(|e| e.to_string())?
            .join()
            .map_err(|_panic| "the replay thread panicked".to_owned())?
    }

    /// Replays the decode rows of `file`; each listed known gap must still be present and
    /// still differ, and only its other outputs are compared.
    fn replay_decode(file: &str, gaps: &[(&str, &[u8], usize, &str)]) -> Result<(), String> {
        let mut failures = Vec::new();
        let mut gaps_seen = 0;
        let rows = rows(file)?;
        for row in &rows {
            let name = String::from_utf8_lossy(&field(row, 0)?).into_owned();
            let prefill = field(row, 1)?;
            let input = field(row, 2)?;
            let mut actual = decode_by_name(&name, &prefill, &input)
                .ok_or_else(|| format!("no Rust type for Go type {name}"))?;
            let what = format!(
                "decode {name} prefill {} input {}",
                quote_to_ascii(&prefill),
                quote_to_ascii(&input)
            );
            for &(gap_type, gap_input, index, _reason) in gaps {
                if gap_type == name && gap_input == input.as_slice() {
                    let want = field(row, 3 + index)?;
                    let slot = actual.get_mut(index).ok_or("bad known-gap index")?;
                    if *slot == want {
                        return Err(format!("{what}: known gap now matches Go; remove it"));
                    }
                    // Compare Go's output with itself so only the other outputs are checked.
                    *slot = want;
                    gaps_seen += 1;
                }
            }
            compare(&mut failures, &what, row, 3, &DECODE_OUTPUTS, &actual);
        }
        if gaps_seen != gaps.len() {
            return Err(format!("only {gaps_seen} known gaps found in {file}"));
        }
        finish(file, rows.len() * DECODE_OUTPUTS.len(), &failures)
    }

    // ---- encoding ---------------------------------------------------------------------------

    const ENCODE_OUTPUTS: [&str; 3] =
        ["jsonutil.Marshal", "json.Marshal", "json.Encoder (no HTML)"];

    fn encode_outputs<T: GoEncode + ?Sized>(v: &T) -> Vec<Vec<u8>> {
        vec![
            output(jsonutil::marshal(v).map_err(|e| e.message.into_bytes())),
            output(stdjson::marshal(v).map_err(|e| e.go_text())),
            output(stdjson::encode_no_escape_html(v).map_err(|e| e.go_text())),
        ]
    }

    fn encode_input<T: Default + StdDecode + GoEncode>(input: &[u8]) -> Option<Vec<Vec<u8>>> {
        let mut v = T::default();
        stdjson::unmarshal(input, &mut v).ok()?;
        Some(encode_outputs(&v))
    }

    fn encode_by_name(name: &str, input: &[u8]) -> Option<Vec<Vec<u8>>> {
        if name == go_type_name::<AnonTop>() {
            return encode_input::<AnonTop>(input);
        }
        match name {
            "string" => encode_input::<GoString>(input),
            "float64" => encode_input::<f64>(input),
            "float32" => encode_input::<f32>(input),
            "[]uint8" => encode_input::<Option<Vec<u8>>>(input),
            "map[int]string" => encode_input::<Option<BTreeMap<isize, GoString>>>(input),
            "map[string]interface {}" => encode_input::<Option<BTreeMap<GoString, Value>>>(input),
            "interface {}" => encode_input::<Value>(input),
            "[]int" => encode_input::<Option<Vec<isize>>>(input),
            "*int" => encode_input::<Option<isize>>(input),
            "json.RawMessage" => encode_input::<RawJson>(input),
            "*json.RawMessage" => encode_input::<Option<RawJson>>(input),
            "[]json.RawMessage" => encode_input::<Option<Vec<RawJson>>>(input),
            "map[string]json.RawMessage" => {
                encode_input::<Option<BTreeMap<GoString, RawJson>>>(input)
            }
            "main.Small" => encode_input::<Small>(input),
            "main.Wide" => encode_input::<Wide>(input),
            "main.Upper" => encode_input::<Upper>(input),
            "main.Embeds" => encode_input::<Embeds>(input),
            "main.Quoted" => encode_input::<Quoted>(input),
            "main.Custom" => encode_input::<Custom>(input),
            "main.Loud" => encode_input::<Loud>(input),
            "main.Maps" => encode_input::<Maps>(input),
            "main.Empty" => encode_input::<Empty>(input),
            "main.OnlyPrivate" => encode_input::<OnlyPrivate>(input),
            "json.Number" => encode_input::<JsonNumber>(input),
            "main.Numbers" => encode_input::<Numbers>(input),
            "map[main.Key]int" => encode_input::<Option<BTreeMap<Key, isize>>>(input),
            "map[main.Slot]string" => encode_input::<Option<BTreeMap<Slot, GoString>>>(input),
            "main.Anon" => encode_input::<Anon>(input),
            "special" => special(input),
            _ => None,
        }
    }

    fn gs(text: &str) -> GoString {
        GoString::from(text)
    }

    fn raw(text: &str) -> RawJson {
        RawJson::from(text)
    }

    /// The generator's `specials`, built the same way.
    fn special(name: &[u8]) -> Option<Vec<Vec<u8>>> {
        let small = Small::default();
        let wide = Wide::default();
        Some(match name {
            b"string-invalid-utf8" => encode_outputs(&GoString::from_bytes(b"a\xffb\xc3".to_vec())),
            b"string-controls" => {
                encode_outputs(&gs("\0\u{1}\u{1f}\u{7f}\"\\/\u{2028}\u{2029}<>&"))
            }
            b"float64-nan" => encode_outputs(&f64::NAN),
            b"float64-inf" => encode_outputs(&f64::INFINITY),
            b"float64-neginf" => encode_outputs(&f64::NEG_INFINITY),
            b"float32-nan" => encode_outputs(&f32::NAN),
            b"small-nan" => encode_outputs(&Small {
                id: gs("x"),
                price: f64::NAN,
                ..small
            }),
            b"wide-two-errors" => encode_outputs(&Wide {
                e: f32::INFINITY,
                l: Some(Box::new(Small {
                    price: f64::NAN,
                    ..small
                })),
                ..wide
            }),
            b"raw-nil" => encode_outputs(&RawJson::nil()),
            b"raw-empty" => encode_outputs(&raw("")),
            b"raw-invalid" => encode_outputs(&raw("{bad")),
            b"small-raw-invalid" => encode_outputs(&Small {
                id: gs("a"),
                ext: raw("{bad"),
                ..small
            }),
            b"small-raw-empty" => encode_outputs(&Small {
                ext: raw(""),
                ..small
            }),
            b"ptr-raw-nil" => encode_outputs(&None::<RawJson>),
            b"ptr-raw-spaces" => encode_outputs(&Some(raw(" [1, 2] \n"))),
            b"ptr-raw-invalid" => encode_outputs(&Some(raw("{bad"))),
            b"ptr-raw-to-nil" => encode_outputs(&Some(RawJson::nil())),
            b"wide-raw" => encode_outputs(&Wide {
                m: raw("[1, {\"a\" : 2}]"),
                p: Some(raw("{\"b\" : 3}")),
                ..wide
            }),
            b"wide-raw-invalid" => encode_outputs(&Wide {
                m: raw("nope"),
                p: Some(raw("nope")),
                ..wide
            }),
            b"loud-empty" => encode_outputs(&Loud::default()),
            b"loud-ptr-bad" => encode_outputs(&Loud {
                s: Shout(gs("a")),
                p: Some(Shout(gs("bad"))),
                ..Loud::default()
            }),
            b"bytes-nil" => encode_outputs(&None::<Vec<u8>>),
            b"bytes-empty" => encode_outputs(&Some(Vec::<u8>::new())),
            b"maps-raw-invalid" => encode_outputs(&Maps {
                raw: Some(BTreeMap::from([
                    (gs("a"), raw("{x")),
                    (gs("b"), RawJson::nil()),
                ])),
                ..Maps::default()
            }),
            b"embeds-nil-extra" => encode_outputs(&Embeds {
                base: Base {
                    id: gs("a"),
                    ..Base::default()
                },
                name: gs("n"),
                ..Embeds::default()
            }),
            b"slice-raw-invalid" => encode_outputs(&Some(vec![raw("1"), raw("{"), RawJson::nil()])),
            b"wide-slice-inf" => encode_outputs(&Wide {
                i: Some(BTreeMap::from([(gs("a"), 1)])),
                k: Some(vec![Small {
                    id: gs("k"),
                    price: f64::NEG_INFINITY,
                    ..small
                }]),
                ..wide
            }),
            _ => return number_special(name),
        })
    }

    /// The generator's `json.Number` specials.
    fn number_special(name: &[u8]) -> Option<Vec<Vec<u8>>> {
        let num = JsonNumber::from;
        Some(match name {
            b"number-empty" => encode_outputs(&num("")),
            b"number-invalid" => encode_outputs(&num("12abc")),
            b"number-plus" => encode_outputs(&num("+1")),
            b"number-html" => encode_outputs(&num("<1>")),
            b"numbers-invalid" => encode_outputs(&Numbers {
                n: num("1e+"),
                s: num("x"),
                ..Numbers::default()
            }),
            b"numbers-quoted-invalid" => encode_outputs(&Numbers {
                n: num("1"),
                s: num("x<"),
                ..Numbers::default()
            }),
            b"numbers-empty-ptr" => encode_outputs(&Numbers {
                p: Some(num("")),
                ..Numbers::default()
            }),
            b"numbers-slice-invalid" => encode_outputs(&Some(vec![num("1"), num(""), num("x")])),
            _ => return anon_special(name),
        })
    }

    /// The generator's specials of unnamed struct types.
    fn anon_special(name: &[u8]) -> Option<Vec<Vec<u8>>> {
        let meta = |count, price| AnonMeta { count, price };
        Some(match name {
            b"anon-nan" => encode_outputs(&Anon {
                prebid: AnonPrebid {
                    kind: gs("x"),
                    meta: Some(meta(0, f64::NAN)),
                },
                list: None,
            }),
            b"anon-meta-inf" => encode_outputs(&Some(meta(1, f64::INFINITY))),
            b"anon-list-meta-nan" => {
                encode_outputs(&Some(vec![Some(meta(1, 0.0)), Some(meta(0, f64::NAN))]))
            }
            _ => return None,
        })
    }

    #[test]
    fn encode_goldens() -> Result<(), String> {
        let mut failures = Vec::new();
        let rows = rows("encode.txt")?;
        for row in &rows {
            let name = String::from_utf8_lossy(&field(row, 0)?).into_owned();
            let input = field(row, 1)?;
            let what = format!("encode {name} {}", quote_to_ascii(&input));
            let actual =
                encode_by_name(&name, &input).ok_or_else(|| format!("cannot build {what}"))?;
            compare(&mut failures, &what, row, 2, &ENCODE_OUTPUTS, &actual);
        }
        finish("encode", rows.len() * ENCODE_OUTPUTS.len(), &failures)
    }

    // ---- review encoding ------------------------------------------------------------------

    /// Decodes `input` with `jsonutil.Unmarshal` (keeping what a failed decode stored, as Go
    /// does) and encodes the value with the three encoders.
    fn review_encode_input<T: Default + JsoniterDecode + GoEncode>(input: &[u8]) -> Vec<Vec<u8>> {
        let mut v = T::default();
        let _decoded = jsonutil::unmarshal(input, &mut v);
        encode_outputs(&v)
    }

    fn review_encode_by_name(name: &str, input: &[u8]) -> Option<Vec<Vec<u8>>> {
        Some(match name {
            "main.Small" => review_encode_input::<Small>(input),
            "main.Ten" => review_encode_input::<Ten>(input),
            "main.Eleven" => review_encode_input::<Eleven>(input),
            "map[string]int" => review_encode_input::<Option<BTreeMap<GoString, isize>>>(input),
            "map[string]string" => {
                review_encode_input::<Option<BTreeMap<GoString, GoString>>>(input)
            }
            "[]string" => review_encode_input::<Option<Vec<GoString>>>(input),
            "[]float64" => review_encode_input::<Option<Vec<f64>>>(input),
            "[]float32" => review_encode_input::<Option<Vec<f32>>>(input),
            "[]json.RawMessage" => review_encode_input::<Option<Vec<RawJson>>>(input),
            "map[string]json.RawMessage" => {
                review_encode_input::<Option<BTreeMap<GoString, RawJson>>>(input)
            }
            "main.Quoted" => review_encode_input::<Quoted>(input),
            "openrtb2.BidResponse" => review_encode_input::<BidResponse>(input),
            "main.Wide" => review_encode_input::<Wide>(input),
            "interface {}" => review_encode_input::<Value>(input),
            "map[string]interface {}" => {
                review_encode_input::<Option<BTreeMap<GoString, Value>>>(input)
            }
            "main.Numbers" => review_encode_input::<Numbers>(input),
            "main.Anon" => review_encode_input::<Anon>(input),
            "main.Embeds" => review_encode_input::<Embeds>(input),
            "main.Maps" => review_encode_input::<Maps>(input),
            "main.Custom" => review_encode_input::<Custom>(input),
            "main.Upper" => review_encode_input::<Upper>(input),
            "main.Tags" => review_encode_input::<Tags>(input),
            _ => return None,
        })
    }

    #[test]
    fn review_encode_goldens() -> Result<(), String> {
        let mut failures = Vec::new();
        let rows = rows("review_encode.txt")?;
        for row in &rows {
            let name = String::from_utf8_lossy(&field(row, 0)?).into_owned();
            let input = field(row, 1)?;
            let what = format!("review encode {name} {}", quote_to_ascii(&input));
            let actual = review_encode_by_name(&name, &input)
                .ok_or_else(|| format!("cannot build {what}"))?;
            compare(&mut failures, &what, row, 2, &ENCODE_OUTPUTS, &actual);
        }
        finish(
            "review encode",
            rows.len() * ENCODE_OUTPUTS.len(),
            &failures,
        )
    }

    // ---- floats -----------------------------------------------------------------------------

    const FLOAT_OUTPUTS: [&str; 2] = ["jsonutil.Marshal", "json.Marshal"];

    fn bits64(f: f64) -> Vec<u8> {
        format!("{:016x}", f.to_bits()).into_bytes()
    }

    fn bits32(f: f32) -> Vec<u8> {
        format!("{:08x}", f.to_bits()).into_bytes()
    }

    fn parse_outputs(text: &[u8]) -> Vec<Vec<u8>> {
        let mut f64_iter = 0.0_f64;
        let r1 = jsoniter::unmarshal(text, &mut f64_iter).map_err(|e| e.go_text());
        let mut f32_iter = 0.0_f32;
        let r2 = jsoniter::unmarshal(text, &mut f32_iter).map_err(|e| e.go_text());
        let mut f64_std = 0.0_f64;
        let r3 = stdjson::unmarshal(text, &mut f64_std).map_err(|e| e.go_text());
        let mut f32_std = 0.0_f32;
        let r4 = stdjson::unmarshal(text, &mut f32_std).map_err(|e| e.go_text());
        vec![
            output(r1.map(|()| bits64(f64_iter))),
            output(r2.map(|()| bits32(f32_iter))),
            output(r3.map(|()| bits64(f64_std))),
            output(r4.map(|()| bits32(f32_std))),
        ]
    }

    /// `json.Number`'s `Int64` and `Float64` (`strconv.ParseInt` and `ParseFloat`) of `text`.
    fn number_outputs(text: &[u8]) -> Vec<Vec<u8>> {
        let n = JsonNumber::new(GoString::from_bytes(text.to_vec()));
        let ints = n.int64().map(|v| v.to_string().into_bytes());
        let floats = n.float64().map(bits64);
        vec![
            output(ints.map_err(|e| e.go_text())),
            output(floats.map_err(|e| e.go_text())),
        ]
    }

    #[test]
    fn float_goldens() -> Result<(), String> {
        let mut failures = Vec::new();
        let rows = rows("floats.txt")?;
        let mut total = 0;
        for row in &rows {
            let kind = field(row, 0)?;
            let arg = field(row, 1)?;
            let text = String::from_utf8_lossy(&arg).into_owned();
            let what = format!("{} {text}", String::from_utf8_lossy(&kind));
            let (names, actual): (&[&str], Vec<Vec<u8>>) = match kind.as_slice() {
                b"f64" => {
                    let bits = u64::from_str_radix(&text, 16).map_err(|e| e.to_string())?;
                    (&FLOAT_OUTPUTS, encode_outputs(&f64::from_bits(bits)))
                }
                b"f32" => {
                    let bits = u32::from_str_radix(&text, 16).map_err(|e| e.to_string())?;
                    (&FLOAT_OUTPUTS, encode_outputs(&f32::from_bits(bits)))
                }
                b"parse" => (
                    &[
                        "jsoniter float64",
                        "jsoniter float32",
                        "std float64",
                        "std float32",
                    ],
                    parse_outputs(&arg),
                ),
                b"number" => (
                    &["json.Number Int64", "json.Number Float64"],
                    number_outputs(&arg),
                ),
                _ => return Err(format!("unknown float row {what}")),
            };
            total += names.len();
            compare(&mut failures, &what, row, 2, names, &actual);
        }
        finish("floats", total, &failures)
    }

    // ---- adapter fixtures ----------------------------------------------------------------

    /// The error the fixture's decoder reports: `jsonutil.Unmarshal`, `json.Unmarshal`, or
    /// (`jsonutil+marshal`) `json.Marshal` of the value `jsonutil.Unmarshal` decoded.
    fn fixture_error<T>(decoder: &[u8], input: &[u8]) -> Vec<u8>
    where
        T: Default + JsoniterDecode + StdDecode + GoEncode,
    {
        let mut v = T::default();
        match decoder {
            b"std" => err_text(stdjson::unmarshal(input, &mut v), |e| e.go_text()),
            b"jsonutil+marshal" => match jsonutil::unmarshal(input, &mut v) {
                Ok(()) => err_text(stdjson::marshal(&v).map(drop), |e| e.go_text()),
                Err(e) => [b"decoding failed: ".as_slice(), e.message.as_bytes()].concat(),
            },
            _ => err_text(jsonutil::unmarshal(input, &mut v), |e| {
                e.message.into_bytes()
            }),
        }
    }

    #[test]
    fn adapter_fixture_errors() -> Result<(), String> {
        let mut failures = Vec::new();
        let rows = rows("fixtures.txt")?;
        for row in &rows {
            let path = String::from_utf8_lossy(&field(row, 0)?).into_owned();
            let decoder = field(row, 1)?;
            let input = field(row, 3)?;
            let got = match field(row, 2)?.as_slice() {
                b"openrtb_ext.ExtImpPulsePoint" => {
                    fixture_error::<ExtImpPulsePoint>(&decoder, &input)
                }
                b"openrtb_ext.ExtIntenze" => fixture_error::<ExtIntenze>(&decoder, &input),
                b"openrtb_ext.ExtImpTheTradeDesk" => {
                    fixture_error::<ExtImpTheTradeDesk>(&decoder, &input)
                }
                b"openrtb_ext.ExtImpImds" => fixture_error::<ExtImpImds>(&decoder, &input),
                b"openrtb_ext.ExtImpIx" => fixture_error::<ExtImpIx>(&decoder, &input),
                b"openrtb_ext.ExtImpAdkernelAdn" => {
                    fixture_error::<ExtImpAdkernelAdn>(&decoder, &input)
                }
                b"openrtb2.BidResponse" => fixture_error::<BidResponse>(&decoder, &input),
                b"openrtb_ext.ExtImpAdtarget" => fixture_error::<ExtImpAdtarget>(&decoder, &input),
                b"openrtb_ext.ExtImpAdtelligent" => {
                    fixture_error::<ExtImpAdtelligent>(&decoder, &input)
                }
                b"openrtb_ext.ExtImpBidmatic" => fixture_error::<ExtImpBidmatic>(&decoder, &input),
                _ => return Err(format!("{path}: no Rust type")),
            };
            // The adapter's message prefix, then the fixture's own expectation, as the adapter
            // test framework checks it.
            let got = [field(row, 7)?, got].concat();
            let expected = field(row, 4)?;
            let satisfied = if field(row, 5)? == b"startswith" {
                got.starts_with(&expected)
            } else {
                got == expected
            };
            if !satisfied {
                failures.push(format!(
                    "{path}: {} does not satisfy the fixture",
                    quote_to_ascii(&got)
                ));
            }
            compare(&mut failures, &path, row, 6, &["full error"], &[got]);
        }
        finish("fixtures", rows.len(), &failures)
    }
}
