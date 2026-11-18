//! The input reader (Go: `readerc.go`): BOM detection, UTF-8/UTF-16 decoding and validation
//! into the working buffer.
//!
//! The buffers keep Go's sizes (a 512-byte raw buffer decoded into a 1536-byte working buffer)
//! because decoding happens chunk by chunk as the scanner asks for characters: an invalid byte
//! is only reported once the scanner needs the chunk holding it, so the sizes decide which error
//! wins and whether a later document's bad bytes are seen at all. After a reader error the
//! working buffer is left at its full capacity, as in Go.

use super::yamlh::{Encoding, ErrorType, INPUT_RAW_BUFFER_SIZE, Parser};

/// Go: `bom_UTF8`.
const BOM_UTF8: [u8; 3] = [0xEF, 0xBB, 0xBF];
/// Go: `bom_UTF16LE`.
const BOM_UTF16LE: [u8; 2] = [0xFF, 0xFE];
/// Go: `bom_UTF16BE`.
const BOM_UTF16BE: [u8; 2] = [0xFE, 0xFF];

/// The outcome of decoding one character of the raw buffer.
pub(super) enum Decoded {
    /// The code point and the number of raw bytes it took.
    Char(u32, usize),
    /// The character is incomplete and more input may follow (Go: `break inner`).
    Incomplete,
    /// The input is invalid; the reader error is set.
    Invalid,
}

/// Go's `byte(v)`: the low eight bits.
const fn low_byte(v: u32) -> u8 {
    let [low, ..] = v.to_le_bytes();
    low
}

/// The reader functions of the parser (Go: `readerc.go`).
pub(super) trait Reader {
    /// Go: `yaml_parser_set_reader_error`.
    fn set_reader_error(&mut self, problem: &str) -> Decoded;

    /// Go: `parser.raw_buffer[i]`.
    fn raw_at(&self, i: usize) -> u8;

    /// Go: `yaml_parser_determine_encoding`.
    fn determine_encoding(&mut self);

    /// Go: `yaml_parser_update_raw_buffer` with `yaml_string_read_handler`, which cannot fail.
    fn update_raw_buffer(&mut self);

    /// Writes one byte of the working buffer (Go: `parser.buffer[i] = b`, which panics past
    /// the buffer's capacity; the buffer grows instead).
    fn put(&mut self, i: usize, b: u8);

    /// Go: `yaml_parser_update_buffer`: ensures `length` characters are unread, decoding the
    /// raw input chunk by chunk.
    fn update_buffer(&mut self, length: i64) -> bool;

    /// Writes `value` as UTF-8 at `at`, returning the new length.
    fn put_char(&mut self, at: usize, value: u32) -> usize;

    /// Decodes one UTF-8 character of the raw buffer (RFC 3629, without surrogates).
    fn decode_utf8(&mut self, raw_unread: usize) -> Decoded;

    /// Decodes one UTF-16 character of the raw buffer; `low` and `high` are the byte offsets of
    /// the low and high halves of a code unit.
    fn decode_utf16(&mut self, raw_unread: usize, low: usize, high: usize) -> Decoded;
}

impl Reader for Parser<'_> {
    /// Go: `yaml_parser_set_reader_error`.
    fn set_reader_error(&mut self, problem: &str) -> Decoded {
        self.error = ErrorType::Reader;
        problem.clone_into(&mut self.problem);
        Decoded::Invalid
    }

    /// Go: `parser.raw_buffer[i]`.
    fn raw_at(&self, i: usize) -> u8 {
        if i < self.raw_buffer_len {
            self.raw_buffer.get(i).copied().unwrap_or(0)
        } else {
            0
        }
    }

    /// Go: `yaml_parser_determine_encoding`.
    fn determine_encoding(&mut self) {
        // Ensure that we had enough bytes in the raw buffer.
        while !self.eof && self.raw_buffer_len - self.raw_buffer_pos < 3 {
            self.update_raw_buffer();
        }

        // Determine the encoding.
        let pos = self.raw_buffer_pos;
        let avail = self.raw_buffer_len - pos;
        let b0 = self.raw_at(pos);
        let b1 = self.raw_at(pos + 1);
        let b2 = self.raw_at(pos + 2);
        if avail >= 2 && [b0, b1] == BOM_UTF16LE {
            self.encoding = Encoding::Utf16Le;
            self.raw_buffer_pos += 2;
        } else if avail >= 2 && [b0, b1] == BOM_UTF16BE {
            self.encoding = Encoding::Utf16Be;
            self.raw_buffer_pos += 2;
        } else if avail >= 3 && [b0, b1, b2] == BOM_UTF8 {
            self.encoding = Encoding::Utf8;
            self.raw_buffer_pos += 3;
        } else {
            self.encoding = Encoding::Utf8;
        }
    }

    /// Go: `yaml_parser_update_raw_buffer` with `yaml_string_read_handler`, which cannot fail.
    fn update_raw_buffer(&mut self) {
        // Return if the raw buffer is full.
        if self.raw_buffer_pos == 0 && self.raw_buffer_len == self.raw_buffer.len() {
            return;
        }

        // Return on EOF.
        if self.eof {
            return;
        }

        // Move the remaining bytes in the raw buffer to the beginning.
        if self.raw_buffer_pos > 0 && self.raw_buffer_pos < self.raw_buffer_len {
            self.raw_buffer
                .copy_within(self.raw_buffer_pos..self.raw_buffer_len, 0);
        }
        self.raw_buffer_len -= self.raw_buffer_pos;
        self.raw_buffer_pos = 0;

        // Call the read handler to fill the buffer.
        if self.input_pos == self.input.len() {
            self.eof = true;
            return;
        }
        let rest = self.input.get(self.input_pos..).unwrap_or_default();
        let n = rest.len().min(INPUT_RAW_BUFFER_SIZE - self.raw_buffer_len);
        if let (Some(dst), Some(src)) = (
            self.raw_buffer
                .get_mut(self.raw_buffer_len..self.raw_buffer_len + n),
            rest.get(..n),
        ) {
            dst.copy_from_slice(src);
        }
        self.input_pos += n;
        self.raw_buffer_len += n;
    }

    /// Writes one byte of the working buffer (Go: `parser.buffer[i] = b`, which panics past
    /// the buffer's capacity; the buffer grows instead).
    fn put(&mut self, i: usize, b: u8) {
        if i >= self.buffer.len() {
            self.buffer.resize(i + 1, 0);
        }
        if let Some(slot) = self.buffer.get_mut(i) {
            *slot = b;
        }
    }

    /// Go: `yaml_parser_update_buffer`: ensures `length` characters are unread, decoding the
    /// raw input chunk by chunk.
    fn update_buffer(&mut self, length: i64) -> bool {
        // Return if the buffer contains enough characters.
        if self.unread >= length {
            return true;
        }

        // Determine the input encoding if it is not known yet.
        if self.encoding == Encoding::Any {
            self.determine_encoding();
        }

        // Move the unread characters to the beginning of the buffer.
        let mut buffer_len = self.buffer_len;
        if self.buffer_pos > 0 && self.buffer_pos < buffer_len {
            self.buffer.copy_within(self.buffer_pos..buffer_len, 0);
            buffer_len -= self.buffer_pos;
            self.buffer_pos = 0;
        } else if self.buffer_pos == buffer_len {
            buffer_len = 0;
            self.buffer_pos = 0;
        }

        // Open the whole buffer for writing, and cut it before returning.
        self.buffer_len = self.buffer.len();

        // Fill the buffer until it has enough characters.
        let mut first = true;
        while self.unread < length {
            // Fill the raw buffer if necessary.
            if !first || self.raw_buffer_pos == self.raw_buffer_len {
                self.update_raw_buffer();
            }
            first = false;

            // Decode the raw buffer.
            while self.raw_buffer_pos != self.raw_buffer_len {
                let raw_unread = self.raw_buffer_len - self.raw_buffer_pos;
                let decoded = match self.encoding {
                    Encoding::Utf16Le => self.decode_utf16(raw_unread, 0, 1),
                    Encoding::Utf16Be => self.decode_utf16(raw_unread, 1, 0),
                    Encoding::Utf8 | Encoding::Any => self.decode_utf8(raw_unread),
                };
                let (value, width) = match decoded {
                    Decoded::Char(value, width) => (value, width),
                    // The buffer is left open at its full capacity, as in Go.
                    Decoded::Invalid => return false,
                    // An incomplete character: read more raw input.
                    Decoded::Incomplete => break,
                };

                // Check if the character is in the allowed range:
                //      #x9 | #xA | #xD | [#x20-#x7E]               (8 bit)
                //      | #x85 | [#xA0-#xD7FF] | [#xE000-#xFFFD]    (16 bit)
                //      | [#x10000-#x10FFFF]                        (32 bit)
                if !matches!(
                    value,
                    0x09 | 0x0A
                        | 0x0D
                        | 0x20..=0x7E
                        | 0x85
                        | 0xA0..=0xD7FF
                        | 0xE000..=0xFFFD
                        | 0x10000..=0x10_FFFF
                ) {
                    let _ = self.set_reader_error("control characters are not allowed");
                    return false;
                }

                // Move the raw pointers.
                self.raw_buffer_pos += width;

                // Finally put the character into the buffer.
                buffer_len = self.put_char(buffer_len, value);
                self.unread += 1;
            }

            // On EOF, put NUL into the buffer and return.
            if self.eof {
                self.put(buffer_len, 0);
                buffer_len += 1;
                self.unread += 1;
                break;
            }
        }
        // [Go] To return true, the buffer must hold the given length (compared in bytes).
        while i64::try_from(buffer_len).unwrap_or(i64::MAX) < length {
            self.put(buffer_len, 0);
            buffer_len += 1;
        }
        self.buffer_len = buffer_len;
        true
    }

    /// Writes `value` as UTF-8 at `at`, returning the new length.
    fn put_char(&mut self, at: usize, value: u32) -> usize {
        if value <= 0x7F {
            self.put(at, low_byte(value));
            at + 1
        } else if value <= 0x7FF {
            self.put(at, low_byte(0xC0 + (value >> 6)));
            self.put(at + 1, low_byte(0x80 + (value & 0x3F)));
            at + 2
        } else if value <= 0xFFFF {
            self.put(at, low_byte(0xE0 + (value >> 12)));
            self.put(at + 1, low_byte(0x80 + ((value >> 6) & 0x3F)));
            self.put(at + 2, low_byte(0x80 + (value & 0x3F)));
            at + 3
        } else {
            self.put(at, low_byte(0xF0 + (value >> 18)));
            self.put(at + 1, low_byte(0x80 + ((value >> 12) & 0x3F)));
            self.put(at + 2, low_byte(0x80 + ((value >> 6) & 0x3F)));
            self.put(at + 3, low_byte(0x80 + (value & 0x3F)));
            at + 4
        }
    }

    /// Decodes one UTF-8 character of the raw buffer (RFC 3629, without surrogates).
    fn decode_utf8(&mut self, raw_unread: usize) -> Decoded {
        let pos = self.raw_buffer_pos;
        let octet = self.raw_at(pos);
        // Determine the length of the UTF-8 sequence.
        let width = if octet & 0x80 == 0x00 {
            1
        } else if octet & 0xE0 == 0xC0 {
            2
        } else if octet & 0xF0 == 0xE0 {
            3
        } else if octet & 0xF8 == 0xF0 {
            4
        } else {
            // The leading octet is invalid.
            return self.set_reader_error("invalid leading UTF-8 octet");
        };

        // Check if the raw buffer contains an incomplete character.
        if width > raw_unread {
            if self.eof {
                return self.set_reader_error("incomplete UTF-8 octet sequence");
            }
            return Decoded::Incomplete;
        }

        // Decode the leading octet.
        let mut value = u32::from(match width {
            1 => octet & 0x7F,
            2 => octet & 0x1F,
            3 => octet & 0x0F,
            _ => octet & 0x07,
        });

        // Check and decode the trailing octets.
        for k in 1..width {
            let trailing = self.raw_at(pos + k);
            // Check if the octet is valid.
            if trailing & 0xC0 != 0x80 {
                return self.set_reader_error("invalid trailing UTF-8 octet");
            }
            // Decode the octet.
            value = (value << 6) + u32::from(trailing & 0x3F);
        }

        // Check the length of the sequence against the value.
        let shortest = match width {
            1 => true,
            2 => value >= 0x80,
            3 => value >= 0x800,
            _ => value >= 0x10000,
        };
        if !shortest {
            return self.set_reader_error("invalid length of a UTF-8 sequence");
        }

        // Check the range of the value.
        if (0xD800..=0xDFFF).contains(&value) || value > 0x10_FFFF {
            return self.set_reader_error("invalid Unicode character");
        }
        Decoded::Char(value, width)
    }

    /// Decodes one UTF-16 character of the raw buffer; `low` and `high` are the byte offsets of
    /// the low and high halves of a code unit.
    fn decode_utf16(&mut self, raw_unread: usize, low: usize, high: usize) -> Decoded {
        let pos = self.raw_buffer_pos;
        // Check for incomplete UTF-16 character.
        if raw_unread < 2 {
            if self.eof {
                return self.set_reader_error("incomplete UTF-16 character");
            }
            return Decoded::Incomplete;
        }

        // Get the character.
        let value = u32::from(self.raw_at(pos + low)) + (u32::from(self.raw_at(pos + high)) << 8);

        // Check for unexpected low surrogate area.
        if value & 0xFC00 == 0xDC00 {
            return self.set_reader_error("unexpected low surrogate area");
        }

        // Check for a high surrogate area.
        if value & 0xFC00 != 0xD800 {
            return Decoded::Char(value, 2);
        }

        // Check for incomplete surrogate pair.
        if raw_unread < 4 {
            if self.eof {
                return self.set_reader_error("incomplete UTF-16 surrogate pair");
            }
            return Decoded::Incomplete;
        }

        // Get the next character.
        let value2 =
            u32::from(self.raw_at(pos + low + 2)) + (u32::from(self.raw_at(pos + high + 2)) << 8);

        // Check for a low surrogate area.
        if value2 & 0xFC00 != 0xDC00 {
            return self.set_reader_error("expected low surrogate area");
        }

        // Generate the value of the surrogate pair.
        Decoded::Char(0x10000 + ((value & 0x3FF) << 10) + (value2 & 0x3FF), 4)
    }
}
