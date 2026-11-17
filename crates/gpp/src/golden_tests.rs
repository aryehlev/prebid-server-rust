//! Differential tests against outputs recorded from the real go-gpp v0.2.0.
//!
//! The files under `testdata/golden` are written by `testdata/go` (run
//! `go run ./rust/crates/gpp/testdata/go` from the repository root). Each line is one case with
//! tab-separated fields; strings are quoted with `esc` (see the generator for the format). Every
//! test here rebuilds the Go-side dump of a case in Rust and compares the two strings.

use std::{fs, path::Path};

use crate::{
    GenericSection, GppError, Section,
    constants::SectionId,
    encode, parse,
    sections::{
        CommonUsCoreSegment, CommonUsGpcSegment,
        uspca::{Uspca, UspcaCoreSegment},
        uspco::Uspco,
        uspct::Uspct,
        uspnat::{Uspnat, UspnatCoreSegment},
        usput::{Usput, UsputCoreSegment},
        uspva::Uspva,
    },
    util::{BitStream, IRange, IntRange},
};

/// Replays every line of a golden file through `check`. Fails with a report of the first
/// mismatches, or if the file has fewer than `min_cases` cases (a truncated or missing corpus).
fn replay<F>(name: &str, check: F, min_cases: usize) -> Result<(), String>
where
    F: Fn(&str) -> Result<(), String>,
{
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join("golden")
        .join(name);
    let content = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let failures: Vec<String> = content
        .lines()
        .enumerate()
        .filter_map(|(n, line)| {
            check(line)
                .err()
                .map(|e| format!("{name}:{}: {e}", n.saturating_add(1)))
        })
        .collect();
    let cases = content.lines().count();
    if failures.is_empty() && cases >= min_cases {
        Ok(())
    } else if failures.is_empty() {
        Err(format!(
            "{name}: only {cases} cases, expected at least {min_cases}"
        ))
    } else {
        let shown: Vec<&str> = failures.iter().take(10).map(String::as_str).collect();
        Err(format!(
            "{} mismatches, first {}:\n{}",
            failures.len(),
            shown.len(),
            shown.join("\n")
        ))
    }
}

fn compare(input: &str, want: &str, got: &str) -> Result<(), String> {
    if want == got {
        Ok(())
    } else {
        Err(format!("{input}\n  go:   {want}\n  rust: {got}"))
    }
}

/// The generator's `esc`.
fn esc(s: &[u8]) -> String {
    let body: String = s
        .iter()
        .map(|&c| match c {
            b'\\' | b'"' => format!("\\{}", char::from(c)),
            0x20..=0x7E => char::from(c).to_string(),
            _ => format!("\\x{c:02x}"),
        })
        .collect();
    format!("\"{body}\"")
}

fn nibble(c: u8) -> Result<u8, String> {
    char::from(c)
        .to_digit(16)
        .and_then(|d| u8::try_from(d).ok())
        .ok_or_else(|| format!("bad hex digit {c:#04x}"))
}

/// Inverse of `esc`.
fn unesc(s: &str) -> Result<Vec<u8>, String> {
    let body = s
        .strip_prefix('"')
        .and_then(|b| b.strip_suffix('"'))
        .ok_or_else(|| format!("not a quoted string: {s}"))?;
    let mut out = Vec::with_capacity(body.len());
    let mut bytes = body.bytes();
    while let Some(c) = bytes.next() {
        if c != b'\\' {
            out.push(c);
            continue;
        }
        match bytes.next() {
            Some(e @ (b'\\' | b'"')) => out.push(e),
            Some(b'x') => match (bytes.next(), bytes.next()) {
                (Some(h), Some(l)) => out.push((nibble(h)? << 4) | nibble(l)?),
                _ => return Err(format!("truncated escape in {s}")),
            },
            _ => return Err(format!("bad escape in {s}")),
        }
    }
    Ok(out)
}

fn hex_digit(n: u8) -> char {
    char::from_digit(u32::from(n), 16).unwrap_or('?')
}

fn hex(b: &[u8]) -> String {
    b.iter()
        .flat_map(|&c| [hex_digit(c >> 4), hex_digit(c & 0x0F)])
        .collect()
}

fn unhex(s: &str) -> Result<Vec<u8>, String> {
    s.as_bytes()
        .chunks(2)
        .map(|pair| match pair {
            [h, l] => Ok((nibble(*h)? << 4) | nibble(*l)?),
            _ => Err(format!("odd-length hex {s}")),
        })
        .collect()
}

fn fields(line: &str) -> Vec<&str> {
    line.split('\t').collect()
}

fn err_text(e: &GppError) -> String {
    format!("!{}", esc(e.message().as_bytes()))
}

fn joined<T, F>(items: &[T], sep: &str, f: F) -> String
where
    F: Fn(&T) -> String,
{
    items.iter().map(f).collect::<Vec<_>>().join(sep)
}

fn nums(values: &[u8]) -> Vec<String> {
    values.iter().map(u8::to_string).collect()
}

fn list(values: &[u8]) -> String {
    format!("[{}]", nums(values).join(" "))
}

// ---------------------------------------------------------------------------------------------
// parse.txt and parse_bytes.txt

fn common_fields(c: &CommonUsCoreSegment) -> Vec<String> {
    let mut f = nums(&[
        c.version,
        c.sharing_notice,
        c.sale_opt_out_notice,
        c.targeted_advertising_opt_out_notice,
        c.sale_opt_out,
        c.targeted_advertising_opt_out,
    ]);
    f.push(list(&c.sensitive_data_processing));
    f.push(list(&c.known_child_sensitive_data_consents));
    f.extend(nums(&[
        c.mspa_covered_transaction,
        c.mspa_opt_out_option_mode,
        c.mspa_service_provider_mode,
    ]));
    f
}

fn nat_fields(c: &UspnatCoreSegment) -> Vec<String> {
    let mut f = nums(&[
        c.version,
        c.sharing_notice,
        c.sale_opt_out_notice,
        c.sharing_opt_out_notice,
        c.targeted_advertising_opt_out_notice,
        c.sensitive_data_processing_opt_out_notice,
        c.sensitive_data_limit_use_notice,
        c.sale_opt_out,
        c.sharing_opt_out,
        c.targeted_advertising_opt_out,
    ]);
    f.push(list(&c.sensitive_data_processing));
    f.push(list(&c.known_child_sensitive_data_consents));
    f.extend(nums(&[
        c.personal_data_consents,
        c.mspa_covered_transaction,
        c.mspa_opt_out_option_mode,
        c.mspa_service_provider_mode,
    ]));
    f
}

fn ca_fields(c: &UspcaCoreSegment) -> Vec<String> {
    let mut f = nums(&[
        c.version,
        c.sale_opt_out_notice,
        c.sharing_opt_out_notice,
        c.sensitive_data_limit_use_notice,
        c.sale_opt_out,
        c.sharing_opt_out,
    ]);
    f.push(list(&c.sensitive_data_processing));
    f.push(list(&c.known_child_sensitive_data_consents));
    f.extend(nums(&[
        c.personal_data_consents,
        c.mspa_covered_transaction,
        c.mspa_opt_out_option_mode,
        c.mspa_service_provider_mode,
    ]));
    f
}

fn ut_fields(c: &UsputCoreSegment) -> Vec<String> {
    let mut f = nums(&[
        c.version,
        c.sharing_notice,
        c.sale_opt_out_notice,
        c.targeted_advertising_opt_out_notice,
        c.sensitive_data_processing_opt_out_notice,
        c.sale_opt_out,
        c.targeted_advertising_opt_out,
    ]);
    f.push(list(&c.sensitive_data_processing));
    f.extend(nums(&[
        c.known_child_sensitive_data_consents,
        c.mspa_covered_transaction,
        c.mspa_opt_out_option_mode,
        c.mspa_service_provider_mode,
    ]));
    f
}

/// How a dump quotes strings: the generator's `esc` or `short`.
type Quote = fn(&[u8]) -> String;

fn typed(
    kind: &str,
    sec: &Section,
    mut f: Vec<String>,
    gpc: Option<&CommonUsGpcSegment>,
    q: Quote,
) -> String {
    if let Some(g) = gpc {
        f.push(format!("gpc={}/{}", g.subsection_type, g.gpc));
    }
    format!(
        "{kind}:{}:{}:{}:{}:{}",
        sec.id().0,
        q(sec.value().as_bytes()),
        f.join(","),
        q(&sec.encode(true)),
        q(&sec.encode(false))
    )
}

/// The generator's `dumpSection`.
fn dump_section(sec: &Section, q: Quote) -> String {
    match sec {
        Section::Uspnat(s) => typed(
            "nat",
            sec,
            nat_fields(&s.core_segment),
            Some(&s.gpc_segment),
            q,
        ),
        Section::Uspca(s) => typed(
            "ca",
            sec,
            ca_fields(&s.core_segment),
            Some(&s.gpc_segment),
            q,
        ),
        Section::Uspva(s) => typed("va", sec, common_fields(&s.core_segment), None, q),
        Section::Uspco(s) => typed(
            "co",
            sec,
            common_fields(&s.core_segment),
            Some(&s.gpc_segment),
            q,
        ),
        Section::Usput(s) => typed("ut", sec, ut_fields(&s.core_segment), None, q),
        Section::Uspct(s) => typed(
            "ct",
            sec,
            common_fields(&s.core_segment),
            Some(&s.gpc_segment),
            q,
        ),
        Section::Generic(g) => format!("g:{}:{}", g.id().0, q(g.value().as_bytes())),
    }
}

fn dump_types(types: &[SectionId]) -> String {
    format!("t=[{}]", joined(types, ",", |id| id.0.to_string()))
}

fn dump_errors(errs: &[GppError], q: Quote) -> String {
    format!("e=[{}]", joined(errs, ",", |e| q(e.message().as_bytes())))
}

/// The generator's `dumpParse`.
fn dump_parse(input: &str) -> String {
    dump_parse_q(input, esc)
}

/// The generator's `dumpParseQ`: `dumpParse` with every string written through `q`.
fn dump_parse_q(input: &str, q: Quote) -> String {
    let (mut container, errs) = parse(input);
    let types = dump_types(&container.section_types);
    let sections = joined(&container.sections, ";", |s| dump_section(s, q));
    let errors = dump_errors(&errs, q);
    // Encode sorts in place, so it runs after the sections were dumped.
    let encoded = match encode(&mut container.sections) {
        Ok(s) => q(s.as_bytes()),
        Err(e) => format!("!{}", q(e.message().as_bytes())),
    };
    format!(
        "v={} {types} s=[{sections}] {errors} x={encoded}",
        container.version
    )
}

fn parse_case(line: &str) -> Result<(), String> {
    let f = fields(line);
    let [input, want] = f.as_slice() else {
        return Err(format!("bad line {line}"));
    };
    let text = String::from_utf8(unesc(input)?).map_err(|e| e.to_string())?;
    compare(input, want, &dump_parse(&text))
}

fn parse_bytes_case(line: &str) -> Result<(), String> {
    let f = fields(line);
    let [input, want] = f.as_slice() else {
        return Err(format!("bad line {line}"));
    };
    let bytes = unesc(input)?;
    let (container, errs) = parse(&String::from_utf8_lossy(&bytes));
    let got = format!(
        "{} {}",
        dump_types(&container.section_types),
        dump_errors(&errs, esc)
    );
    compare(input, want, &got)
}

#[test]
fn parse_matches_go() -> Result<(), String> {
    replay("parse.txt", parse_case, 5000)
}

#[test]
fn lossy_utf8_parse_matches_go() -> Result<(), String> {
    replay("parse_bytes.txt", parse_bytes_case, 400)
}

// ---------------------------------------------------------------------------------------------
// bitstream.txt

fn dump_int_range(r: &IntRange) -> String {
    format!(
        "{}/{}/[{}]",
        r.size,
        r.max,
        joined(&r.range, " ", |x| format!("{}-{}", x.start_id, x.end_id))
    )
}

fn run_op(op: &str, bs: &mut BitStream) -> Result<Result<String, GppError>, String> {
    Ok(match op {
        "b1" => bs.read_byte1().map(|v| v.to_string()),
        "b2" => bs.read_byte2().map(|v| v.to_string()),
        "b4" => bs.read_byte4().map(|v| v.to_string()),
        "b6" => bs.read_byte6().map(|v| v.to_string()),
        "b8" => bs.read_byte8().map(|v| v.to_string()),
        "u12" => bs.read_uint12().map(|v| v.to_string()),
        "u16" => bs.read_uint16().map(|v| v.to_string()),
        "fib" => bs.read_fibonacci_int().map(|v| v.to_string()),
        "fibr" => bs.read_fibonacci_range().map(|r| dump_int_range(&r)),
        "intr" => bs.read_int_range().map(|r| dump_int_range(&r)),
        _ => {
            let n = op
                .strip_prefix("tbf")
                .ok_or_else(|| format!("unknown op {op}"))?
                .parse::<i64>()
                .map_err(|e| e.to_string())?;
            bs.read_two_bit_field(n).map(|v| list(&v))
        }
    })
}

fn bitstream_case(line: &str) -> Result<(), String> {
    let f = fields(line);
    let [op, data, offset, want] = f.as_slice() else {
        return Err(format!("bad line {line}"));
    };
    let mut bs = BitStream::new(unhex(data)?);
    bs.set_position(offset.parse::<u16>().map_err(|e| e.to_string())?);
    let got = match run_op(op, &mut bs)? {
        Ok(v) => format!("={v}@{}", bs.position()),
        Err(e) => err_text(&e),
    };
    compare(line, want, &got)
}

#[test]
fn bitstream_reads_match_go() -> Result<(), String> {
    replay("bitstream.txt", bitstream_case, 6000)
}

// ---------------------------------------------------------------------------------------------
// base64.txt

fn base64_case(line: &str) -> Result<(), String> {
    let f = fields(line);
    let [input, want] = f.as_slice() else {
        return Err(format!("bad line {line}"));
    };
    let text = String::from_utf8(unesc(input)?).map_err(|e| e.to_string())?;
    let got = match BitStream::from_base64(&text) {
        Ok(bs) => format!("={}", hex(bs.bytes())),
        Err(e) => err_text(&e),
    };
    compare(input, want, &got)
}

#[test]
fn base64_decoding_matches_go() -> Result<(), String> {
    replay("base64.txt", base64_case, 1000)
}

// ---------------------------------------------------------------------------------------------
// write.txt

fn apply_write(bs: &mut BitStream, op: &str) -> Result<(), String> {
    let (width, value) = op
        .split_once(':')
        .ok_or_else(|| format!("bad write op {op}"))?;
    let value = value.parse::<u16>().map_err(|e| e.to_string())?;
    let byte = || u8::try_from(value).map_err(|e| e.to_string());
    match width {
        "1" => bs.write_byte1(byte()?),
        "2" => bs.write_byte2(byte()?),
        "4" => bs.write_byte4(byte()?),
        "6" => bs.write_byte6(byte()?),
        "8" => bs.write_byte8(byte()?),
        "12" => bs.write_uint12(value),
        "16" => bs.write_uint16(value),
        _ => return Err(format!("bad width {width}")),
    }
    Ok(())
}

fn parse_ranges(ranges: &str) -> Result<Vec<IRange>, String> {
    ranges
        .split(' ')
        .filter(|r| !r.is_empty())
        .map(|r| {
            let (start, end) = r.split_once('-').ok_or_else(|| format!("bad range {r}"))?;
            Ok(IRange {
                start_id: start.parse().map_err(|e| format!("{r}: {e}"))?,
                end_id: end.parse().map_err(|e| format!("{r}: {e}"))?,
            })
        })
        .collect()
}

fn write_case(line: &str) -> Result<(), String> {
    let mut bs = BitStream::new_for_write();
    let f = fields(line);
    match f.as_slice() {
        ["fib", n, want] => {
            let n = n.parse::<u16>().map_err(|e| e.to_string())?;
            let got = match bs.write_fibonacci_int(n) {
                Ok(()) => hex(bs.bytes()),
                Err(e) => err_text(&e),
            };
            compare(line, want, &got)
        }
        ["ops", ops, want] => {
            for op in ops.split(',').filter(|op| !op.is_empty()) {
                apply_write(&mut bs, op)?;
            }
            compare(line, want, &hex(bs.bytes()))
        }
        ["range", ranges, want_hex, want_result] => {
            let range = parse_ranges(ranges)?;
            let size = u16::try_from(range.len()).map_err(|e| e.to_string())?;
            let result = match bs.write_int_range(&IntRange {
                size,
                range,
                max: 0,
            }) {
                Ok(()) => String::new(),
                Err(e) => err_text(&e),
            };
            compare(
                line,
                &format!("{want_hex}\t{want_result}"),
                &format!("{}\t{result}", hex(bs.bytes())),
            )
        }
        _ => Err(format!("bad line {line}")),
    }
}

#[test]
fn bit_writes_match_go() -> Result<(), String> {
    replay("write.txt", write_case, 7000)
}

// ---------------------------------------------------------------------------------------------
// encode.txt

fn num(s: &str) -> Result<u8, String> {
    s.parse::<u8>().map_err(|e| format!("{s}: {e}"))
}

fn dotted(s: &str) -> Result<Vec<u8>, String> {
    if s.is_empty() {
        Ok(Vec::new())
    } else {
        s.split('.').map(num).collect()
    }
}

fn gpc_segment(s: &str) -> Result<CommonUsGpcSegment, String> {
    let (subsection_type, gpc) = s.split_once('/').ok_or_else(|| format!("bad gpc {s}"))?;
    Ok(CommonUsGpcSegment {
        subsection_type: num(subsection_type)?,
        gpc: gpc == "1",
    })
}

fn common_core(f: &[&str]) -> Result<CommonUsCoreSegment, String> {
    let [v, sn, son, taon, so, tao, sdp, kc, mct, moom, mspm] = f else {
        return Err(format!("bad common core {f:?}"));
    };
    Ok(CommonUsCoreSegment {
        version: num(v)?,
        sharing_notice: num(sn)?,
        sale_opt_out_notice: num(son)?,
        targeted_advertising_opt_out_notice: num(taon)?,
        sale_opt_out: num(so)?,
        targeted_advertising_opt_out: num(tao)?,
        sensitive_data_processing: dotted(sdp)?,
        known_child_sensitive_data_consents: dotted(kc)?,
        mspa_covered_transaction: num(mct)?,
        mspa_opt_out_option_mode: num(moom)?,
        mspa_service_provider_mode: num(mspm)?,
    })
}

fn common_core_and_gpc(f: &[&str]) -> Result<(CommonUsCoreSegment, CommonUsGpcSegment), String> {
    let [core @ .., gpc] = f else {
        return Err(format!("bad section {f:?}"));
    };
    Ok((common_core(core)?, gpc_segment(gpc)?))
}

fn nat_section(section_id: SectionId, f: &[&str]) -> Result<Section, String> {
    let [
        v,
        sn,
        sale_notice,
        share_notice,
        taon,
        sdpoon,
        sdlun,
        sale,
        share,
        tao,
        sdp,
        kc,
        pdc,
        mct,
        moom,
        mspm,
        gpc,
    ] = f
    else {
        return Err(format!("bad uspnat {f:?}"));
    };
    Ok(Section::Uspnat(Uspnat {
        section_id,
        value: String::new(),
        core_segment: UspnatCoreSegment {
            version: num(v)?,
            sharing_notice: num(sn)?,
            sale_opt_out_notice: num(sale_notice)?,
            sharing_opt_out_notice: num(share_notice)?,
            targeted_advertising_opt_out_notice: num(taon)?,
            sensitive_data_processing_opt_out_notice: num(sdpoon)?,
            sensitive_data_limit_use_notice: num(sdlun)?,
            sale_opt_out: num(sale)?,
            sharing_opt_out: num(share)?,
            targeted_advertising_opt_out: num(tao)?,
            sensitive_data_processing: dotted(sdp)?,
            known_child_sensitive_data_consents: dotted(kc)?,
            personal_data_consents: num(pdc)?,
            mspa_covered_transaction: num(mct)?,
            mspa_opt_out_option_mode: num(moom)?,
            mspa_service_provider_mode: num(mspm)?,
        },
        gpc_segment: gpc_segment(gpc)?,
    }))
}

fn ca_section(section_id: SectionId, f: &[&str]) -> Result<Section, String> {
    let [
        v,
        sale_notice,
        share_notice,
        sdlun,
        sale,
        share,
        sdp,
        kc,
        pdc,
        mct,
        moom,
        mspm,
        gpc,
    ] = f
    else {
        return Err(format!("bad uspca {f:?}"));
    };
    Ok(Section::Uspca(Uspca {
        section_id,
        value: String::new(),
        core_segment: UspcaCoreSegment {
            version: num(v)?,
            sale_opt_out_notice: num(sale_notice)?,
            sharing_opt_out_notice: num(share_notice)?,
            sensitive_data_limit_use_notice: num(sdlun)?,
            sale_opt_out: num(sale)?,
            sharing_opt_out: num(share)?,
            sensitive_data_processing: dotted(sdp)?,
            known_child_sensitive_data_consents: dotted(kc)?,
            personal_data_consents: num(pdc)?,
            mspa_covered_transaction: num(mct)?,
            mspa_opt_out_option_mode: num(moom)?,
            mspa_service_provider_mode: num(mspm)?,
        },
        gpc_segment: gpc_segment(gpc)?,
    }))
}

fn ut_section(section_id: SectionId, f: &[&str]) -> Result<Section, String> {
    let [v, sn, son, taon, sdpoon, so, tao, sdp, kc, mct, moom, mspm] = f else {
        return Err(format!("bad usput {f:?}"));
    };
    Ok(Section::Usput(Usput {
        section_id,
        value: String::new(),
        core_segment: UsputCoreSegment {
            version: num(v)?,
            sharing_notice: num(sn)?,
            sale_opt_out_notice: num(son)?,
            targeted_advertising_opt_out_notice: num(taon)?,
            sensitive_data_processing_opt_out_notice: num(sdpoon)?,
            sale_opt_out: num(so)?,
            targeted_advertising_opt_out: num(tao)?,
            sensitive_data_processing: dotted(sdp)?,
            known_child_sensitive_data_consents: num(kc)?,
            mspa_covered_transaction: num(mct)?,
            mspa_opt_out_option_mode: num(moom)?,
            mspa_service_provider_mode: num(mspm)?,
        },
    }))
}

/// Builds a section from the generator's `<kind>:<id>:<fields>` description.
fn build_section(desc: &str) -> Result<Section, String> {
    let mut parts = desc.splitn(3, ':');
    let (Some(kind), Some(id), Some(rest)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(format!("bad section {desc}"));
    };
    let section_id = SectionId(id.parse().map_err(|e| format!("{desc}: {e}"))?);
    let f: Vec<&str> = rest.split(',').collect();
    match kind {
        "g" => Ok(Section::Generic(GenericSection::new(
            section_id,
            rest.to_owned(),
        ))),
        "nat" => nat_section(section_id, &f),
        "ca" => ca_section(section_id, &f),
        "va" => Ok(Section::Uspva(Uspva {
            section_id,
            value: String::new(),
            core_segment: common_core(&f)?,
        })),
        "co" => {
            let (core_segment, gpc_segment) = common_core_and_gpc(&f)?;
            Ok(Section::Uspco(Uspco {
                section_id,
                value: String::new(),
                core_segment,
                gpc_segment,
            }))
        }
        "ut" => ut_section(section_id, &f),
        "ct" => {
            let (core_segment, gpc_segment) = common_core_and_gpc(&f)?;
            Ok(Section::Uspct(Uspct {
                section_id,
                value: String::new(),
                core_segment,
                gpc_segment,
            }))
        }
        _ => Err(format!("unknown section kind {kind}")),
    }
}

fn encode_case(line: &str) -> Result<(), String> {
    let f = fields(line);
    let [desc, want] = f.as_slice() else {
        return Err(format!("bad line {line}"));
    };
    let mut sections = desc
        .split(';')
        .filter(|d| !d.is_empty())
        .map(build_section)
        .collect::<Result<Vec<_>, _>>()?;
    let got = match encode(&mut sections) {
        Ok(s) => format!("={}", esc(s.as_bytes())),
        Err(e) => err_text(&e),
    };
    compare(desc, want, &got)
}

#[test]
fn encode_matches_go() -> Result<(), String> {
    replay("encode.txt", encode_case, 600)
}

#[test]
fn esc_round_trips() {
    let raw = b"a\\b\"c\x00\x7f\xff~\t";
    assert_eq!(unesc(&esc(raw)), Ok(raw.to_vec()), "esc/unesc");
    assert_eq!(unhex(&hex(raw)), Ok(raw.to_vec()), "hex/unhex");
}

// ---------------------------------------------------------------------------------------------
// edge.txt (testdata/go/edge.go): long Fibonacci codes, wrapping and repeated section IDs,
// inputs of 8 to 128 KiB, every GPC sub-segment shape, and tens of thousands of sections.

/// The generator's `fnvHash`: `#<length>:<FNV-1a 64 hex>`.
fn fnv_hash(s: &[u8]) -> String {
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    for &b in s {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    format!("#{}:{h:016x}", s.len())
}

/// The generator's `short`.
fn short(s: &[u8]) -> String {
    if s.len() <= 200 { esc(s) } else { fnv_hash(s) }
}

/// The generator's `capped`.
fn capped(s: String) -> String {
    if s.len() <= 4096 {
        s
    } else {
        fnv_hash(s.as_bytes())
    }
}

/// `prefix + unit * count + suffix`.
fn recipe(prefix: &[u8], count: &str, unit: &[u8], suffix: &[u8]) -> Result<Vec<u8>, String> {
    let count = count
        .parse::<usize>()
        .map_err(|e| format!("{count}: {e}"))?;
    let mut out = prefix.to_vec();
    out.extend_from_slice(&unit.repeat(count));
    out.extend_from_slice(suffix);
    Ok(out)
}

fn edge_case(line: &str) -> Result<(), String> {
    let f = fields(line);
    match f.as_slice() {
        ["P", prefix, count, unit, suffix, want] => {
            let input = recipe(&unesc(prefix)?, count, &unesc(unit)?, &unesc(suffix)?)?;
            let text = String::from_utf8(input).map_err(|e| e.to_string())?;
            let shown = format!("{prefix} + {count} * {unit} + {suffix}");
            compare(&shown, want, &capped(dump_parse_q(&text, short)))
        }
        ["B", op, prefix, count, unit, suffix, offset, want] => {
            let data = recipe(&unhex(prefix)?, count, &unhex(unit)?, &unhex(suffix)?)?;
            let mut bs = BitStream::new(data);
            bs.set_position(offset.parse::<u16>().map_err(|e| e.to_string())?);
            let got = match run_op(op, &mut bs)? {
                Ok(v) => format!("={v}@{}", bs.position()),
                Err(e) => err_text(&e),
            };
            let shown = format!("{op} {prefix} + {count} * {unit} + {suffix} at {offset}");
            compare(&shown, want, &capped(got))
        }
        _ => Err(format!("bad line {line}")),
    }
}

#[test]
fn edge_cases_match_go() -> Result<(), String> {
    replay("edge.txt", edge_case, 7000)
}

#[test]
fn fnv_hash_matches_go() {
    // Go: fmt.Sprintf("#%d:%016x", 3, fnv.New64a() of "abc").
    assert_eq!(fnv_hash(b"abc"), "#3:e71fa2190541574b", "fnv-1a 64");
    assert_eq!(fnv_hash(b""), "#0:cbf29ce484222325", "empty");
}
