//! Byte-exact Go `compress/flate` and `compress/gzip`.
//!
//! A port of Go's DEFLATE compressor and decompressor and of its gzip writer and reader. For the
//! same level, header, sequence of `write` calls and flush points, [`flate::Writer`] and
//! [`gzip::Writer`] write the bytes Go writes, block layout and sync markers included, so
//! request bodies Prebid Server gzips for bidders are identical. [`flate::Reader`] and
//! [`gzip::Reader`] return Go's `(n, err)` pairs for the same buffer sizes and Go's error texts
//! ([`GzipError`]), so code that depends on how a read ends behaves as in Go.
//!
//! Golden files written by Go cover every level over a generated and fixture corpus, write
//! chunkings, flush points, writer reuse, headers, and corrupt and truncated input; the
//! generator lives under `testdata/go/` and documents how to rerun it.
//!
//! # Go to Rust
//!
//! - `gzip.NewWriter(w)`, `gzip.NewWriterLevel(w, level)`: [`gzip::Writer::new`],
//!   [`gzip::Writer::new_level`].
//! - `z.Name = ...` and the other `Header` fields: [`gzip::Writer::header_mut`].
//! - `z.Write(p)`, `z.Flush()`, `z.Close()`, `z.Reset(w)`: [`gzip::Writer::write`],
//!   [`gzip::Writer::flush`], [`gzip::Writer::close`], [`gzip::Writer::reset`].
//! - `gzip.NewReader(r)`, `z.Reset(r)`, `z.Multistream(ok)`: [`gzip::Reader::new`],
//!   [`gzip::Reader::reset`], [`gzip::Reader::multistream`].
//! - `z.Read(p)`, `z.Close()`, `z.Header`: [`gzip::Reader::read`], [`gzip::Reader::close`],
//!   [`gzip::Reader::header`].
//! - `flate.NewWriter(w, level)`, `flate.NewWriterDict(w, level, dict)`: [`flate::Writer::new`],
//!   [`flate::Writer::new_dict`].
//! - `flate.NewReader(r)`, `flate.NewReaderDict(r, dict)`, `Resetter.Reset`:
//!   [`flate::Reader::new`], [`flate::Reader::new_dict`], [`flate::Reader::reset`].
//! - `flate.BestSpeed` and the other levels: [`flate::BEST_SPEED`] and the other constants, also in
//!   [`gzip`].
//! - `crc32.ChecksumIEEE(p)`, `crc32.Update(crc, crc32.IEEETable, p)`: [`crc32::checksum_ieee`],
//!   [`crc32::update`].
//! - `io.EOF`, `flate.CorruptInputError`, `gzip.ErrChecksum` and the other errors: [`GzipError`].
//!
//! Writers take any [`std::io::Write`]; their methods are Go's, returning [`GzipError`] where
//! Go returns an `error`. Readers take a [`std::io::BufRead`] where Go reads through
//! `io.ByteReader` (wrapping other readers in a `bufio.Reader`), and `read` returns Go's pair
//! `(n, Option<GzipError>)` rather than a [`std::io::Read`] result, because Go returns data
//! and `EOF` together. A writer keeps its allocations across `reset`, as Go's pooled writers
//! do, so compressing many bodies with one writer allocates only on the first.
//!
//! # Go versions
//!
//! The reference is Go 1.25, the version `go.mod` declares: the golden files are generated
//! with Go 1.25.7, whose `compress/flate`, `compress/gzip` and `hash/crc32` are those of
//! 1.25.0. The `Dockerfile` builds with Go 1.26.2, which writes and reads the same bytes (the
//! golden files regenerate identically with 1.26.2 and 1.26.11): Go 1.26 only reorders fields
//! of the compressor struct and changes the amd64 CRC-32 assembly. Go 1.24 differs from 1.25
//! only in documentation, in a `NewWriterDict` return statement with the same result, and in
//! lacking the CRC-32 digest's `Clone` method.
//!
//! # Example
//!
//! ```
//! use pbs_gogzip::{GzipError, gzip};
//!
//! let mut w = gzip::Writer::new(Vec::new());
//! w.write(b"hello, world\n")?;
//! w.close()?;
//! // Reuse the writer for the next body, as Go's sync.Pool of gzip writers does.
//! let compressed = w.reset(Vec::new());
//! assert_eq!(
//!     compressed,
//!     [
//!         0x1F, 0x8B, 8, 0, 0, 0, 0, 0, 0, 255, 202, 72, 205, 201, 201, 215, 81, 40, 207, 47,
//!         202, 73, 225, 2, 4, 0, 0, 255, 255, 83, 116, 36, 244, 13, 0, 0, 0,
//!     ]
//! );
//!
//! let mut r = gzip::Reader::new(compressed.as_slice())?;
//! let mut buf = [0; 64];
//! let (n, err) = r.read(&mut buf);
//! assert_eq!(buf.get(..n), Some(&b"hello, world\n"[..]));
//! // Go returns the last bytes and io.EOF from the same Read.
//! assert_eq!(err, Some(GzipError::Eof));
//! # Ok::<(), GzipError>(())
//! ```

mod compress;
pub mod crc32;
mod error;

pub use compress::{flate, gzip};
pub use error::GzipError;
