//! Replays the golden files that `testdata/go/` writes with Go's `compress/flate`: the writer at
//! every level over the corpus and scripts (`flate_writer.txt`, `flate_large.txt`), preset
//! dictionaries (`flate_dict.txt`), the reader over intact, flushed, truncated and corrupt
//! streams and Go's own test vectors (`flate_reader.txt`), and the block writers over Go's
//! `writeBlockTests` and Huffman testdata (`flate_blocks.txt` and `testdata/flate`).

use std::{
    cell::Cell,
    fs, iter,
    path::{Path, PathBuf},
    rc::Rc,
};

use super::{
    Reader,
    huffman_bit_writer::test_support::{Kind, write_to_type},
};
use crate::compress::testutil::{
    BytesReader, Target, err_result, fields, fnv64, gen_input, golden, mutate, num, read_trace,
    report, run_flate, testdata,
};

/// The levels of `flate_writer.txt`'s result columns.
const LEVELS: [isize; 12] = [-2, -1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9];

/// Checks the column of `flate_writer.txt` for `LEVELS[column]`.
fn writer_column(column: usize) -> Result<(), String> {
    let level = LEVELS.get(column).copied().ok_or("no such level")?;
    let mut failures = Vec::new();
    for (i, line) in golden("flate_writer.txt")?.lines().enumerate() {
        let f: Vec<&str> = line.split('\t').collect();
        let (Some(inputs), Some(script), Some(want)) = (f.first(), f.get(1), f.get(2 + column))
        else {
            return Err(format!("flate_writer.txt:{}: short line", i + 1));
        };
        let (got, _) = run_flate(level, "-", inputs, script)?;
        if got != *want {
            failures.push(format!(
                "flate_writer.txt:{} level {level} {inputs} {script}\n got {got}\nwant {want}",
                i + 1
            ));
        }
    }
    report(&failures)
}

#[test]
fn writer_huffman_only() -> Result<(), String> {
    writer_column(0)
}

#[test]
fn writer_default_compression() -> Result<(), String> {
    writer_column(1)
}

#[test]
fn writer_no_compression() -> Result<(), String> {
    writer_column(2)
}

#[test]
fn writer_best_speed() -> Result<(), String> {
    writer_column(3)
}

#[test]
fn writer_level2() -> Result<(), String> {
    writer_column(4)
}

#[test]
fn writer_level3() -> Result<(), String> {
    writer_column(5)
}

#[test]
fn writer_level4() -> Result<(), String> {
    writer_column(6)
}

#[test]
fn writer_level5() -> Result<(), String> {
    writer_column(7)
}

#[test]
fn writer_level6() -> Result<(), String> {
    writer_column(8)
}

#[test]
fn writer_level7() -> Result<(), String> {
    writer_column(9)
}

#[test]
fn writer_level8() -> Result<(), String> {
    writer_column(10)
}

#[test]
fn writer_best_compression() -> Result<(), String> {
    writer_column(11)
}

/// Checks the line of `flate_large.txt` for `level`: an input past Go's `maxHashOffset`.
fn large(level: &str) -> Result<(), String> {
    for line in golden("flate_large.txt")?.lines() {
        let [lvl, input, script, want] = fields(line)?;
        if lvl == level {
            let (got, _) = run_flate(num(lvl)?, "-", input, script)?;
            return report(&if got == want {
                Vec::new()
            } else {
                vec![format!("level {level} {input}\n got {got}\nwant {want}")]
            });
        }
    }
    Err(format!("no level {level} in flate_large.txt"))
}

#[test]
fn large_level2() -> Result<(), String> {
    large("2")
}

#[test]
fn large_level3() -> Result<(), String> {
    large("3")
}

#[test]
fn large_level4() -> Result<(), String> {
    large("4")
}

#[test]
fn large_level5() -> Result<(), String> {
    large("5")
}

#[test]
fn large_level6() -> Result<(), String> {
    large("6")
}

#[test]
fn large_level7() -> Result<(), String> {
    large("7")
}

#[test]
fn large_level8() -> Result<(), String> {
    large("8")
}

#[test]
fn large_level9() -> Result<(), String> {
    large("9")
}

#[test]
fn dict() -> Result<(), String> {
    let mut failures = Vec::new();
    for (i, line) in golden("flate_dict.txt")?.lines().enumerate() {
        let [level, dict, inputs, script, want] = fields(line)?;
        let (got, _) = run_flate(num(level)?, dict, inputs, script)?;
        if got != want {
            failures.push(format!(
                "flate_dict.txt:{} level {level} dict {dict} {inputs} {script}\n got {got}\nwant \
                 {want}",
                i + 1
            ));
        }
    }
    report(&failures)
}

/// Go: `flateStream`: compresses `input` with `script` at `level`, or for level `raw` takes
/// the input as the stream.
fn stream(level: &str, dict: &str, input: &str, script: &str) -> Result<Vec<u8>, String> {
    if level == "raw" {
        return gen_input(input);
    }
    let (_, targets) = run_flate(num(level)?, dict, input, script)?;
    Ok(targets
        .into_iter()
        .next()
        .map_or_else(Vec::new, Target::into_bytes))
}

/// Go: `runFlateReader`.
fn read(stream: &[u8], dict: &[u8], bufsize: usize, reset: bool) -> String {
    let pos = Rc::new(Cell::new(0));
    let mut r = Reader::new_dict(BytesReader::new(stream, &pos), dict);
    let trace = read_trace(|b| r.read(b), bufsize);
    let mut res = format!(
        "{}.{:016x}|{trace}|{}|{}",
        stream.len(),
        fnv64(stream),
        err_result(r.close()),
        stream.len().saturating_sub(pos.get())
    );
    if reset {
        pos.set(0);
        let _old = r.reset(BytesReader::new(stream, &pos), dict);
        let again = read_trace(|b| r.read(b), bufsize);
        res = format!(
            "{res}||{again}|{}|{}",
            err_result(r.close()),
            stream.len().saturating_sub(pos.get())
        );
    }
    res
}

/// Checks part `part` of `parts` of `flate_reader.txt`.
fn reader_part(part: usize, parts: usize) -> Result<(), String> {
    let text = golden("flate_reader.txt")?;
    let lines: Vec<&str> = text.lines().collect();
    let (start, end) = (lines.len() * part / parts, lines.len() * (part + 1) / parts);
    let mut failures = Vec::new();
    // Consecutive lines mostly read the same stream; compress it once.
    let mut cached: Option<(String, Vec<u8>)> = None;
    for (i, line) in lines.iter().enumerate().take(end).skip(start) {
        let [
            name,
            level,
            dict,
            input,
            script,
            mutation,
            bufsize,
            reset,
            want,
        ] = fields(line)?;
        let key = format!("{level}\t{dict}\t{input}\t{script}");
        let base = match cached.take() {
            Some((k, s)) if k == key => s,
            Some(_) | None => stream(level, dict, input, script)?,
        };
        let data = mutate(&base, mutation)?;
        cached = Some((key, base));
        let dict = if dict == "-" {
            Vec::new()
        } else {
            gen_input(dict)?
        };
        let got = read(&data, &dict, num(bufsize)?, reset == "reset");
        if got != want {
            failures.push(format!(
                "flate_reader.txt:{} {name} level {level} {input} {script} {mutation} buffer \
                 {bufsize}\n got {got}\nwant {want}",
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

/// The tokens of a `flate_blocks.txt` line: hexadecimal, runs as `token*count`.
fn tokens(field: &str) -> Result<Vec<u32>, String> {
    let mut tokens = Vec::new();
    for part in field.split(',') {
        let (token, count) = match part.split_once('*') {
            Some((token, count)) => (token, num(count)?),
            None => (part, 1),
        };
        let token = u32::from_str_radix(token, 16).map_err(|err| format!("{token}: {err}"))?;
        tokens.extend(iter::repeat_n(token, count));
    }
    Ok(tokens)
}

/// Go: `testWriterEOF`: the block written with `eof` set is marked final.
fn writer_eof(kind: Kind, tokens: &[u32], input: Option<&[u8]>) -> Result<(), String> {
    let (got, _) = write_to_type(kind, tokens, true, input)?;
    match got.first() {
        Some(b) if b & 1 == 1 => Ok(()),
        Some(_) => Err(format!("{kind:?}: block not marked with EOF")),
        None => Err(format!("{kind:?}: no output received")),
    }
}

/// Go: `TestWriteBlock` and `TestWriteBlockDynamic` (`testBlock`) over `writeBlockTests`.
#[test]
fn write_block() -> Result<(), String> {
    let mut failures = Vec::new();
    for line in golden("flate_blocks.txt")?.lines() {
        let [input, want, want_no_input, tokens_field] = fields(line)?;
        let tokens = tokens(tokens_field)?;
        for (kind, ttype) in [(Kind::Block, "wb"), (Kind::Dynamic, "dyn")] {
            if input != "-" {
                let data = testdata(&format!("flate/{input}"))?;
                let want = want.replace("%s", ttype);
                let expect = testdata(&format!("flate/{want}"))?;
                let (got, after_reset) = write_to_type(kind, &tokens, false, Some(&data))?;
                if got != expect || after_reset != expect {
                    failures.push(format!(
                        "{want}: got {} bytes, {} after reset, want {}",
                        got.len(),
                        after_reset.len(),
                        expect.len()
                    ));
                }
                writer_eof(Kind::Block, &tokens, Some(&data))?;
            }
            let file = want_no_input.replace("%s", ttype);
            let expect = testdata(&format!("flate/{file}"))?;
            let (got, after_reset) = write_to_type(kind, &tokens, false, None)?;
            if got != expect || after_reset != expect {
                failures.push(format!(
                    "{file}: got {} bytes, {} after reset, want {}",
                    got.len(),
                    after_reset.len(),
                    expect.len()
                ));
            } else if got.first().is_some_and(|b| b & 1 == 1) {
                failures.push(format!("{file}: got unexpected EOF"));
            }
            writer_eof(Kind::Block, &tokens, None)?;
        }
    }
    report(&failures)
}

/// Go: `TestBlockHuff` over `testdata/huffman-*.in`.
#[test]
fn block_huff() -> Result<(), String> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/flate");
    let mut names: Vec<String> = fs::read_dir(&dir)
        .map_err(|err| format!("{}: {err}", dir.display()))?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| {
            name.starts_with("huffman-") && Path::new(name).extension().is_some_and(|e| e == "in")
        })
        .collect();
    names.sort();
    if names.is_empty() {
        return Err("no huffman-*.in files".to_owned());
    }
    let mut failures = Vec::new();
    for name in names {
        let input = testdata(&format!("flate/{name}"))?;
        let out = format!("{}.golden", name.strip_suffix(".in").unwrap_or(&name));
        let expect = testdata(&format!("flate/{out}"))?;
        let (got, after_reset) = write_to_type(Kind::Huff, &[], false, Some(&input))?;
        if got != expect || after_reset != expect {
            failures.push(format!(
                "{name}: got {} bytes, {} after reset, want {} ({out})",
                got.len(),
                after_reset.len(),
                expect.len()
            ));
        }
        writer_eof(Kind::Huff, &[], Some(&input))?;
    }
    report(&failures)
}
