//! Replays the golden files that `testdata/go/` writes with Go's `compress/gzip`: the level
//! checks (`levels.txt`), the writer at every level with every kind of header, script and
//! failing writer (`gzip_writer.txt`), and the reader over single and concatenated members,
//! multistream off, `Reset`, truncated and corrupt streams, header flags and Go's own test
//! vectors (`gzip_reader.txt`).

use std::{cell::Cell, io, rc::Rc};

use super::{Reader, Writer};
use crate::compress::{
    flate,
    testutil::{
        BytesReader, err_result, fields, fnv64, gen_input, golden, header_result, mutate, num,
        parse_header, read_trace, report, run_gzip,
    },
};

/// Go: `writeLevels`: the level checks of `flate.NewWriter` and `gzip.NewWriterLevel`.
#[test]
fn levels() -> Result<(), String> {
    let result = |err: Option<String>| err.map_or_else(|| "ok".to_owned(), |e| format!("!{e}"));
    let mut failures = Vec::new();
    for line in golden("levels.txt")?.lines() {
        let [level, want_flate, want_gzip] = fields(line)?;
        let level: isize = num(level)?;
        let got_flate = result(
            flate::Writer::new(io::sink(), level)
                .err()
                .map(|e| e.to_string()),
        );
        let got_gzip = result(
            Writer::new_level(io::sink(), level)
                .err()
                .map(|e| e.to_string()),
        );
        if got_flate != want_flate || got_gzip != want_gzip {
            failures.push(format!(
                "level {level}: got {got_flate} {got_gzip}, want {want_flate} {want_gzip}"
            ));
        }
    }
    report(&failures)
}

/// Checks the lines of `gzip_writer.txt` for `level`.
fn writer_level(level: &str) -> Result<(), String> {
    let mut failures = Vec::new();
    let mut seen = 0;
    for (i, line) in golden("gzip_writer.txt")?.lines().enumerate() {
        let [lvl, header, inputs, script, want] = fields(line)?;
        if lvl != level {
            continue;
        }
        seen += 1;
        let got = run_gzip(num(lvl)?, header, inputs, script)?;
        if got != want {
            failures.push(format!(
                "gzip_writer.txt:{} level {level} header {header} {inputs} {script}\n got \
                 {got}\nwant {want}",
                i + 1
            ));
        }
    }
    if seen == 0 {
        return Err(format!("no level {level} in gzip_writer.txt"));
    }
    report(&failures)
}

#[test]
fn writer_huffman_only() -> Result<(), String> {
    writer_level("-2")
}

#[test]
fn writer_default_compression() -> Result<(), String> {
    writer_level("-1")
}

#[test]
fn writer_no_compression() -> Result<(), String> {
    writer_level("0")
}

#[test]
fn writer_best_speed() -> Result<(), String> {
    writer_level("1")
}

#[test]
fn writer_level2() -> Result<(), String> {
    writer_level("2")
}

#[test]
fn writer_level3() -> Result<(), String> {
    writer_level("3")
}

#[test]
fn writer_level4() -> Result<(), String> {
    writer_level("4")
}

#[test]
fn writer_level5() -> Result<(), String> {
    writer_level("5")
}

#[test]
fn writer_level6() -> Result<(), String> {
    writer_level("6")
}

#[test]
fn writer_level7() -> Result<(), String> {
    writer_level("7")
}

#[test]
fn writer_level8() -> Result<(), String> {
    writer_level("8")
}

#[test]
fn writer_best_compression() -> Result<(), String> {
    writer_level("9")
}

/// Go: `gzipStream`: `raw:SPEC`, or `&`-separated members `LEVEL~HEADER~INPUT`.
fn stream(members: &str) -> Result<Vec<u8>, String> {
    if let Some(spec) = members.strip_prefix("raw:") {
        return gen_input(spec);
    }
    let mut b = Vec::new();
    for member in members.split('&') {
        let f: Vec<&str> = member.split('~').collect();
        let [level, header, input] =
            <[&str; 3]>::try_from(f).map_err(|f| format!("bad member {f:?}"))?;
        let mut w = Writer::new_level(Vec::new(), num(level)?).map_err(|err| err.to_string())?;
        *w.header_mut() = parse_header(header)?;
        let _n = w.write(&gen_input(input)?).map_err(|err| err.to_string())?;
        w.close().map_err(|err| err.to_string())?;
        b.extend(w.into_inner());
    }
    Ok(b)
}

/// Go: `runGzipReader`.
fn read(stream: &[u8], mode: &str, bufsize: usize) -> Result<String, String> {
    let pos = Rc::new(Cell::new(0));
    let remaining = || stream.len().saturating_sub(pos.get());
    let prefix = format!("{}.{:016x}|", stream.len(), fnv64(stream));
    let mut z = match Reader::new(BytesReader::new(stream, &pos)) {
        Ok(z) => z,
        Err(err) => return Ok(format!("{prefix}!{err}|{}", remaining())),
    };
    let pass = |reader: &mut Reader<BytesReader<'_>>| {
        let header = header_result(reader.header());
        let trace = read_trace(|b| reader.read(b), bufsize);
        format!(
            "ok|{header}|{trace}|{}|{}",
            err_result(reader.close()),
            remaining()
        )
    };
    let failed_reset = |err: &crate::GzipError, reader: &Reader<BytesReader<'_>>| {
        format!("!{err}|{}|{}", header_result(reader.header()), remaining())
    };
    let mut parts = Vec::new();
    match mode {
        "ms" => parts.push(pass(&mut z)),
        "ss" => {
            for _ in 0..4 {
                z.multistream(false);
                parts.push(pass(&mut z));
                if let Err(err) = z.reset(BytesReader::new(stream, &pos)) {
                    parts.push(failed_reset(&err, &z));
                    break;
                }
            }
        }
        "rs" => {
            parts.push(pass(&mut z));
            pos.set(0);
            match z.reset(BytesReader::new(stream, &pos)) {
                Ok(()) => parts.push(pass(&mut z)),
                Err(err) => parts.push(failed_reset(&err, &z)),
            }
        }
        _ => return Err(format!("unknown mode {mode}")),
    }
    Ok(format!("{prefix}{}", parts.join("||")))
}

/// Checks part `part` of `parts` of `gzip_reader.txt`.
fn reader_part(part: usize, parts: usize) -> Result<(), String> {
    let text = golden("gzip_reader.txt")?;
    let lines: Vec<&str> = text.lines().collect();
    let (start, end) = (lines.len() * part / parts, lines.len() * (part + 1) / parts);
    let mut failures = Vec::new();
    // Consecutive lines mostly mutate the same stream; build it once.
    let mut cached: Option<(String, Vec<u8>)> = None;
    for (i, line) in lines.iter().enumerate().take(end).skip(start) {
        let [name, members, mutation, mode, bufsize, want] = fields(line)?;
        let base = match cached.take() {
            Some((k, s)) if k == members => s,
            Some(_) | None => stream(members)?,
        };
        let data = mutate(&base, mutation)?;
        cached = Some((members.to_owned(), base));
        let got = read(&data, mode, num(bufsize)?)?;
        if got != want {
            failures.push(format!(
                "gzip_reader.txt:{} {name} {members} {mutation} {mode} buffer {bufsize}\n got \
                 {got}\nwant {want}",
                i + 1
            ));
        }
    }
    report(&failures)
}

#[test]
fn reader_part0() -> Result<(), String> {
    reader_part(0, 3)
}

#[test]
fn reader_part1() -> Result<(), String> {
    reader_part(1, 3)
}

#[test]
fn reader_part2() -> Result<(), String> {
    reader_part(2, 3)
}
