//! Test support: the corpus, script and trace formats of the Go golden generator in
//! `testdata/go/` (`corpus.go`, `writer.go` and `reader.go`), mirrored line for line so that the
//! golden replays in `flate/golden_test.rs` and `gzip/golden_test.rs` rebuild Go's inputs and
//! describe the port's results in Go's words.

use core::{fmt, str::FromStr};
use std::{
    cell::Cell,
    fs,
    io::{self, BufRead, Read, Write},
    path::PathBuf,
    rc::Rc,
};

use pbs_gocompat::time::Time;

use super::{flate, gzip};
use crate::GzipError;

/// Go: `splitmix64`, the generator behind every random input.
#[derive(Clone, Copy, Debug)]
struct SplitMix64(u64);

impl SplitMix64 {
    /// Go: `next`.
    const fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// Go: `vocabulary`, the words of text inputs, most frequent first.
const VOCABULARY: &str = "the bid request response imp banner video native site app device user \
                          geo ext price floor currency USD seat bidder adapter timeout cache \
                          targeting hb_pb hb_bidder deal id width height prebid server auction \
                          quick brown fox jumps over lazy dog privacy consent gdpr ccpa gpp \
                          schain supply chain";

/// Go: `domains`, the site domains of JSON inputs.
const DOMAINS: [&str; 6] = ["example", "prebid", "news", "sports", "weather", "shop"];

fn to_usize(v: u64) -> usize {
    usize::try_from(v).unwrap_or(usize::MAX)
}

fn to_u64(v: usize) -> u64 {
    u64::try_from(v).unwrap_or(u64::MAX)
}

/// Go: `byte(v)`.
const fn low_byte(v: u64) -> u8 {
    let [b, ..] = v.to_le_bytes();
    b
}

/// Go: `genRand`.
fn gen_rand(n: usize, seed: u64) -> Vec<u8> {
    let mut r = SplitMix64(seed);
    let mut b = Vec::with_capacity(n + 8);
    while b.len() < n {
        b.extend_from_slice(&r.next_u64().to_le_bytes());
    }
    b.truncate(n);
    b
}

/// Go: `genText`.
fn gen_text(n: usize, seed: u64) -> Vec<u8> {
    let words: Vec<&str> = VOCABULARY.split_whitespace().collect();
    let count = to_u64(words.len());
    let mut rng = SplitMix64(seed);
    let mut out = Vec::with_capacity(n + 32);
    while out.len() < n {
        let x = rng.next_u64() % count;
        let y = rng.next_u64() % count;
        let word = words
            .get(to_usize(x * y / count))
            .copied()
            .unwrap_or_default();
        out.extend_from_slice(word.as_bytes());
        match rng.next_u64() % 16 {
            0 => out.extend_from_slice(b".\n"),
            1 => out.extend_from_slice(b", "),
            _ => out.push(b' '),
        }
    }
    out.truncate(n);
    out
}

/// Go: `genJSON`.
fn gen_json(n: usize, seed: u64) -> Vec<u8> {
    let mut rng = SplitMix64(seed);
    let mut out = Vec::with_capacity(n + 512);
    let mut i = 0u64;
    while out.len() < n {
        let id = rng.next_u64();
        let width = 100 * (1 + rng.next_u64() % 8);
        let height = 50 * (1 + rng.next_u64() % 12);
        let floor = rng.next_u64() % 1000;
        let domain = DOMAINS
            .get(to_usize(rng.next_u64() % to_u64(DOMAINS.len())))
            .copied()
            .unwrap_or_default();
        let page = rng.next_u64() % 100_000;
        let tmax = 100 + rng.next_u64() % 900;
        let line = format!(
            concat!(
                r#"{{"id":"{:016x}","imp":[{{"id":"{}","banner":{{"w":{},"h":{}}},"#,
                r#""bidfloor":{}.{:02}}}],"site":{{"domain":"{}.com","#,
                r#""page":"https://{}.com/p/{}"}},"tmax":{}}}"#,
            ),
            id,
            i,
            width,
            height,
            floor / 100,
            floor % 100,
            domain,
            domain,
            page,
            tmax,
        );
        out.extend_from_slice(line.as_bytes());
        out.push(b'\n');
        i += 1;
    }
    out.truncate(n);
    out
}

/// Go: `genMixed`.
fn gen_mixed(n: usize, seed: u64) -> Vec<u8> {
    let mut r = SplitMix64(seed);
    let mut b: Vec<u8> = Vec::with_capacity(n + 4096);
    while b.len() < n {
        let kind = r.next_u64() % 5;
        let length = 1 + to_usize(r.next_u64() % 3000);
        match kind {
            0 => b.resize(b.len() + length, 0),
            1 => {
                for _ in 0..length {
                    b.push(low_byte(r.next_u64()));
                }
            }
            2 => {
                // Go: a back reference, possibly beyond the 32 KiB window.
                if b.is_empty() {
                    continue;
                }
                let d = 1 + to_usize(r.next_u64() % to_u64(b.len().min(40000)));
                let start = b.len() - d;
                for j in 0..length {
                    if let Some(&c) = b.get(start + j) {
                        b.push(c);
                    }
                }
            }
            3 => {
                let text_seed = r.next_u64();
                b.extend(gen_text(length, text_seed));
            }
            _ => {
                let c = low_byte(r.next_u64());
                b.resize(b.len() + length, c);
            }
        }
    }
    b.truncate(n);
    b
}

/// Reads `testdata/<rel>` of this crate.
pub(in crate::compress) fn testdata(rel: &str) -> Result<Vec<u8>, String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(rel);
    fs::read(&path).map_err(|err| format!("{}: {err}", path.display()))
}

/// The text of `testdata/golden/<name>`.
pub(in crate::compress) fn golden(name: &str) -> Result<String, String> {
    String::from_utf8(testdata(&format!("golden/{name}"))?)
        .map_err(|err| format!("golden/{name}: {err}"))
}

/// The `N` tab-separated fields of a golden line.
pub(in crate::compress) fn fields<const N: usize>(line: &str) -> Result<[&str; N], String> {
    let v: Vec<&str> = line.split('\t').collect();
    <[&str; N]>::try_from(v).map_err(|v| format!("want {N} fields, got {}: {line}", v.len()))
}

/// Parses a decimal field.
pub(in crate::compress) fn num<T: FromStr>(s: &str) -> Result<T, String>
where
    T::Err: fmt::Display,
{
    s.parse().map_err(|err| format!("{s:?}: {err}"))
}

/// Returns `Ok` when nothing failed, or the first failures and their count.
pub(in crate::compress) fn report(failures: &[String]) -> Result<(), String> {
    if failures.is_empty() {
        return Ok(());
    }
    let shown: Vec<&str> = failures.iter().take(5).map(String::as_str).collect();
    Err(format!(
        "{} failures, the first:\n{}",
        failures.len(),
        shown.join("\n")
    ))
}

/// Go: `hex.EncodeToString`.
pub(in crate::compress) fn hex_encode(b: &[u8]) -> String {
    let mut s = String::with_capacity(2 * b.len());
    for &c in b {
        s.extend(char::from_digit(u32::from(c >> 4), 16));
        s.extend(char::from_digit(u32::from(c & 0x0F), 16));
    }
    s
}

/// Go: `hex.DecodeString`.
pub(in crate::compress) fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    let digits = s
        .chars()
        .map(|c| c.to_digit(16).and_then(|d| u8::try_from(d).ok()))
        .collect::<Option<Vec<u8>>>()
        .ok_or_else(|| format!("bad hex {s:?}"))?;
    let (pairs, odd) = digits.as_chunks::<2>();
    if !odd.is_empty() {
        return Err(format!("odd hex {s:?}"));
    }
    Ok(pairs.iter().map(|&[hi, lo]| (hi << 4) | lo).collect())
}

/// Go: `fnv64`, FNV-1a 64.
pub(in crate::compress) fn fnv64(b: &[u8]) -> u64 {
    b.iter().fold(0xCBF2_9CE4_8422_2325, |h, &c| {
        (h ^ u64::from(c)).wrapping_mul(0x0000_0100_0000_01B3)
    })
}

/// Go: `genInput`: the bytes of an input spec (see `testdata/go/corpus.go`).
pub(in crate::compress) fn gen_input(spec: &str) -> Result<Vec<u8>, String> {
    if spec.contains('+') {
        let mut b = Vec::new();
        for part in spec.split('+') {
            b.extend(gen_input(part)?);
        }
        return Ok(b);
    }
    let f: Vec<&str> = spec.split(':').collect();
    let arg = |i: usize| {
        f.get(i)
            .copied()
            .ok_or_else(|| format!("input spec {spec}: no field {i}"))
    };
    let size = |i: usize| arg(i).and_then(num::<usize>);
    let seed = |i: usize| arg(i).and_then(num::<u64>);
    match arg(0)? {
        "lit" => hex_decode(arg(1)?),
        "zeros" => Ok(vec![0; size(1)?]),
        "rand" => Ok(gen_rand(size(1)?, seed(2)?)),
        "repeat" => {
            let p = gen_rand(size(1)?, seed(3)?);
            Ok(p.iter().copied().cycle().take(size(2)?).collect())
        }
        "low" => {
            let k = seed(1)?;
            let mut r = SplitMix64(seed(3)?);
            Ok((0..size(2)?).map(|_| low_byte(r.next_u64() % k)).collect())
        }
        "text" => Ok(gen_text(size(1)?, seed(2)?)),
        "json" => Ok(gen_json(size(1)?, seed(2)?)),
        "mixed" => Ok(gen_mixed(size(1)?, seed(2)?)),
        "abc" => Ok((0..size(1)?).map(|i| low_byte(to_u64(i % 128))).collect()),
        "seq" => Ok((0..size(1)?).map(|i| low_byte(to_u64(i))).collect()),
        "square" => Ok((0..size(1)?)
            .map(|i| low_byte(to_u64(i.wrapping_mul(i))))
            .collect()),
        "file" => {
            let b = testdata(&format!("corpus/{}", arg(1)?))?;
            if f.len() > 2 {
                let n = size(2)?;
                return b
                    .get(..n)
                    .map(<[u8]>::to_vec)
                    .ok_or_else(|| format!("{spec}: file shorter than {n}"));
            }
            Ok(b)
        }
        other => Err(format!("unknown input spec {other}")),
    }
}

/// Go: `genInputs`: the inputs of a `|`-separated list of input specs.
pub(in crate::compress) fn gen_inputs(specs: &str) -> Result<Vec<Vec<u8>>, String> {
    specs.split('|').map(gen_input).collect()
}

/// Go: `target`: the underlying writer of a compressor. It records the bytes and the `Write`
/// calls, and fails call number `fail_at` (from 0) with Go's `io.ErrClosedPipe`, and when
/// `sticky` every later call too.
#[derive(Debug, Default)]
pub(in crate::compress) struct Target {
    buf: Vec<u8>,
    calls: usize,
    fail_at: Option<usize>,
    sticky: bool,
}

impl Target {
    /// A target whose `Write` call `fail_at` fails, and the later ones too when `sticky`.
    pub(in crate::compress) fn failing(fail_at: usize, sticky: bool) -> Self {
        Self {
            fail_at: Some(fail_at),
            sticky,
            ..Self::default()
        }
    }

    /// The bytes written.
    pub(in crate::compress) fn bytes(&self) -> &[u8] {
        &self.buf
    }

    /// The bytes written.
    pub(in crate::compress) fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    /// The number of `Write` calls.
    pub(in crate::compress) const fn calls(&self) -> usize {
        self.calls
    }

    /// Go: `target.result`: `LEN.FNV.CALLS`.
    fn result(&self) -> String {
        format!(
            "{}.{:016x}.{}",
            self.buf.len(),
            fnv64(&self.buf),
            self.calls
        )
    }
}

impl Write for Target {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let i = self.calls;
        self.calls += 1;
        if let Some(fail_at) = self.fail_at
            && (i == fail_at || self.sticky && i > fail_at)
        {
            return Err(io::Error::other("io: read/write on closed pipe"));
        }
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The compressor a script drives (Go: the `compressor` interface).
#[derive(Debug)]
pub(in crate::compress) enum Subject {
    /// A `flate.Writer`.
    Flate(Box<flate::Writer<Target>>),
    /// A `gzip.Writer` and the header the `H` operation sets.
    Gzip(Box<gzip::Writer<Target>>, Box<gzip::Header>),
}

impl Subject {
    fn write(&mut self, p: &[u8]) -> Result<usize, GzipError> {
        match self {
            Self::Flate(w) => w.write(p),
            Self::Gzip(w, _) => w.write(p),
        }
    }

    fn flush(&mut self) -> Result<(), GzipError> {
        match self {
            Self::Flate(w) => w.flush(),
            Self::Gzip(w, _) => w.flush(),
        }
    }

    fn close(&mut self) -> Result<(), GzipError> {
        match self {
            Self::Flate(w) => w.close(),
            Self::Gzip(w, _) => w.close(),
        }
    }

    fn reset(&mut self, t: Target) -> Target {
        match self {
            Self::Flate(w) => w.reset(t),
            Self::Gzip(w, _) => w.reset(t),
        }
    }

    fn set_header(&mut self) -> Result<(), String> {
        match self {
            Self::Flate(_) => Err("H on a flate writer".to_owned()),
            Self::Gzip(w, h) => {
                h.as_ref().clone_into(w.header_mut());
                Ok(())
            }
        }
    }

    fn target(&self) -> &Target {
        match self {
            Self::Flate(w) => w.get_ref(),
            Self::Gzip(w, _) => w.get_ref(),
        }
    }

    fn into_target(self) -> Target {
        match self {
            Self::Flate(w) => w.into_inner(),
            Self::Gzip(w, _) => w.into_inner(),
        }
    }
}

/// Go: `script`: runs a script, a comma-separated list of operations (see
/// `testdata/go/writer.go`), on a compressor over a list of inputs.
#[derive(Debug)]
pub(in crate::compress) struct Script {
    w: Subject,
    inputs: Vec<Vec<u8>>,
    seg: usize,
    pos: usize,
    /// The targets replaced by resets, in order (Go's `targets` but the last).
    done: Vec<Target>,
    trace: Vec<String>,
    total: usize,
}

impl Script {
    /// A script over `inputs`, writing to the target `w` was created with.
    pub(in crate::compress) const fn new(w: Subject, inputs: Vec<Vec<u8>>) -> Self {
        Self {
            w,
            inputs,
            seg: 0,
            pos: 0,
            done: Vec::new(),
            trace: Vec::new(),
            total: 0,
        }
    }

    fn input_len(&self) -> usize {
        self.inputs.get(self.seg).map_or(0, Vec::len)
    }

    /// Go: `script.write` of the next `n` bytes of the input.
    fn write_next(&mut self, n: usize) -> Result<Option<GzipError>, String> {
        let input = self.inputs.get(self.seg).map_or(&[][..], Vec::as_slice);
        let p = input
            .get(self.pos..self.pos + n)
            .ok_or_else(|| format!("a write of {n} bytes runs past the input"))?;
        let result = self.w.write(p);
        self.pos += n;
        Ok(match result {
            Ok(written) => {
                self.total += written;
                None
            }
            Err(err) => Some(err),
        })
    }

    /// Go: `script.chunks`.
    fn chunks(&mut self, op: &[&str]) -> Result<Vec<Option<GzipError>>, String> {
        let name = op.first().copied().unwrap_or_default();
        let size: u64 = num(op.get(1).copied().unwrap_or_default())?;
        let mut g = None;
        let mut end = self.input_len();
        if name == "r" || name == "rf" {
            g = Some(SplitMix64(num(op.get(2).copied().unwrap_or_default())?));
        } else if let Some(limit) = op.get(2) {
            end = self.pos + num::<usize>(limit)?;
        }
        let mut errs = Vec::new();
        while self.pos < end {
            let n = g
                .as_mut()
                .map_or_else(|| to_usize(size), |g| 1 + to_usize(g.next_u64() % size));
            errs.push(self.write_next(n.min(end - self.pos))?);
            if name == "cf" || name == "rf" {
                errs.push(self.w.flush().err());
            }
        }
        Ok(errs)
    }

    /// Go: `script.exec`.
    fn exec(&mut self, i: usize, text: &str) -> Result<(), String> {
        let op: Vec<&str> = text.split(':').collect();
        let name = op.first().copied().unwrap_or_default();
        let mut errs = Vec::new();
        match name {
            "w" => {
                let rest = self.input_len().saturating_sub(self.pos);
                let n = match op.get(1).copied() {
                    Some("*") => rest,
                    Some(n) => num(n)?,
                    None => return Err(format!("{text}: no count")),
                };
                errs.push(self.write_next(n)?);
            }
            "c" | "cf" | "r" | "rf" => errs = self.chunks(&op)?,
            "e" => errs.push(self.write_next(0)?),
            "f" => errs.push(self.w.flush().err()),
            "x" => errs.push(self.w.close().err()),
            "H" => self.w.set_header()?,
            "R" | "RF" | "RS" => {
                let t = if name == "R" {
                    Target::default()
                } else {
                    Target::failing(num(op.get(1).copied().unwrap_or_default())?, name == "RS")
                };
                let old = self.w.reset(t);
                self.done.push(old);
                if name == "R" && self.seg + 1 < self.inputs.len() {
                    self.seg += 1;
                }
                self.pos = 0;
            }
            _ => return Err(format!("unknown script operation {text}")),
        }
        let failed: Vec<&GzipError> = errs.iter().flatten().collect();
        if let Some(first) = failed.first() {
            self.trace.push(if failed.len() > 1 {
                format!("{i}!{first}*{}", failed.len())
            } else {
                format!("{i}!{first}")
            });
        }
        if name == "f" {
            self.trace
                .push(format!("{i}@{}", self.w.target().bytes().len()));
        }
        Ok(())
    }

    /// Go: `script.run`: runs `text` and returns `TRACE|N|TARGETS|HEX` and the targets.
    pub(in crate::compress) fn run(mut self, text: &str) -> Result<(String, Vec<Target>), String> {
        for (i, op) in text.split(',').enumerate() {
            self.exec(i, op)?;
        }
        let trace = if self.trace.is_empty() {
            "-".to_owned()
        } else {
            self.trace.join(",")
        };
        let mut targets = self.done;
        targets.push(self.w.into_target());
        let results: Vec<String> = targets.iter().map(Target::result).collect();
        let bytes = match targets.as_slice() {
            [only] if only.buf.len() <= 64 => hex_encode(&only.buf),
            _ => "-".to_owned(),
        };
        Ok((
            format!("{trace}|{}|{}|{bytes}", self.total, results.join(";")),
            targets,
        ))
    }
}

/// Go: `runFlate`: runs a script on a `flate.Writer` at `level`, with the dictionary `dict`
/// (an input spec, or `-` for none), over the `|`-separated `inputs`.
pub(in crate::compress) fn run_flate(
    level: isize,
    dict: &str,
    inputs: &str,
    script: &str,
) -> Result<(String, Vec<Target>), String> {
    let w = if dict == "-" {
        flate::Writer::new(Target::default(), level)
    } else {
        flate::Writer::new_dict(Target::default(), level, &gen_input(dict)?)
    }
    .map_err(|err| err.to_string())?;
    Script::new(Subject::Flate(Box::new(w)), gen_inputs(inputs)?).run(script)
}

/// Go: `parseHeader`: `-` or comma-separated `name=HEX`, `comment=HEX`, `extra=HEX` (or
/// `extra=@SPEC`), `mtime=SEC.NSEC` and `os=N`.
pub(in crate::compress) fn parse_header(spec: &str) -> Result<gzip::Header, String> {
    let mut h = gzip::Header {
        os: 255,
        ..gzip::Header::default()
    };
    if spec == "-" {
        return Ok(h);
    }
    let text = |v: &str| String::from_utf8(hex_decode(v)?).map_err(|err| err.to_string());
    for kv in spec.split(',') {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        match k {
            "name" => h.name = text(v)?,
            "comment" => h.comment = text(v)?,
            "extra" => {
                h.extra = Some(match v.strip_prefix('@') {
                    Some(input) => gen_input(input)?,
                    None => hex_decode(v)?,
                });
            }
            "mtime" => {
                let (seconds, nanos) = v.split_once('.').unwrap_or((v, "0"));
                h.mod_time = Time::from_unix(num(seconds)?, num(nanos)?);
            }
            "os" => h.os = num(v)?,
            _ => return Err(format!("unknown header field {k}")),
        }
    }
    Ok(h)
}

/// Go: `runGzip`: runs a script on a `gzip.Writer` at `level` with the header spec `header`.
pub(in crate::compress) fn run_gzip(
    level: isize,
    header: &str,
    inputs: &str,
    script: &str,
) -> Result<String, String> {
    let mut w = gzip::Writer::new_level(Target::default(), level).map_err(|err| err.to_string())?;
    let h = parse_header(header)?;
    h.clone_into(w.header_mut());
    let subject = Subject::Gzip(Box::new(w), Box::new(h));
    Ok(Script::new(subject, gen_inputs(inputs)?).run(script)?.0)
}

/// Go: `headerResult`: `xNAME.xCOMMENT.EXTRA.MTIME.OS`, with hexadecimal strings, `nil` for no
/// extra field and `zero` for no time.
pub(in crate::compress) fn header_result(h: &gzip::Header) -> String {
    let extra = h
        .extra
        .as_ref()
        .map_or_else(|| "nil".to_owned(), |e| format!("x{}", hex_encode(e)));
    let mtime = if h.mod_time.is_zero() {
        "zero".to_owned()
    } else {
        h.mod_time.unix().to_string()
    };
    format!(
        "x{}.x{}.{extra}.{mtime}.{}",
        hex_encode(h.name.as_bytes()),
        hex_encode(h.comment.as_bytes()),
        h.os
    )
}

/// Go: a `bytes.Reader` over a stream. The position is shared with the test, which reports
/// `Len()` after the reader has moved into a decompressor.
#[derive(Debug)]
pub(in crate::compress) struct BytesReader<'a> {
    data: &'a [u8],
    pos: Rc<Cell<usize>>,
}

impl<'a> BytesReader<'a> {
    /// A reader of `data` from `pos`, which it advances.
    pub(in crate::compress) fn new(data: &'a [u8], pos: &Rc<Cell<usize>>) -> Self {
        Self {
            data,
            pos: Rc::clone(pos),
        }
    }

    fn rest(&self) -> &'a [u8] {
        self.data.get(self.pos.get()..).unwrap_or_default()
    }
}

impl Read for BytesReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let rest = self.rest();
        let n = rest.len().min(buf.len());
        if let (Some(dst), Some(src)) = (buf.get_mut(..n), rest.get(..n)) {
            dst.copy_from_slice(src);
        }
        self.pos.set(self.pos.get() + n);
        Ok(n)
    }
}

impl BufRead for BytesReader<'_> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        Ok(self.rest())
    }

    fn consume(&mut self, amount: usize) {
        self.pos.set((self.pos.get() + amount).min(self.data.len()));
    }
}

/// Go: `rle`: joins entries with commas, a run of equal entries as `entry*count`.
pub(in crate::compress) fn rle(entries: &[String]) -> String {
    let mut parts = Vec::new();
    let mut i = 0;
    while let Some(first) = entries.get(i) {
        let run = entries
            .get(i..)
            .unwrap_or_default()
            .iter()
            .take_while(|e| *e == first)
            .count();
        parts.push(if run > 1 {
            format!("{first}*{run}")
        } else {
            first.clone()
        });
        i += run;
    }
    parts.join(",")
}

/// Go: `readTrace`: reads with a `bufsize` buffer until three reads have returned an error (or
/// three reads with an empty buffer) and returns `TRACE|LEN.FNV`.
pub(in crate::compress) fn read_trace<F>(mut read: F, bufsize: usize) -> String
where
    F: FnMut(&mut [u8]) -> (usize, Option<GzipError>),
{
    let mut buf = vec![0; bufsize];
    let mut out = Vec::new();
    let mut entries = Vec::new();
    let mut errs = 0;
    for calls in 0..200_000 {
        let (n, err) = read(&mut buf);
        let Some(data) = buf.get(..n) else {
            entries.push(format!("{n} is past the buffer"));
            break;
        };
        out.extend_from_slice(data);
        if err.is_some() {
            errs += 1;
        }
        entries.push(err.map_or_else(|| n.to_string(), |err| format!("{n}!{err}")));
        if errs == 3 || bufsize == 0 && calls == 2 {
            break;
        }
    }
    format!("{}|{}.{:016x}", rle(&entries), out.len(), fnv64(&out))
}

/// Go: `errResult`: `ok` or `!error`.
pub(in crate::compress) fn err_result(r: Result<(), GzipError>) -> String {
    match r {
        Ok(()) => "ok".to_owned(),
        Err(err) => format!("!{err}"),
    }
}

/// Go: `mutate`: `-`, `t:N` (keep the first N bytes), `x:POS:VAL` (xor the byte at POS with
/// VAL) or `a:HEX` (append).
pub(in crate::compress) fn mutate(stream: &[u8], m: &str) -> Result<Vec<u8>, String> {
    let f: Vec<&str> = m.split(':').collect();
    let arg = |i: usize| f.get(i).copied().unwrap_or_default();
    let mut b = stream.to_vec();
    match arg(0) {
        "-" => {}
        "t" => b.truncate(num(arg(1))?),
        "x" => {
            let pos: usize = num(arg(1))?;
            let val: u8 = num(arg(2))?;
            let slot = b
                .get_mut(pos)
                .ok_or_else(|| format!("{m}: past the stream"))?;
            *slot ^= val;
        }
        "a" => b.extend(hex_decode(arg(1))?),
        _ => return Err(format!("unknown mutation {m}")),
    }
    Ok(b)
}
