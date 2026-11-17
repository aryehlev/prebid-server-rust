//! Port of freecache `ringbuf_test.go`, checked step by step against `ringbuf.golden`.

use std::{borrow::Cow, error::Error};

use super::{
    CacheError, RingBuf,
    golden::{TestResult, check_eq, field, golden_lines, read_golden},
};

/// Go `%q` of bytes, for the ASCII the test buffers hold.
fn quote(bytes: &[u8]) -> String {
    let mut out = String::from("\"");
    for &byte in bytes {
        match byte {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b' '..=b'~' => out.push(char::from(byte)),
            _ => {
                out.push_str("\\x");
                for nibble in [byte >> 4, byte & 0xF] {
                    out.push(char::from_digit(u32::from(nibble), 16).unwrap_or('?'));
                }
            }
        }
    }
    out.push('"');
    out
}

/// Go `%d %v` of an `(n int, err error)` pair.
fn result_text((n, err): (usize, Option<CacheError>)) -> String {
    let err = err.map_or_else(|| "<nil>".to_owned(), |err| err.to_string());
    format!("{n} {err}")
}

/// Go: `TestRingBuf`, recording the same lines as `writeRingBuf`.
fn ring_buf_lines() -> Result<Vec<String>, Box<dyn Error>> {
    let mut lines = Vec::new();
    let mut rb = RingBuf::new(16, 0);
    for _ in 0..2 {
        let full = result_text(rb.write(b"fghibbbbccccddde"));
        lines.push(format!("write {full} {rb}"));
        let wrapped = result_text(rb.write(b"fghibbbbc"));
        lines.push(format!("write {wrapped} {rb} {}", quote(&rb.dump())));
        rb.resize(16);
        let first = rb.evacuate(9, 3);
        lines.push(format!("evacuate {first} {rb} {}", quote(&rb.dump())));
        check_eq(&first, &(rb.end() - 3), "first evacuated offset")?;
        let off = rb.evacuate(15, 5);
        lines.push(format!("evacuate {off} {rb} {}", quote(&rb.dump())));
        check_eq(&off, &(rb.end() - 5), "second evacuated offset")?;
        rb.resize(64);
        lines.push(format!("resize {rb} {}", quote(&rb.dump())));
        rb.resize(32);
        lines.push(format!("resize {rb} {}", quote(&rb.dump())));
        let mut data = [0; 5];
        let read = result_text(rb.read_at(&mut data, off));
        lines.push(format!("readat {read} {}", quote(&data)));
        check_eq(&data, b"efghi", "read at should be efghi")?;
        let mut long = [0; 40];
        lines.push(format!(
            "readat {}",
            result_text(rb.read_at(&mut long, off))
        ));
        let past_end = rb.end() + 1;
        lines.push(format!(
            "readat {}",
            result_text(rb.read_at(&mut long, past_end))
        ));
        let slice = rb.slice(off, 5).map_or_else(
            |err| format!("\"\" {err}"),
            |slice| format!("{} <nil>", quote(&slice)),
        );
        lines.push(format!("slice {slice}"));
        lines.push(format!(
            "equalat {} {}",
            rb.equal_at(b"efghi", off),
            rb.equal_at(b"efghx", off)
        ));
        let overwritten = result_text(rb.write_at(b"XY", off));
        lines.push(format!("writeat {overwritten} {}", quote(&rb.dump())));
        let last = rb.end() - 1;
        lines.push(format!("writeat {}", result_text(rb.write_at(b"XY", last))));
        let out_of_range = rb.evacuate(0, 10);
        lines.push(format!("evacuate {out_of_range}"));
        check_eq(
            &out_of_range,
            &-1,
            "evacutate out of range offset should return error",
        )?;
        lines.push(format!("write {}", result_text(rb.write(&[0; 33]))));
        rb.skip(40);
        lines.push(format!("skip {rb}"));
        // After reset the buffer should behave exactly the same as a new one. Hence, run the
        // test once more again with reset buffer.
        rb.reset(0);
        lines.push(format!("reset {rb}"));
    }
    Ok(lines)
}

#[test]
fn go_ring_buf() -> TestResult {
    let golden = read_golden("ringbuf.golden")?;
    let want = golden_lines(&golden, "ringbuf");
    let got = ring_buf_lines()?;
    check_eq(&got.len(), &want.len(), "ringbuf.golden line count")?;
    for (line, expected) in got.iter().zip(&want) {
        check_eq(line.as_str(), field(expected, 0)?, "RingBuf step")?;
    }
    Ok(())
}

#[test]
fn slice_out_of_range_and_negative_length() -> TestResult {
    let mut rb = RingBuf::new(8, 0);
    check_eq(&rb.write(b"abcdef"), &(6, None), "write")?;
    check_eq(
        &rb.slice(7, 1),
        &Err(CacheError::OutOfRange),
        "offset past the end",
    )?;
    check_eq(
        &rb.slice(0, -1),
        &Err(CacheError::OutOfRange),
        "negative length",
    )?;
    check_eq(
        &rb.slice(0, 6).map(Cow::into_owned),
        &Ok(b"abcdef".to_vec()),
        "whole stream",
    )
}

#[test]
fn zero_sized_buffer_does_not_hang() -> TestResult {
    let mut rb = RingBuf::new(0, 0);
    check_eq(&rb.write(b""), &(0, None), "empty write")?;
    check_eq(
        &rb.write(b"a"),
        &(0, Some(CacheError::OutOfRange)),
        "write too long",
    )?;
    rb.skip(3);
    check_eq(
        &rb.to_string(),
        &"[size:0, start:3, end:3, index:3]".to_owned(),
        "after skip",
    )
}
