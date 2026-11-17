//! The scanner (Go: `scannerc.go`): turns the character stream into tokens.
//!
//! The port keeps Go's control flow, including its quirks: the token queue is filled until
//! three tokens are queued and the head token cannot start a simple key (so errors two tokens
//! ahead, possibly in the next document, are reported), `is_bom` tests the start of the working
//! buffer rather than the current position, and failures to fill the buffer while peeking for
//! comments are ignored. Comments are attributed exactly as Go does (see [`super::yamlh::Comment`]
//! for why).

use std::mem;

use super::{
    readerc::Reader as _,
    yamlh::{Comment, ErrorType, Mark, Parser, ScalarStyle, SimpleKey, Token, TokenRef, TokenType},
    yamlprivateh::width,
};

/// The blanks scanned between two runs of a flow or plain scalar's text (Go: the
/// `whitespaces`, `leading_break` and `trailing_breaks` buffers).
#[derive(Debug, Default)]
pub(super) struct Blanks {
    /// Spaces and tabs on the current line.
    whitespaces: Vec<u8>,
    /// The first line break.
    leading_break: Vec<u8>,
    /// The line breaks after the first.
    trailing_breaks: Vec<u8>,
}

impl Blanks {
    /// Go: "Join the whitespaces or fold line breaks": appends the blanks to `s`, folding a
    /// single line break into a space, and clears them.
    fn join(&mut self, s: &mut Vec<u8>, leading_blanks: bool) {
        if leading_blanks {
            // Do we need to fold line breaks?
            if self.leading_break.first() == Some(&b'\n') {
                if self.trailing_breaks.is_empty() {
                    s.push(b' ');
                } else {
                    s.extend_from_slice(&self.trailing_breaks);
                }
            } else {
                s.extend_from_slice(&self.leading_break);
                s.extend_from_slice(&self.trailing_breaks);
            }
            self.trailing_breaks.clear();
            self.leading_break.clear();
        } else {
            s.extend_from_slice(&self.whitespaces);
            self.whitespaces.clear();
        }
    }
}

/// The comment being scanned by `yaml_parser_scan_comments` (Go: its locals).
#[derive(Debug)]
struct CommentScan {
    /// Go: `scan_mark`.
    scan_mark: Mark,
    /// Go: `token_mark`.
    token_mark: Mark,
    /// Go: `start_mark`.
    start_mark: Mark,
    /// Go: `line`.
    line: i64,
    /// Go: `column`.
    column: i64,
    /// Go: `text`.
    text: Vec<u8>,
}

impl CommentScan {
    /// Ends the text so far as a foot comment at `index` (Go: the appends of `foot: text`).
    fn split_foot(&mut self, comments: &mut Vec<Comment>, index: i64) {
        let end = Mark {
            index,
            line: self.line,
            column: self.column,
        };
        comments.push(Comment {
            scan_mark: self.scan_mark,
            token_mark: self.token_mark,
            start_mark: self.start_mark,
            end_mark: end,
            ..Comment::default()
        });
        self.text.clear();
        self.scan_mark = end;
        self.token_mark = end;
    }
}

/// Go: `max_flow_level`.
const MAX_FLOW_LEVEL: i64 = 10000;

/// Go: `max_indents`.
const MAX_INDENTS: usize = 10000;

/// Go: `max_number_length`, the longest `%YAML` version number.
const MAX_NUMBER_LENGTH: i8 = 2;

/// Converts a small buffer offset to a Go `int`.
fn as_int(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// `unsafe.Sizeof(yaml_token_t{})` on 64-bit platforms.
const TOKEN_SIZE: usize = 144;

/// Go: `runtime.nextslicecap`.
const fn next_slice_cap(new_len: usize, old_cap: usize) -> usize {
    const THRESHOLD: usize = 256;
    let mut newcap = old_cap;
    let doublecap = newcap.saturating_add(newcap);
    if new_len > doublecap {
        return new_len;
    }
    if old_cap < THRESHOLD {
        return doublecap;
    }
    while newcap < new_len {
        // Transition from growing 2x for small slices to growing 1.25x for large slices.
        newcap = newcap.saturating_add((newcap + 3 * THRESHOLD) >> 2);
    }
    newcap
}

/// Go: `runtime.SizeClassToSize`, the size classes of small allocations.
const SIZE_CLASSES: [usize; 67] = [
    8, 16, 24, 32, 48, 64, 80, 96, 112, 128, 144, 160, 176, 192, 208, 224, 240, 256, 288, 320, 352,
    384, 416, 448, 480, 512, 576, 640, 704, 768, 896, 1024, 1152, 1280, 1408, 1536, 1792, 2048,
    2304, 2688, 3072, 3200, 3456, 4096, 4864, 5376, 6144, 6528, 6784, 6912, 8192, 9472, 9728,
    10240, 10880, 12288, 13568, 14336, 16384, 18432, 19072, 20480, 21760, 24576, 27264, 28672,
    32768,
];

/// Go: `runtime.roundupsize(size, false)`: the usable size of an allocation of pointerful
/// memory.
fn roundup_size(size: usize) -> usize {
    const MALLOC_HEADER_SIZE: usize = 8;
    const MIN_SIZE_FOR_MALLOC_HEADER: usize = 512;
    const MAX_SMALL_SIZE: usize = 32768;
    const PAGE_SIZE: usize = 8192;
    if size <= MAX_SMALL_SIZE - MALLOC_HEADER_SIZE {
        let header = if size > MIN_SIZE_FOR_MALLOC_HEADER {
            MALLOC_HEADER_SIZE
        } else {
            0
        };
        let req = size + header;
        let class = SIZE_CLASSES
            .iter()
            .copied()
            .find(|&c| c >= req)
            .unwrap_or(req);
        return class - header;
    }
    size.checked_add(PAGE_SIZE - 1)
        .map_or(size, |s| s & !(PAGE_SIZE - 1))
}

/// The capacity Go's `append` gives a full token slice of capacity `old_cap` (Go: `growslice`).
fn grown_token_cap(old_cap: usize, new_len: usize) -> usize {
    let newcap = next_slice_cap(new_len, old_cap);
    roundup_size(newcap.saturating_mul(TOKEN_SIZE)) / TOKEN_SIZE
}

/// The scanner functions of the parser (Go: `scannerc.go`).
pub(super) trait Scanner {
    /// Go: `cache`: ensures `length` characters are buffered.
    fn cache(&mut self, length: i64) -> bool;

    /// Go: `skip`: advances past the current character.
    fn skip(&mut self);

    /// Go: `skip_line`: advances past the current line break.
    fn skip_line(&mut self);

    /// Go: `read`: copies the current character to `s` and advances.
    fn read_char(&mut self, s: &mut Vec<u8>);

    /// Go: `read_line`: copies the current line break to `s` (normalized to `\n` except for
    /// LS and PS) and advances.
    fn read_line(&mut self, s: &mut Vec<u8>);

    /// Go: `yaml_parser_set_scanner_error`. The context text is not kept: `yaml.Unmarshal`
    /// only reports the problem and a line.
    fn set_scanner_error(&mut self, context_mark: Mark, problem: &str) -> bool;

    /// Go: `yaml_insert_token`: inserts at `pos` of the queue, or appends when `pos < 0`.
    ///
    /// Go's token slice keeps the consumed tokens until the slice is full, and the scanner's
    /// look-behind reads them, so the slice's capacity is tracked as `append` grows it.
    fn insert_token(&mut self, pos: i64, token: Token);

    /// Go: `yaml_parser_fetch_more_tokens`: ensures the tokens queue holds the next token.
    fn fetch_more_tokens(&mut self) -> bool;

    /// Go: `yaml_parser_fetch_next_token`.
    fn fetch_next_token(&mut self) -> bool;

    /// The token dispatch of `yaml_parser_fetch_next_token` after the document indicators.
    fn fetch_token(&mut self, pos: usize) -> bool;

    /// Go: `yaml_simple_key_is_valid`: `None` on error, otherwise whether the key at `i` of
    /// the simple keys stack is still possible.
    fn simple_key_is_valid(&mut self, i: usize) -> Option<bool>;

    /// Go: `yaml_parser_save_simple_key`: records that a simple key may start here.
    fn save_simple_key(&mut self) -> bool;

    /// Go: `yaml_parser_remove_simple_key`: removes a potential simple key at the current
    /// flow level.
    fn remove_simple_key(&mut self) -> bool;

    /// Go: `yaml_parser_increase_flow_level`.
    fn increase_flow_level(&mut self) -> bool;

    /// Go: `yaml_parser_decrease_flow_level`.
    fn decrease_flow_level(&mut self);

    /// Go: `yaml_parser_roll_indent`: pushes the indentation level and appends a
    /// BLOCK-SEQUENCE-START or BLOCK-MAPPING-START token if needed.
    fn roll_indent(&mut self, column: i64, number: i64, typ: TokenType, mark: Mark) -> bool;

    /// Go: `yaml_parser_unroll_indent`: pops indentation levels deeper than `column`,
    /// appending a BLOCK-END token for each.
    fn unroll_indent(&mut self, column: i64, scan_mark: Mark);

    /// Go: `yaml_parser_fetch_stream_start`.
    fn fetch_stream_start(&mut self);

    /// Go: `yaml_parser_fetch_stream_end`.
    fn fetch_stream_end(&mut self) -> bool;

    /// Go: `yaml_parser_fetch_directive`.
    fn fetch_directive(&mut self) -> bool;

    /// Go: `yaml_parser_fetch_document_indicator`.
    fn fetch_document_indicator(&mut self, typ: TokenType) -> bool;

    /// Go: `yaml_parser_fetch_flow_collection_start`.
    fn fetch_flow_collection_start(&mut self, typ: TokenType) -> bool;

    /// Go: `yaml_parser_fetch_flow_collection_end`.
    fn fetch_flow_collection_end(&mut self, typ: TokenType) -> bool;

    /// Go: `yaml_parser_fetch_flow_entry`.
    fn fetch_flow_entry(&mut self) -> bool;

    /// Go: `yaml_parser_fetch_block_entry`.
    fn fetch_block_entry(&mut self) -> bool;

    /// Go: `yaml_parser_fetch_key`.
    fn fetch_key(&mut self) -> bool;

    /// Go: `yaml_parser_fetch_value`.
    fn fetch_value(&mut self) -> bool;

    /// Go: `yaml_parser_fetch_anchor`.
    fn fetch_anchor(&mut self, typ: TokenType) -> bool;

    /// Go: `yaml_parser_fetch_tag`.
    fn fetch_tag(&mut self) -> bool;

    /// Go: `yaml_parser_fetch_block_scalar`.
    fn fetch_block_scalar(&mut self, literal: bool) -> bool;

    /// Go: `yaml_parser_fetch_flow_scalar`.
    fn fetch_flow_scalar(&mut self, single: bool) -> bool;

    /// Go: `yaml_parser_fetch_plain_scalar`.
    fn fetch_plain_scalar(&mut self) -> bool;

    /// Go: `yaml_parser_scan_to_next_token`: eats whitespaces, comments and line breaks.
    fn scan_to_next_token(&mut self) -> bool;

    /// Go: `yaml_parser_scan_directive`.
    ///
    /// Scope:
    ///      %YAML    1.1    # a comment \n
    ///      ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    ///      %TAG    !yaml!  tag:yaml.org,2002:  \n
    ///      ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    fn scan_directive(&mut self) -> Option<Token>;

    /// Go: `yaml_parser_scan_directive_name`.
    ///
    /// Scope:
    ///      %YAML   1.1     # a comment \n
    ///       ^^^^
    ///      %TAG    !yaml!  tag:yaml.org,2002:  \n
    ///       ^^^
    fn scan_directive_name(&mut self, start_mark: Mark) -> Option<Vec<u8>>;

    /// Go: `yaml_parser_scan_version_directive_value`.
    ///
    /// Scope:
    ///      %YAML   1.1     # a comment \n
    ///           ^^^^^^
    fn scan_version_directive_value(&mut self, start_mark: Mark) -> Option<(i8, i8)>;

    /// Go: `yaml_parser_scan_version_directive_number`.
    ///
    /// Scope:
    ///      %YAML   1.1     # a comment \n
    ///              ^
    ///      %YAML   1.1     # a comment \n
    ///                ^
    fn scan_version_directive_number(&mut self, start_mark: Mark) -> Option<i8>;

    /// Go: `yaml_parser_scan_tag_directive_value`.
    ///
    /// Scope:
    ///      %TAG    !yaml!  tag:yaml.org,2002:  \n
    ///          ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    fn scan_tag_directive_value(&mut self, start_mark: Mark) -> Option<(Vec<u8>, Vec<u8>)>;

    /// Go: `yaml_parser_scan_anchor`.
    fn scan_anchor(&mut self, typ: TokenType) -> Option<Token>;

    /// Go: `yaml_parser_scan_tag`.
    fn scan_tag(&mut self) -> Option<Token>;

    /// Go: `yaml_parser_set_scanner_tag_error`. The context ("while parsing a tag" or "while
    /// parsing a %TAG directive") is not kept.
    fn set_scanner_tag_error(&mut self, context_mark: Mark, problem: &str) -> bool;

    /// Go: `yaml_parser_scan_tag_handle`.
    fn scan_tag_handle(&mut self, directive: bool, start_mark: Mark) -> Option<Vec<u8>>;

    /// Go: `yaml_parser_scan_tag_uri`.
    fn scan_tag_uri(&mut self, directive: bool, head: &[u8], start_mark: Mark) -> Option<Vec<u8>>;

    /// Go: `yaml_parser_scan_uri_escapes`: decodes a URI-escaped UTF-8 sequence.
    fn scan_uri_escapes(&mut self, _directive: bool, start_mark: Mark, s: &mut Vec<u8>) -> bool;

    /// Go: `yaml_parser_scan_block_scalar`.
    fn scan_block_scalar(&mut self, literal: bool) -> Option<Token>;

    /// The header of a block scalar (Go: the start of `yaml_parser_scan_block_scalar`): the
    /// chomping and indentation indicators, then blanks and a comment to the end of the line,
    /// and its line break. Returns `(chomping, increment)`, or `None` once an error is set.
    fn scan_block_scalar_header(&mut self, start_mark: Mark) -> Option<(i64, i64)>;

    /// Go: `yaml_parser_scan_block_scalar_breaks`: scans indentation spaces and line breaks
    /// for a block scalar, determining the indentation level if needed.
    fn scan_block_scalar_breaks(
        &mut self,
        indent: &mut i64,
        breaks: &mut Vec<u8>,
        start_mark: Mark,
        end_mark: &mut Mark,
    ) -> bool;

    /// Go: `yaml_parser_scan_flow_scalar`: scans a single- or double-quoted scalar.
    fn scan_flow_scalar(&mut self, single: bool) -> Option<Token>;

    /// The non-blank characters of a quoted scalar line (Go: the "Consume non-blank
    /// characters" loop of `yaml_parser_scan_flow_scalar`). Returns whether an escaped line
    /// break ended them (Go's `leading_blanks`), or `None` once an error is set.
    fn scan_flow_scalar_text(
        &mut self,
        single: bool,
        start_mark: Mark,
        s: &mut Vec<u8>,
    ) -> Option<bool>;

    /// The "Consume blank characters" loop of `yaml_parser_scan_flow_scalar` and
    /// `yaml_parser_scan_plain_scalar`: the spaces, tabs and line breaks after a run of text.
    /// `tab_limit` is the plain scalar's check for tabs that abuse indentation (its indentation
    /// and start mark). Returns the updated `leading_blanks`, or `None` once an error is set.
    fn scan_scalar_blanks(
        &mut self,
        blanks: &mut Blanks,
        leading_blanks: bool,
        tab_limit: Option<(i64, Mark)>,
    ) -> Option<bool>;

    /// The escape sequence branch of `yaml_parser_scan_flow_scalar`.
    fn scan_escape(&mut self, start_mark: Mark, s: &mut Vec<u8>) -> bool;

    /// Go: `yaml_parser_scan_plain_scalar`.
    fn scan_plain_scalar(&mut self) -> Option<Token>;

    /// Go: `yaml_parser_scan_line_comment`: consumes a comment that follows a token on its
    /// line.
    fn scan_line_comment(&mut self, token_mark: Mark) -> bool;

    /// The foot line of `yaml_parser_scan_comments`.
    fn comment_foot_line(&self, scan_mark_in: Mark) -> i64;

    /// Go: `yaml_parser_scan_comments`: consumes the comment lines (and the blank lines
    /// between them) that start at the current position, splitting them into foot comments of
    /// the prior token and a head comment of the next one.
    fn scan_comments(&mut self, scan_mark_in: Mark) -> bool;

    /// The token that comments scanned now follow (Go: the start of
    /// `yaml_parser_scan_comments`): the last token, or the one before a trailing FLOW-ENTRY.
    fn comment_prior_token(&self) -> TokenRef;

    /// The "Consume until after the consumed comment line" loop of
    /// `yaml_parser_scan_comments`: skips to the index `seen`, then reads the comment text up
    /// to the line break into `text`.
    fn consume_comment_line(&mut self, seen: i64, text: &mut Vec<u8>) -> bool;
}

impl Scanner for Parser<'_> {
    /// Go: `cache`: ensures `length` characters are buffered.
    fn cache(&mut self, length: i64) -> bool {
        self.unread >= length || self.update_buffer(length)
    }

    /// Go: `skip`: advances past the current character.
    fn skip(&mut self) {
        if !self.is_blank(self.buffer_pos) {
            self.newlines = 0;
        }
        self.mark.index += 1;
        self.mark.column += 1;
        self.unread -= 1;
        // Go advances by `width`, which is never 0 for a decoded character.
        self.buffer_pos += width(self.at(self.buffer_pos)).max(1);
    }

    /// Go: `skip_line`: advances past the current line break.
    fn skip_line(&mut self) {
        if self.is_crlf(self.buffer_pos) {
            self.mark.index += 2;
            self.mark.column = 0;
            self.mark.line += 1;
            self.unread -= 2;
            self.buffer_pos += 2;
            self.newlines += 1;
        } else if self.is_break(self.buffer_pos) {
            self.mark.index += 1;
            self.mark.column = 0;
            self.mark.line += 1;
            self.unread -= 1;
            self.buffer_pos += width(self.at(self.buffer_pos)).max(1);
            self.newlines += 1;
        }
    }

    /// Go: `read`: copies the current character to `s` and advances.
    fn read_char(&mut self, s: &mut Vec<u8>) {
        if !self.is_blank(self.buffer_pos) {
            self.newlines = 0;
        }
        // Go panics on a zero width, which a decoded buffer never holds.
        let w = width(self.at(self.buffer_pos)).max(1);
        for k in 0..w {
            s.push(self.at(self.buffer_pos + k));
        }
        self.buffer_pos += w;
        self.mark.index += 1;
        self.mark.column += 1;
        self.unread -= 1;
    }

    /// Go: `read_line`: copies the current line break to `s` (normalized to `\n` except for
    /// LS and PS) and advances.
    fn read_line(&mut self, s: &mut Vec<u8>) {
        let pos = self.buffer_pos;
        let (b0, b1, b2) = (self.at(pos), self.at(pos + 1), self.at(pos + 2));
        if b0 == b'\r' && b1 == b'\n' {
            // CR LF . LF
            s.push(b'\n');
            self.buffer_pos += 2;
            self.mark.index += 1;
            self.unread -= 1;
        } else if b0 == b'\r' || b0 == b'\n' {
            // CR|LF . LF
            s.push(b'\n');
            self.buffer_pos += 1;
        } else if b0 == 0xC2 && b1 == 0x85 {
            // NEL . LF
            s.push(b'\n');
            self.buffer_pos += 2;
        } else if b0 == 0xE2 && b1 == 0x80 && (b2 == 0xA8 || b2 == 0xA9) {
            // LS|PS . LS|PS
            s.extend_from_slice(&[b0, b1, b2]);
            self.buffer_pos += 3;
        } else {
            return;
        }
        self.mark.index += 1;
        self.mark.column = 0;
        self.mark.line += 1;
        self.unread -= 1;
        self.newlines += 1;
    }

    /// Go: `yaml_parser_set_scanner_error`. The context text is not kept: `yaml.Unmarshal`
    /// only reports the problem and a line.
    fn set_scanner_error(&mut self, context_mark: Mark, problem: &str) -> bool {
        self.error = ErrorType::Scanner;
        self.context_mark = context_mark;
        problem.clone_into(&mut self.problem);
        self.problem_mark = self.mark;
        false
    }

    /// Go: `yaml_insert_token`: inserts at `pos` of the queue, or appends when `pos < 0`.
    ///
    /// Go's token slice keeps the consumed tokens until the slice is full, and the scanner's
    /// look-behind reads them, so the slice's capacity is tracked as `append` grows it.
    fn insert_token(&mut self, pos: i64, token: Token) {
        // Check if we can move the queue at the beginning of the buffer.
        let len = self.tokens_head + self.tokens.len();
        if self.tokens_head > 0 && len == self.tokens_cap {
            self.tokens_head = 0;
        } else if len == self.tokens_cap {
            self.tokens_cap = grown_token_cap(self.tokens_cap, len + 1);
        }
        match usize::try_from(pos) {
            Ok(pos) if pos <= self.tokens.len() => self.tokens.insert(pos, token),
            _ => self.tokens.push_back(token),
        }
    }

    /// Go: `yaml_parser_fetch_more_tokens`: ensures the tokens queue holds the next token.
    fn fetch_more_tokens(&mut self) -> bool {
        // While we need more tokens to fetch, do it.
        loop {
            // [Go] The comment parsing logic requires a lookahead of two tokens so that foot
            // comments may be parsed in time of associating them with the tokens that are
            // parsed before them, and also for line comments.
            //
            // The simple key logic is also required here so that the parser can be sure the
            // next token is not a simple key.
            if self.tokens.len() > 2 {
                let Some(&i) = self.simple_keys_by_tok.get(&self.tokens_parsed) else {
                    break;
                };
                match self.simple_key_is_valid(i) {
                    None => return false,
                    Some(false) => break,
                    Some(true) => {}
                }
            }
            // Fetch the next token.
            if !self.fetch_next_token() {
                return false;
            }
        }
        self.token_available = true;
        true
    }

    /// Go: `yaml_parser_fetch_next_token`.
    fn fetch_next_token(&mut self) -> bool {
        // Ensure that the buffer is initialized.
        if !self.cache(1) {
            return false;
        }

        // Check if we just started scanning.  Fetch STREAM-START then.
        if !self.stream.start_produced {
            self.fetch_stream_start();
            return true;
        }

        let scan_mark = self.mark;

        // Eat whitespaces and comments until we reach the next token.
        if !self.scan_to_next_token() {
            return false;
        }

        // [Go] While unrolling indents, transform the head comments of prior indentation
        // levels observed after scan_start into foot comments at the respective indexes.

        // Check the indentation level against the current column.
        self.unroll_indent(self.mark.column, scan_mark);

        // Ensure that the buffer contains at least 4 characters.  4 is the length of the
        // longest indicators ('--- ' and '... ').
        if !self.cache(4) {
            return false;
        }

        let pos = self.buffer_pos;

        // Is it the end of the stream?
        if self.is_z(pos) {
            return self.fetch_stream_end();
        }

        // Is it a directive?
        if self.mark.column == 0 && self.at(pos) == b'%' {
            return self.fetch_directive();
        }

        // Is it the document start indicator?
        if self.mark.column == 0
            && self.at(pos) == b'-'
            && self.at(pos + 1) == b'-'
            && self.at(pos + 2) == b'-'
            && self.is_blankz(pos + 3)
        {
            return self.fetch_document_indicator(TokenType::DocumentStart);
        }

        // Is it the document end indicator?
        if self.mark.column == 0
            && self.at(pos) == b'.'
            && self.at(pos + 1) == b'.'
            && self.at(pos + 2) == b'.'
            && self.is_blankz(pos + 3)
        {
            return self.fetch_document_indicator(TokenType::DocumentEnd);
        }

        let mut comment_mark = self.mark;
        if let Some(last) = self.last_token() {
            let c = self.at(pos);
            if (self.flow_level == 0 && c == b':') || (self.flow_level > 0 && c == b',') {
                // Associate any following comments with the prior token.
                comment_mark = last.start_mark;
            }
        }

        if !self.fetch_token(pos) {
            return false;
        }

        // [Go] The deferred line comment scan.
        if self
            .last_token()
            .is_some_and(|t| t.typ == TokenType::BlockEntry)
        {
            // Sequence indicators alone have no line comments. It becomes a head comment for
            // whatever follows.
            return true;
        }
        self.scan_line_comment(comment_mark)
    }

    /// The token dispatch of `yaml_parser_fetch_next_token` after the document indicators.
    fn fetch_token(&mut self, pos: usize) -> bool {
        let c = self.at(pos);
        match c {
            // Is it the flow sequence start indicator?
            b'[' => return self.fetch_flow_collection_start(TokenType::FlowSequenceStart),
            // Is it the flow mapping start indicator?
            b'{' => return self.fetch_flow_collection_start(TokenType::FlowMappingStart),
            // Is it the flow sequence end indicator?
            b']' => return self.fetch_flow_collection_end(TokenType::FlowSequenceEnd),
            // Is it the flow mapping end indicator?
            b'}' => return self.fetch_flow_collection_end(TokenType::FlowMappingEnd),
            // Is it the flow entry indicator?
            b',' => return self.fetch_flow_entry(),
            _ => {}
        }

        // Is it the block entry indicator?
        if c == b'-' && self.is_blankz(pos + 1) {
            return self.fetch_block_entry();
        }

        // Is it the key indicator?
        if c == b'?' && (self.flow_level > 0 || self.is_blankz(pos + 1)) {
            return self.fetch_key();
        }

        // Is it the value indicator?
        if c == b':' && (self.flow_level > 0 || self.is_blankz(pos + 1)) {
            return self.fetch_value();
        }

        match c {
            // Is it an alias?
            b'*' => return self.fetch_anchor(TokenType::Alias),
            // Is it an anchor?
            b'&' => return self.fetch_anchor(TokenType::Anchor),
            // Is it a tag?
            b'!' => return self.fetch_tag(),
            // Is it a literal scalar?
            b'|' if self.flow_level == 0 => return self.fetch_block_scalar(true),
            // Is it a folded scalar?
            b'>' if self.flow_level == 0 => return self.fetch_block_scalar(false),
            // Is it a single-quoted scalar?
            b'\'' => return self.fetch_flow_scalar(true),
            // Is it a double-quoted scalar?
            b'"' => return self.fetch_flow_scalar(false),
            _ => {}
        }

        // Is it a plain scalar?
        //
        // A plain scalar may start with any non-blank characters except
        //
        //      '-', '?', ':', ',', '[', ']', '{', '}',
        //      '#', '&', '*', '!', '|', '>', '\'', '\"',
        //      '%', '@', '`'.
        //
        // In the block context (and, for the '-' indicator, in the flow context too), it may
        // also start with the characters
        //
        //      '-', '?', ':'
        //
        // if it is followed by a non-space character.
        //
        // The last rule is more restrictive than the specification requires.
        // [Go] TODO Make this logic more reasonable.
        // switch parser.buffer[parser.buffer_pos] {
        // case '-', '?', ':', ',', '?', '-', ',', ':', ']', '[', '}', '{', '&', '#', '!', '*', '>',
        // '|', '"', '\'', '@', '%', '-', '`':
        //}
        let indicator = matches!(
            c,
            b'-' | b'?'
                | b':'
                | b','
                | b'['
                | b']'
                | b'{'
                | b'}'
                | b'#'
                | b'&'
                | b'*'
                | b'!'
                | b'|'
                | b'>'
                | b'\''
                | b'"'
                | b'%'
                | b'@'
                | b'`'
        );
        if !(self.is_blankz(pos) || indicator)
            || (c == b'-' && !self.is_blank(pos + 1))
            || (self.flow_level == 0 && (c == b'?' || c == b':') && !self.is_blankz(pos + 1))
        {
            return self.fetch_plain_scalar();
        }

        // If we don't determine the token type so far, it is an error.
        // "while scanning for the next token"
        self.set_scanner_error(self.mark, "found character that cannot start any token")
    }

    /// Go: `yaml_simple_key_is_valid`: `None` on error, otherwise whether the key at `i` of
    /// the simple keys stack is still possible.
    fn simple_key_is_valid(&mut self, i: usize) -> Option<bool> {
        // Go indexes the stack directly (and would panic past its end).
        let key = self.simple_keys.get(i).copied()?;
        if !key.possible {
            return Some(false);
        }

        // The 1.2 specification says:
        //
        //     "If the ? indicator is omitted, parsing needs to see past the
        //     implicit key to recognize it as such. To limit the amount of
        //     lookahead required, the “:” indicator must appear at most 1024
        //     Unicode characters beyond the start of the key. In addition, the key
        //     is restricted to a single line."
        //
        if key.mark.line < self.mark.line || key.mark.index + 1024 < self.mark.index {
            // Check if the potential simple key to be removed is required.
            if key.required {
                // "while scanning a simple key"
                let _ = self.set_scanner_error(key.mark, "could not find expected ':'");
                return None;
            }
            if let Some(k) = self.simple_keys.get_mut(i) {
                k.possible = false;
            }
            return Some(false);
        }
        Some(true)
    }

    /// Go: `yaml_parser_save_simple_key`: records that a simple key may start here.
    fn save_simple_key(&mut self) -> bool {
        // A simple key is required at the current position if the scanner is in the block
        // context and the current column coincides with the indentation level.
        let required = self.flow_level == 0 && self.indent == self.mark.column;

        // If the current position may start a simple key, save it.
        if self.simple_key_allowed {
            let simple_key = SimpleKey {
                possible: true,
                required,
                token_number: self.tokens_parsed + as_int(self.tokens.len()),
                mark: self.mark,
            };

            if !self.remove_simple_key() {
                return false;
            }
            let last = self.simple_keys.len().saturating_sub(1);
            if let Some(slot) = self.simple_keys.get_mut(last) {
                *slot = simple_key;
            }
            let _previous = self
                .simple_keys_by_tok
                .insert(simple_key.token_number, last);
        }
        true
    }

    /// Go: `yaml_parser_remove_simple_key`: removes a potential simple key at the current
    /// flow level.
    fn remove_simple_key(&mut self) -> bool {
        let i = self.simple_keys.len().saturating_sub(1);
        let Some(key) = self.simple_keys.get(i).copied() else {
            return true;
        };
        if key.possible {
            // If the key is required, it is an error.
            if key.required {
                // "while scanning a simple key"
                return self.set_scanner_error(key.mark, "could not find expected ':'");
            }
            // Remove the key from the stack.
            if let Some(k) = self.simple_keys.get_mut(i) {
                k.possible = false;
            }
            let _removed = self.simple_keys_by_tok.remove(&key.token_number);
        }
        true
    }

    /// Go: `yaml_parser_increase_flow_level`.
    fn increase_flow_level(&mut self) -> bool {
        // Reset the simple key on the next level.
        self.simple_keys.push(SimpleKey {
            possible: false,
            required: false,
            token_number: self.tokens_parsed + as_int(self.tokens.len()),
            mark: self.mark,
        });

        // Increase the flow level.
        self.flow_level += 1;
        if self.flow_level > MAX_FLOW_LEVEL {
            let mark = self.simple_keys.last().map_or_default(|k| k.mark);
            // "while increasing flow level"
            return self.set_scanner_error(mark, "exceeded max depth of 10000");
        }
        true
    }

    /// Go: `yaml_parser_decrease_flow_level`.
    fn decrease_flow_level(&mut self) {
        if self.flow_level > 0 {
            self.flow_level -= 1;
            if let Some(key) = self.simple_keys.pop() {
                let _removed = self.simple_keys_by_tok.remove(&key.token_number);
            }
        }
    }

    /// Go: `yaml_parser_roll_indent`: pushes the indentation level and appends a
    /// BLOCK-SEQUENCE-START or BLOCK-MAPPING-START token if needed.
    fn roll_indent(&mut self, column: i64, number: i64, typ: TokenType, mark: Mark) -> bool {
        // In the flow context, do nothing.
        if self.flow_level > 0 {
            return true;
        }

        if self.indent < column {
            // Push the current indentation level to the stack and set the new indentation
            // level.
            self.indents.push(self.indent);
            self.indent = column;
            if self.indents.len() > MAX_INDENTS {
                let key_mark = self.simple_keys.last().map_or_default(|k| k.mark);
                // "while increasing indent level"
                return self.set_scanner_error(key_mark, "exceeded max depth of 10000");
            }

            // Create a token and insert it into the queue.
            let number = if number > -1 {
                number - self.tokens_parsed
            } else {
                number
            };
            self.insert_token(
                number,
                Token {
                    typ,
                    start_mark: mark,
                    end_mark: mark,
                    ..Token::default()
                },
            );
        }
        true
    }

    /// Go: `yaml_parser_unroll_indent`: pops indentation levels deeper than `column`,
    /// appending a BLOCK-END token for each.
    fn unroll_indent(&mut self, column: i64, scan_mark: Mark) {
        // In the flow context, do nothing.
        if self.flow_level > 0 {
            return;
        }

        let mut block_mark = scan_mark;
        block_mark.index -= 1;

        // Loop through the indentation levels in the stack.
        while self.indent > column {
            // [Go] Reposition the end token before potential following foot comments of parent
            // blocks. For that, search backwards for recent comments that were at the same
            // indent as the block that is ending now.
            let mut stop_index = block_mark.index;
            for comment in self.comments.iter().rev() {
                if comment.end_mark.index < stop_index {
                    // Don't go back beyond the start of the comment/whitespace scan, unless
                    // column < 0. If requested indent column is < 0, then the document is over
                    // and everything else is a foot anyway.
                    break;
                }
                if comment.start_mark.column == self.indent + 1 {
                    // This is a good match. But maybe there's a former comment at that same
                    // indent level, so keep searching.
                    block_mark = comment.start_mark;
                }

                // While the end of the former comment matches with the start of the following
                // one, we know there's nothing in between and scanning is still safe.
                stop_index = comment.scan_mark.index;
            }

            // Create a token and append it to the queue.
            self.insert_token(
                -1,
                Token {
                    typ: TokenType::BlockEnd,
                    start_mark: block_mark,
                    end_mark: block_mark,
                    ..Token::default()
                },
            );

            // Pop the indentation level.
            self.indent = self.indents.pop().unwrap_or(-1);
        }
    }

    /// Go: `yaml_parser_fetch_stream_start`.
    fn fetch_stream_start(&mut self) {
        // Set the initial indentation.
        self.indent = -1;

        // Initialize the simple key stack.
        self.simple_keys.push(SimpleKey::default());
        self.simple_keys_by_tok.clear();

        // A simple key is allowed at the beginning of the stream.
        self.simple_key_allowed = true;

        // We have started.
        self.stream.start_produced = true;

        // Create the STREAM-START token and append it to the queue.
        self.insert_token(
            -1,
            Token {
                typ: TokenType::StreamStart,
                start_mark: self.mark,
                end_mark: self.mark,
                ..Token::default()
            },
        );
    }

    /// Go: `yaml_parser_fetch_stream_end`.
    fn fetch_stream_end(&mut self) -> bool {
        // Force new line.
        if self.mark.column != 0 {
            self.mark.column = 0;
            self.mark.line += 1;
        }

        // Reset the indentation level.
        self.unroll_indent(-1, self.mark);

        // Reset simple keys.
        if !self.remove_simple_key() {
            return false;
        }

        self.simple_key_allowed = false;

        // Create the STREAM-END token and append it to the queue.
        self.insert_token(
            -1,
            Token {
                typ: TokenType::StreamEnd,
                start_mark: self.mark,
                end_mark: self.mark,
                ..Token::default()
            },
        );
        true
    }

    /// Go: `yaml_parser_fetch_directive`.
    fn fetch_directive(&mut self) -> bool {
        // Reset the indentation level.
        self.unroll_indent(-1, self.mark);

        // Reset simple keys.
        if !self.remove_simple_key() {
            return false;
        }

        self.simple_key_allowed = false;

        // Create the YAML-DIRECTIVE or TAG-DIRECTIVE token.
        let Some(token) = self.scan_directive() else {
            return false;
        };
        // Append the token to the queue.
        self.insert_token(-1, token);
        true
    }

    /// Go: `yaml_parser_fetch_document_indicator`.
    fn fetch_document_indicator(&mut self, typ: TokenType) -> bool {
        // Reset the indentation level.
        self.unroll_indent(-1, self.mark);

        // Reset simple keys.
        if !self.remove_simple_key() {
            return false;
        }

        self.simple_key_allowed = false;

        // Consume the token.
        let start_mark = self.mark;

        self.skip();
        self.skip();
        self.skip();

        let end_mark = self.mark;

        // Create the DOCUMENT-START or DOCUMENT-END token.
        self.insert_token(
            -1,
            Token {
                typ,
                start_mark,
                end_mark,
                ..Token::default()
            },
        );
        true
    }

    /// Go: `yaml_parser_fetch_flow_collection_start`.
    fn fetch_flow_collection_start(&mut self, typ: TokenType) -> bool {
        // The indicators '[' and '{' may start a simple key.
        if !self.save_simple_key() {
            return false;
        }

        // Increase the flow level.
        if !self.increase_flow_level() {
            return false;
        }

        // A simple key may follow the indicators '[' and '{'.
        self.simple_key_allowed = true;

        // Consume the token.
        let start_mark = self.mark;
        self.skip();
        let end_mark = self.mark;

        // Create the FLOW-SEQUENCE-START of FLOW-MAPPING-START token.
        self.insert_token(
            -1,
            Token {
                typ,
                start_mark,
                end_mark,
                ..Token::default()
            },
        );
        true
    }

    /// Go: `yaml_parser_fetch_flow_collection_end`.
    fn fetch_flow_collection_end(&mut self, typ: TokenType) -> bool {
        // Reset any potential simple key on the current flow level.
        if !self.remove_simple_key() {
            return false;
        }

        // Decrease the flow level.
        self.decrease_flow_level();

        // No simple keys after the indicators ']' and '}'.
        self.simple_key_allowed = false;

        // Consume the token.
        let start_mark = self.mark;
        self.skip();
        let end_mark = self.mark;

        // Create the FLOW-SEQUENCE-END of FLOW-MAPPING-END token.
        self.insert_token(
            -1,
            Token {
                typ,
                start_mark,
                end_mark,
                ..Token::default()
            },
        );
        true
    }

    /// Go: `yaml_parser_fetch_flow_entry`.
    fn fetch_flow_entry(&mut self) -> bool {
        // Reset any potential simple keys on the current flow level.
        if !self.remove_simple_key() {
            return false;
        }

        // Simple keys are allowed after ','.
        self.simple_key_allowed = true;

        // Consume the token.
        let start_mark = self.mark;
        self.skip();
        let end_mark = self.mark;

        // Create the FLOW-ENTRY token and append it to the queue.
        self.insert_token(
            -1,
            Token {
                typ: TokenType::FlowEntry,
                start_mark,
                end_mark,
                ..Token::default()
            },
        );
        true
    }

    /// Go: `yaml_parser_fetch_block_entry`.
    fn fetch_block_entry(&mut self) -> bool {
        // Check if the scanner is in the block context.
        if self.flow_level == 0 {
            // Check if we are allowed to start a new entry.
            if !self.simple_key_allowed {
                return self.set_scanner_error(
                    self.mark,
                    "block sequence entries are not allowed in this context",
                );
            }
            // Add the BLOCK-SEQUENCE-START token if needed.
            if !self.roll_indent(
                self.mark.column,
                -1,
                TokenType::BlockSequenceStart,
                self.mark,
            ) {
                return false;
            }
        }
        // It is an error for the '-' indicator to occur in the flow context, but we let the
        // Parser detect and report about it because the Parser is able to point to the context.

        // Reset any potential simple keys on the current flow level.
        if !self.remove_simple_key() {
            return false;
        }

        // Simple keys are allowed after '-'.
        self.simple_key_allowed = true;

        // Consume the token.
        let start_mark = self.mark;
        self.skip();
        let end_mark = self.mark;

        // Create the BLOCK-ENTRY token and append it to the queue.
        self.insert_token(
            -1,
            Token {
                typ: TokenType::BlockEntry,
                start_mark,
                end_mark,
                ..Token::default()
            },
        );
        true
    }

    /// Go: `yaml_parser_fetch_key`.
    fn fetch_key(&mut self) -> bool {
        // In the block context, additional checks are required.
        if self.flow_level == 0 {
            // Check if we are allowed to start a new key (not necessary simple).
            if !self.simple_key_allowed {
                return self
                    .set_scanner_error(self.mark, "mapping keys are not allowed in this context");
            }
            // Add the BLOCK-MAPPING-START token if needed.
            if !self.roll_indent(
                self.mark.column,
                -1,
                TokenType::BlockMappingStart,
                self.mark,
            ) {
                return false;
            }
        }

        // Reset any potential simple keys on the current flow level.
        if !self.remove_simple_key() {
            return false;
        }

        // Simple keys are allowed after '?' in the block context.
        self.simple_key_allowed = self.flow_level == 0;

        // Consume the token.
        let start_mark = self.mark;
        self.skip();
        let end_mark = self.mark;

        // Create the KEY token and append it to the queue.
        self.insert_token(
            -1,
            Token {
                typ: TokenType::Key,
                start_mark,
                end_mark,
                ..Token::default()
            },
        );
        true
    }

    /// Go: `yaml_parser_fetch_value`.
    fn fetch_value(&mut self) -> bool {
        let i = self.simple_keys.len().saturating_sub(1);

        // [Go] Checking the simple key here, as in libyaml, is safe because the key is always
        // the top of the stack.
        match self.simple_key_is_valid(i) {
            None => return false,
            Some(true) => {
                let Some(simple_key) = self.simple_keys.get(i).copied() else {
                    return false;
                };
                // Create the KEY token and insert it into the queue.
                self.insert_token(
                    simple_key.token_number - self.tokens_parsed,
                    Token {
                        typ: TokenType::Key,
                        start_mark: simple_key.mark,
                        end_mark: simple_key.mark,
                        ..Token::default()
                    },
                );

                // In the block context, we may need to add the BLOCK-MAPPING-START token.
                if !self.roll_indent(
                    simple_key.mark.column,
                    simple_key.token_number,
                    TokenType::BlockMappingStart,
                    simple_key.mark,
                ) {
                    return false;
                }

                // Remove the simple key.
                if let Some(k) = self.simple_keys.get_mut(i) {
                    k.possible = false;
                }
                let _removed = self.simple_keys_by_tok.remove(&simple_key.token_number);

                // A simple key cannot follow another simple key.
                self.simple_key_allowed = false;
            }
            Some(false) => {
                // The ':' indicator follows a complex key.

                // In the block context, extra checks are required.
                if self.flow_level == 0 {
                    // Check if we are allowed to start a complex value.
                    if !self.simple_key_allowed {
                        return self.set_scanner_error(
                            self.mark,
                            "mapping values are not allowed in this context",
                        );
                    }

                    // Add the BLOCK-MAPPING-START token if needed.
                    if !self.roll_indent(
                        self.mark.column,
                        -1,
                        TokenType::BlockMappingStart,
                        self.mark,
                    ) {
                        return false;
                    }
                }

                // Simple keys after ':' are allowed in the block context.
                self.simple_key_allowed = self.flow_level == 0;
            }
        }

        // Consume the token.
        let start_mark = self.mark;
        self.skip();
        let end_mark = self.mark;

        // Create the VALUE token and append it to the queue.
        self.insert_token(
            -1,
            Token {
                typ: TokenType::Value,
                start_mark,
                end_mark,
                ..Token::default()
            },
        );
        true
    }

    /// Go: `yaml_parser_fetch_anchor`.
    fn fetch_anchor(&mut self, typ: TokenType) -> bool {
        // An anchor or an alias could be a simple key.
        if !self.save_simple_key() {
            return false;
        }

        // A simple key cannot follow an anchor or an alias.
        self.simple_key_allowed = false;

        // Create the ALIAS or ANCHOR token and append it to the queue.
        let Some(token) = self.scan_anchor(typ) else {
            return false;
        };
        self.insert_token(-1, token);
        true
    }

    /// Go: `yaml_parser_fetch_tag`.
    fn fetch_tag(&mut self) -> bool {
        // A tag could be a simple key.
        if !self.save_simple_key() {
            return false;
        }

        // A simple key cannot follow a tag.
        self.simple_key_allowed = false;

        // Create the TAG token and append it to the queue.
        let Some(token) = self.scan_tag() else {
            return false;
        };
        self.insert_token(-1, token);
        true
    }

    /// Go: `yaml_parser_fetch_block_scalar`.
    fn fetch_block_scalar(&mut self, literal: bool) -> bool {
        // Remove any potential simple keys.
        if !self.remove_simple_key() {
            return false;
        }

        // A simple key may follow a block scalar.
        self.simple_key_allowed = true;

        // Create the SCALAR token and append it to the queue.
        let Some(token) = self.scan_block_scalar(literal) else {
            return false;
        };
        self.insert_token(-1, token);
        true
    }

    /// Go: `yaml_parser_fetch_flow_scalar`.
    fn fetch_flow_scalar(&mut self, single: bool) -> bool {
        // A plain scalar could be a simple key.
        if !self.save_simple_key() {
            return false;
        }

        // A simple key cannot follow a flow scalar.
        self.simple_key_allowed = false;

        // Create the SCALAR token and append it to the queue.
        let Some(token) = self.scan_flow_scalar(single) else {
            return false;
        };
        self.insert_token(-1, token);
        true
    }

    /// Go: `yaml_parser_fetch_plain_scalar`.
    fn fetch_plain_scalar(&mut self) -> bool {
        // A plain scalar could be a simple key.
        if !self.save_simple_key() {
            return false;
        }

        // A simple key cannot follow a flow scalar.
        self.simple_key_allowed = false;

        // Create the SCALAR token and append it to the queue.
        let Some(token) = self.scan_plain_scalar() else {
            return false;
        };
        self.insert_token(-1, token);
        true
    }

    /// Go: `yaml_parser_scan_to_next_token`: eats whitespaces, comments and line breaks.
    fn scan_to_next_token(&mut self) -> bool {
        let scan_mark = self.mark;

        // Until the next token is not found.
        loop {
            // Allow the BOM.
            if !self.cache(1) {
                return false;
            }
            if self.mark.column == 0 && self.is_bom() {
                self.skip();
            }

            // Eat whitespaces.
            // Tabs are allowed:
            //  - in the flow context
            //  - in the block context, but not at the beginning of the line or after '-', '?', or
            //    ':' (complex value).
            if !self.cache(1) {
                return false;
            }

            while self.peek_byte(0) == b' '
                || ((self.flow_level > 0 || !self.simple_key_allowed) && self.peek_byte(0) == b'\t')
            {
                self.skip();
                if !self.cache(1) {
                    return false;
                }
            }

            // Check if we just had a line comment under a sequence entry that looks more like
            // a header to the following content. Similar to this:
            //
            // - # The comment
            //   - Some data
            //
            // If so, transform the line comment to a head comment and reposition.
            if let (Some(token_a), Some(token_b)) = (self.second_last_token(), self.last_token()) {
                let at_break = self.is_break(self.buffer_pos);
                let mark = self.mark;
                if let Some(comment) = self.comments.last_mut()
                    && token_a.typ == TokenType::BlockSequenceStart
                    && token_b.typ == TokenType::BlockEntry
                    && !comment.line.is_empty()
                    && !at_break
                {
                    comment.head = mem::take(&mut comment.line);
                    if comment.start_mark.line == mark.line - 1 {
                        comment.token_mark = mark;
                    }
                }
            }

            // Eat a comment until a line break.
            if self.peek_byte(0) == b'#' && !self.scan_comments(scan_mark) {
                return false;
            }

            // If it is a line break, eat it.
            if self.is_break(self.buffer_pos) {
                if !self.cache(2) {
                    return false;
                }
                self.skip_line();

                // In the block context, a new line may start a simple key.
                if self.flow_level == 0 {
                    self.simple_key_allowed = true;
                }
            } else {
                break; // We have found a token.
            }
        }

        true
    }

    /// Go: `yaml_parser_scan_directive`.
    ///
    /// Scope:
    ///      %YAML    1.1    # a comment \n
    ///      ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    ///      %TAG    !yaml!  tag:yaml.org,2002:  \n
    ///      ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    fn scan_directive(&mut self) -> Option<Token> {
        // Eat '%'.
        let start_mark = self.mark;
        self.skip();

        // Scan the directive name.
        let name = self.scan_directive_name(start_mark)?;

        // Is it a YAML directive?
        let token = if name == b"YAML" {
            // Scan the VERSION directive value.
            let (major, minor) = self.scan_version_directive_value(start_mark)?;
            let end_mark = self.mark;

            // Create a VERSION-DIRECTIVE token.
            Token {
                typ: TokenType::VersionDirective,
                start_mark,
                end_mark,
                major,
                minor,
                ..Token::default()
            }

            // Is it a TAG directive?
        } else if name == b"TAG" {
            // Scan the TAG directive value.
            let (handle, prefix) = self.scan_tag_directive_value(start_mark)?;
            let end_mark = self.mark;

            // Create a TAG-DIRECTIVE token.
            Token {
                typ: TokenType::TagDirective,
                start_mark,
                end_mark,
                value: handle,
                prefix,
                ..Token::default()
            }

            // Unknown directive.
        } else {
            // "while scanning a directive"
            let _ = self.set_scanner_error(start_mark, "found unknown directive name");
            return None;
        };

        // Eat the rest of the line including any comments.
        if !self.cache(1) {
            return None;
        }

        while self.is_blank(self.buffer_pos) {
            self.skip();
            if !self.cache(1) {
                return None;
            }
        }

        if self.peek_byte(0) == b'#' {
            // [Go] Discard this inline comment for the time being.
            while !self.is_breakz(self.buffer_pos) {
                self.skip();
                if !self.cache(1) {
                    return None;
                }
            }
        }

        // Check if we are at the end of the line.
        if !self.is_breakz(self.buffer_pos) {
            // "while scanning a directive"
            let _ =
                self.set_scanner_error(start_mark, "did not find expected comment or line break");
            return None;
        }

        // Eat a line break.
        if self.is_break(self.buffer_pos) {
            if !self.cache(2) {
                return None;
            }
            self.skip_line();
        }

        Some(token)
    }

    /// Go: `yaml_parser_scan_directive_name`.
    ///
    /// Scope:
    ///      %YAML   1.1     # a comment \n
    ///       ^^^^
    ///      %TAG    !yaml!  tag:yaml.org,2002:  \n
    ///       ^^^
    fn scan_directive_name(&mut self, start_mark: Mark) -> Option<Vec<u8>> {
        // Consume the directive name.
        if !self.cache(1) {
            return None;
        }

        let mut s = Vec::new();
        while self.is_alpha(self.buffer_pos) {
            self.read_char(&mut s);
            if !self.cache(1) {
                return None;
            }
        }

        // Check if the name is empty.
        if s.is_empty() {
            // "while scanning a directive"
            let _ = self.set_scanner_error(start_mark, "could not find expected directive name");
            return None;
        }

        // Check for an blank character after the name.
        if !self.is_blankz(self.buffer_pos) {
            // "while scanning a directive"
            let _ =
                self.set_scanner_error(start_mark, "found unexpected non-alphabetical character");
            return None;
        }
        Some(s)
    }

    /// Go: `yaml_parser_scan_version_directive_value`.
    ///
    /// Scope:
    ///      %YAML   1.1     # a comment \n
    ///           ^^^^^^
    fn scan_version_directive_value(&mut self, start_mark: Mark) -> Option<(i8, i8)> {
        // Eat whitespaces.
        if !self.cache(1) {
            return None;
        }
        while self.is_blank(self.buffer_pos) {
            self.skip();
            if !self.cache(1) {
                return None;
            }
        }

        // Consume the major version number.
        let major = self.scan_version_directive_number(start_mark)?;

        // Eat '.'.
        if self.peek_byte(0) != b'.' {
            // "while scanning a %YAML directive"
            let _ =
                self.set_scanner_error(start_mark, "did not find expected digit or '.' character");
            return None;
        }

        self.skip();

        // Consume the minor version number.
        let minor = self.scan_version_directive_number(start_mark)?;
        Some((major, minor))
    }

    /// Go: `yaml_parser_scan_version_directive_number`.
    ///
    /// Scope:
    ///      %YAML   1.1     # a comment \n
    ///              ^
    ///      %YAML   1.1     # a comment \n
    ///                ^
    fn scan_version_directive_number(&mut self, start_mark: Mark) -> Option<i8> {
        // Repeat while the next character is digit.
        if !self.cache(1) {
            return None;
        }
        let mut value: i8 = 0;
        let mut length: i8 = 0;
        while self.is_digit(self.buffer_pos) {
            // Check if the number is too long.
            length += 1;
            if length > MAX_NUMBER_LENGTH {
                // "while scanning a %YAML directive"
                let _ = self.set_scanner_error(start_mark, "found extremely long version number");
                return None;
            }
            let digit = i8::try_from(self.as_digit(self.buffer_pos)).unwrap_or(0);
            value = value.wrapping_mul(10).wrapping_add(digit);
            self.skip();
            if !self.cache(1) {
                return None;
            }
        }

        // Check if the number was present.
        if length == 0 {
            // "while scanning a %YAML directive"
            let _ = self.set_scanner_error(start_mark, "did not find expected version number");
            return None;
        }
        Some(value)
    }

    /// Go: `yaml_parser_scan_tag_directive_value`.
    ///
    /// Scope:
    ///      %TAG    !yaml!  tag:yaml.org,2002:  \n
    ///          ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    fn scan_tag_directive_value(&mut self, start_mark: Mark) -> Option<(Vec<u8>, Vec<u8>)> {
        // Eat whitespaces.
        if !self.cache(1) {
            return None;
        }

        while self.is_blank(self.buffer_pos) {
            self.skip();
            if !self.cache(1) {
                return None;
            }
        }

        // Scan a handle.
        let handle = self.scan_tag_handle(true, start_mark)?;

        // Expect a whitespace.
        if !self.cache(1) {
            return None;
        }
        if !self.is_blank(self.buffer_pos) {
            // "while scanning a %TAG directive"
            let _ = self.set_scanner_error(start_mark, "did not find expected whitespace");
            return None;
        }

        // Eat whitespaces.
        while self.is_blank(self.buffer_pos) {
            self.skip();
            if !self.cache(1) {
                return None;
            }
        }

        // Scan a prefix.
        let prefix = self.scan_tag_uri(true, &[], start_mark)?;

        // Expect a whitespace or line break.
        if !self.cache(1) {
            return None;
        }
        if !self.is_blankz(self.buffer_pos) {
            // "while scanning a %TAG directive"
            let _ = self
                .set_scanner_error(start_mark, "did not find expected whitespace or line break");
            return None;
        }

        Some((handle, prefix))
    }

    /// Go: `yaml_parser_scan_anchor`.
    fn scan_anchor(&mut self, typ: TokenType) -> Option<Token> {
        let mut s = Vec::new();

        // Eat the indicator character.
        let start_mark = self.mark;
        self.skip();

        // Consume the value.
        if !self.cache(1) {
            return None;
        }

        while self.is_alpha(self.buffer_pos) {
            self.read_char(&mut s);
            if !self.cache(1) {
                return None;
            }
        }

        let end_mark = self.mark;

        // Check if length of the anchor is greater than 0 and it is followed by a whitespace
        // character or one of the indicators:
        //
        //      '?', ':', ',', ']', '}', '%', '@', '`'.
        let c = self.peek_byte(0);
        if s.is_empty()
            || !(self.is_blankz(self.buffer_pos)
                || matches!(c, b'?' | b':' | b',' | b']' | b'}' | b'%' | b'@' | b'`'))
        {
            // "while scanning an alias" / "while scanning an anchor"
            let _ = self.set_scanner_error(
                start_mark,
                "did not find expected alphabetic or numeric character",
            );
            return None;
        }

        // Create a token.
        Some(Token {
            typ,
            start_mark,
            end_mark,
            value: s,
            ..Token::default()
        })
    }

    /// Go: `yaml_parser_scan_tag`.
    fn scan_tag(&mut self) -> Option<Token> {
        let start_mark = self.mark;

        // Check if the tag is in the canonical form.
        if !self.cache(2) {
            return None;
        }

        let (handle, suffix) = if self.peek_byte(1) == b'<' {
            // Keep the handle as ''

            // Eat '!<'
            self.skip();
            self.skip();

            // Consume the tag value.
            let suffix = self.scan_tag_uri(false, &[], start_mark)?;

            // Check for '>' and eat it.
            if self.peek_byte(0) != b'>' {
                // "while scanning a tag"
                let _ = self.set_scanner_error(start_mark, "did not find the expected '>'");
                return None;
            }

            self.skip();
            (Vec::new(), suffix)
        } else {
            // The tag has either the '!suffix' or the '!handle!suffix' form.

            // First, try to scan a handle.
            let handle = self.scan_tag_handle(false, start_mark)?;

            // Check if it is, indeed, handle.
            if handle.first() == Some(&b'!') && handle.len() > 1 && handle.last() == Some(&b'!') {
                // Scan the suffix now.
                let suffix = self.scan_tag_uri(false, &[], start_mark)?;
                (handle, suffix)
            } else {
                // It wasn't a handle after all.  Scan the rest of the tag.
                let suffix = self.scan_tag_uri(false, &handle, start_mark)?;

                // Set the handle to '!'.
                // A special case: the '!' tag.  Set the handle to '' and the suffix to '!'.
                if suffix.is_empty() {
                    (Vec::new(), vec![b'!'])
                } else {
                    (vec![b'!'], suffix)
                }
            }
        };

        // Check the character which ends the tag.
        if !self.cache(1) {
            return None;
        }
        if !self.is_blankz(self.buffer_pos) {
            // "while scanning a tag"
            let _ = self
                .set_scanner_error(start_mark, "did not find expected whitespace or line break");
            return None;
        }

        let end_mark = self.mark;

        // Create a token.
        Some(Token {
            typ: TokenType::Tag,
            start_mark,
            end_mark,
            value: handle,
            suffix,
            ..Token::default()
        })
    }

    /// Go: `yaml_parser_set_scanner_tag_error`. The context ("while parsing a tag" or "while
    /// parsing a %TAG directive") is not kept.
    fn set_scanner_tag_error(&mut self, context_mark: Mark, problem: &str) -> bool {
        self.set_scanner_error(context_mark, problem)
    }

    /// Go: `yaml_parser_scan_tag_handle`.
    fn scan_tag_handle(&mut self, directive: bool, start_mark: Mark) -> Option<Vec<u8>> {
        // Check the initial '!' character.
        if !self.cache(1) {
            return None;
        }
        if self.peek_byte(0) != b'!' {
            let _ = self.set_scanner_tag_error(start_mark, "did not find expected '!'");
            return None;
        }

        // Copy the '!' character.
        let mut s = Vec::new();
        self.read_char(&mut s);

        // Copy all subsequent alphabetical and numerical characters.
        if !self.cache(1) {
            return None;
        }
        while self.is_alpha(self.buffer_pos) {
            self.read_char(&mut s);
            if !self.cache(1) {
                return None;
            }
        }

        // Check if the trailing character is '!' and copy it.
        if self.peek_byte(0) == b'!' {
            self.read_char(&mut s);
        } else if directive && s != b"!" {
            // It's either the '!' tag or not really a tag handle.  If it's a %TAG directive,
            // it's an error.  If it's a tag token, it must be a part of URI.
            let _ = self.set_scanner_tag_error(start_mark, "did not find expected '!'");
            return None;
        }

        Some(s)
    }

    /// Go: `yaml_parser_scan_tag_uri`.
    fn scan_tag_uri(&mut self, directive: bool, head: &[u8], start_mark: Mark) -> Option<Vec<u8>> {
        let mut s = Vec::new();
        let mut has_tag = !head.is_empty();

        // Copy the head if needed.
        //
        // Note that we don't copy the leading '!' character.
        if let Some(rest) = head.get(1..) {
            s.extend_from_slice(rest);
        }

        // Scan the tag.
        if !self.cache(1) {
            return None;
        }

        // The set of characters that may appear in URI is as follows:
        //
        //      '0'-'9', 'A'-'Z', 'a'-'z', '_', '-', ';', '/', '?', ':', '@', '&',
        //      '=', '+', '$', ',', '.', '!', '~', '*', '\'', '(', ')', '[', ']',
        //      '%'.
        // [Go] TODO Convert this into more reasonable logic.
        while self.is_alpha(self.buffer_pos)
            || matches!(
                self.peek_byte(0),
                b';' | b'/'
                    | b'?'
                    | b':'
                    | b'@'
                    | b'&'
                    | b'='
                    | b'+'
                    | b'$'
                    | b','
                    | b'.'
                    | b'!'
                    | b'~'
                    | b'*'
                    | b'\''
                    | b'('
                    | b')'
                    | b'['
                    | b']'
                    | b'%'
            )
        {
            // Check if it is a URI-escape sequence.
            if self.peek_byte(0) == b'%' {
                if !self.scan_uri_escapes(directive, start_mark, &mut s) {
                    return None;
                }
            } else {
                self.read_char(&mut s);
            }
            if !self.cache(1) {
                return None;
            }
            has_tag = true;
        }

        if !has_tag {
            let _ = self.set_scanner_tag_error(start_mark, "did not find expected tag URI");
            return None;
        }
        Some(s)
    }

    /// Go: `yaml_parser_scan_uri_escapes`: decodes a URI-escaped UTF-8 sequence.
    fn scan_uri_escapes(&mut self, _directive: bool, start_mark: Mark, s: &mut Vec<u8>) -> bool {
        // Decode the required number of characters.
        let mut w: usize = 1024;
        while w > 0 {
            // Check for a URI-escaped octet.
            if !self.cache(3) {
                return false;
            }

            if !(self.peek_byte(0) == b'%'
                && self.is_hex(self.buffer_pos + 1)
                && self.is_hex(self.buffer_pos + 2))
            {
                return self.set_scanner_tag_error(start_mark, "did not find URI escaped octet");
            }

            // Get the octet.
            let high = self.as_hex(self.buffer_pos + 1);
            let low = self.as_hex(self.buffer_pos + 2);
            let octet = u8::try_from((high << 4) + low).unwrap_or(0);

            // If it is the leading octet, determine the length of the UTF-8 sequence.
            if w == 1024 {
                w = width(octet);
                if w == 0 {
                    return self.set_scanner_tag_error(
                        start_mark,
                        "found an incorrect leading UTF-8 octet",
                    );
                }
            } else {
                // Check if the trailing octet is correct.
                if octet & 0xC0 != 0x80 {
                    return self.set_scanner_tag_error(
                        start_mark,
                        "found an incorrect trailing UTF-8 octet",
                    );
                }
            }

            // Copy the octet and move the pointers.
            s.push(octet);
            self.skip();
            self.skip();
            self.skip();
            w -= 1;
        }
        true
    }

    /// Go: `yaml_parser_scan_block_scalar`.
    fn scan_block_scalar(&mut self, literal: bool) -> Option<Token> {
        // Eat the indicator '|' or '>'.
        let start_mark = self.mark;
        self.skip();

        let (chomping, increment) = self.scan_block_scalar_header(start_mark)?;

        let mut end_mark = self.mark;

        // Set the indentation level if it was specified.
        let mut indent: i64 = if increment > 0 {
            if self.indent >= 0 {
                self.indent + increment
            } else {
                increment
            }
        } else {
            0
        };

        // Scan the leading line breaks and determine the indentation level if needed.
        let mut s = Vec::new();
        let mut leading_break = Vec::new();
        let mut trailing_breaks = Vec::new();
        if !self.scan_block_scalar_breaks(
            &mut indent,
            &mut trailing_breaks,
            start_mark,
            &mut end_mark,
        ) {
            return None;
        }

        // Scan the block scalar content.
        if !self.cache(1) {
            return None;
        }
        let mut leading_blank = false;
        while self.mark.column == indent && !self.is_z(self.buffer_pos) {
            // We are at the beginning of a non-empty line.

            // Is it a trailing whitespace?
            let trailing_blank = self.is_blank(self.buffer_pos);

            // Check if we need to fold the leading line break.
            if !literal
                && !leading_blank
                && !trailing_blank
                && leading_break.first() == Some(&b'\n')
            {
                // Do we need to join the lines by space?
                if trailing_breaks.is_empty() {
                    s.push(b' ');
                }
            } else {
                s.extend_from_slice(&leading_break);
            }
            leading_break.clear();

            // Append the remaining line breaks.
            s.extend_from_slice(&trailing_breaks);
            trailing_breaks.clear();

            // Is it a leading whitespace?
            leading_blank = self.is_blank(self.buffer_pos);

            // Consume the current line.
            while !self.is_breakz(self.buffer_pos) {
                self.read_char(&mut s);
                if !self.cache(1) {
                    return None;
                }
            }

            // Consume the line break.
            if !self.cache(2) {
                return None;
            }

            self.read_line(&mut leading_break);

            // Eat the following indentation spaces and line breaks.
            if !self.scan_block_scalar_breaks(
                &mut indent,
                &mut trailing_breaks,
                start_mark,
                &mut end_mark,
            ) {
                return None;
            }
        }

        // Chomp the tail.
        if chomping != -1 {
            s.extend_from_slice(&leading_break);
        }
        if chomping == 1 {
            s.extend_from_slice(&trailing_breaks);
        }

        // Create a token.
        Some(Token {
            typ: TokenType::Scalar,
            start_mark,
            end_mark,
            value: s,
            style: if literal {
                ScalarStyle::Literal
            } else {
                ScalarStyle::Folded
            },
            ..Token::default()
        })
    }

    fn scan_block_scalar_header(&mut self, start_mark: Mark) -> Option<(i64, i64)> {
        // Scan the additional block scalar indicators.
        if !self.cache(1) {
            return None;
        }

        // Check for a chomping indicator.
        let mut chomping: i64 = 0;
        let mut increment: i64 = 0;
        if self.peek_byte(0) == b'+' || self.peek_byte(0) == b'-' {
            // Set the chomping method and eat the indicator.
            chomping = if self.peek_byte(0) == b'+' { 1 } else { -1 };
            self.skip();

            // Check for an indentation indicator.
            if !self.cache(1) {
                return None;
            }
            if self.is_digit(self.buffer_pos) {
                // Check that the indentation is greater than 0.
                if self.peek_byte(0) == b'0' {
                    // "while scanning a block scalar"
                    let _ = self
                        .set_scanner_error(start_mark, "found an indentation indicator equal to 0");
                    return None;
                }

                // Get the indentation level and eat the indicator.
                increment = self.as_digit(self.buffer_pos);
                self.skip();
            }
        } else if self.is_digit(self.buffer_pos) {
            // Do the same as above, but in the opposite order.

            if self.peek_byte(0) == b'0' {
                // "while scanning a block scalar"
                let _ =
                    self.set_scanner_error(start_mark, "found an indentation indicator equal to 0");
                return None;
            }
            increment = self.as_digit(self.buffer_pos);
            self.skip();

            if !self.cache(1) {
                return None;
            }
            if self.peek_byte(0) == b'+' || self.peek_byte(0) == b'-' {
                chomping = if self.peek_byte(0) == b'+' { 1 } else { -1 };
                self.skip();
            }
        }

        // Eat whitespaces and comments to the end of the line.
        if !self.cache(1) {
            return None;
        }
        while self.is_blank(self.buffer_pos) {
            self.skip();
            if !self.cache(1) {
                return None;
            }
        }
        if self.peek_byte(0) == b'#' {
            if !self.scan_line_comment(start_mark) {
                return None;
            }
            while !self.is_breakz(self.buffer_pos) {
                self.skip();
                if !self.cache(1) {
                    return None;
                }
            }
        }

        // Check if we are at the end of the line.
        if !self.is_breakz(self.buffer_pos) {
            // "while scanning a block scalar"
            let _ =
                self.set_scanner_error(start_mark, "did not find expected comment or line break");
            return None;
        }

        // Eat a line break.
        if self.is_break(self.buffer_pos) {
            if !self.cache(2) {
                return None;
            }
            self.skip_line();
        }
        Some((chomping, increment))
    }

    /// Go: `yaml_parser_scan_block_scalar_breaks`: scans indentation spaces and line breaks
    /// for a block scalar, determining the indentation level if needed.
    fn scan_block_scalar_breaks(
        &mut self,
        indent: &mut i64,
        breaks: &mut Vec<u8>,
        start_mark: Mark,
        end_mark: &mut Mark,
    ) -> bool {
        *end_mark = self.mark;

        // Eat the indentation spaces and line breaks.
        let mut max_indent: i64 = 0;
        loop {
            // Eat the indentation spaces.
            if !self.cache(1) {
                return false;
            }
            while (*indent == 0 || self.mark.column < *indent) && self.is_space(self.buffer_pos) {
                self.skip();
                if !self.cache(1) {
                    return false;
                }
            }
            if self.mark.column > max_indent {
                max_indent = self.mark.column;
            }

            // Check for a tab character messing the indentation.
            if (*indent == 0 || self.mark.column < *indent) && self.is_tab(self.buffer_pos) {
                // "while scanning a block scalar"
                return self.set_scanner_error(
                    start_mark,
                    "found a tab character where an indentation space is expected",
                );
            }

            // Have we found a non-empty line?
            if !self.is_break(self.buffer_pos) {
                break;
            }

            // Consume the line break.
            if !self.cache(2) {
                return false;
            }
            // [Go] Should really be returning breaks instead.
            self.read_line(breaks);
            *end_mark = self.mark;
        }

        // Determine the indentation level if needed.
        if *indent == 0 {
            *indent = max_indent;
            if *indent < self.indent + 1 {
                *indent = self.indent + 1;
            }
            if *indent < 1 {
                *indent = 1;
            }
        }
        true
    }

    /// Go: `yaml_parser_scan_flow_scalar`: scans a single- or double-quoted scalar.
    fn scan_flow_scalar(&mut self, single: bool) -> Option<Token> {
        // Eat the left quote.
        let start_mark = self.mark;
        self.skip();

        // Consume the content of the quoted scalar.
        let mut s = Vec::new();
        let mut blanks = Blanks::default();
        loop {
            // Check that there are no document indicators at the beginning of the line.
            if !self.cache(4) {
                return None;
            }

            if self.mark.column == 0
                && ((self.peek_byte(0) == b'-'
                    && self.peek_byte(1) == b'-'
                    && self.peek_byte(2) == b'-')
                    || (self.peek_byte(0) == b'.'
                        && self.peek_byte(1) == b'.'
                        && self.peek_byte(2) == b'.'))
                && self.is_blankz(self.buffer_pos + 3)
            {
                // "while scanning a quoted scalar"
                let _ = self.set_scanner_error(start_mark, "found unexpected document indicator");
                return None;
            }

            // Check for EOF.
            if self.is_z(self.buffer_pos) {
                // "while scanning a quoted scalar"
                let _ = self.set_scanner_error(start_mark, "found unexpected end of stream");
                return None;
            }

            // Consume non-blank characters.
            let mut leading_blanks = self.scan_flow_scalar_text(single, start_mark, &mut s)?;

            if !self.cache(1) {
                return None;
            }

            // Check if we are at the end of the scalar.
            if (single && self.peek_byte(0) == b'\'') || (!single && self.peek_byte(0) == b'"') {
                break;
            }

            // Consume blank characters.
            leading_blanks = self.scan_scalar_blanks(&mut blanks, leading_blanks, None)?;

            // Join the whitespaces or fold line breaks.
            blanks.join(&mut s, leading_blanks);
        }

        // Eat the right quote.
        self.skip();
        let end_mark = self.mark;

        // Create a token.
        Some(Token {
            typ: TokenType::Scalar,
            start_mark,
            end_mark,
            value: s,
            style: if single {
                ScalarStyle::SingleQuoted
            } else {
                ScalarStyle::DoubleQuoted
            },
            ..Token::default()
        })
    }

    fn scan_flow_scalar_text(
        &mut self,
        single: bool,
        start_mark: Mark,
        s: &mut Vec<u8>,
    ) -> Option<bool> {
        while !self.is_blankz(self.buffer_pos) {
            let c = self.peek_byte(0);
            if single && c == b'\'' && self.peek_byte(1) == b'\'' {
                // Is is an escaped single quote.
                s.push(b'\'');
                self.skip();
                self.skip();
            } else if single && c == b'\'' {
                // It is a right single quote.
                break;
            } else if !single && c == b'"' {
                // It is a right double quote.
                break;
            } else if !single && c == b'\\' && self.is_break(self.buffer_pos + 1) {
                // It is an escaped line break.
                if !self.cache(3) {
                    return None;
                }
                self.skip();
                self.skip_line();
                return Some(true);
            } else if !single && c == b'\\' {
                // It is an escape sequence.
                if !self.scan_escape(start_mark, s) {
                    return None;
                }
            } else {
                // It is a non-escaped non-blank character.
                self.read_char(s);
            }
            if !self.cache(2) {
                return None;
            }
        }
        Some(false)
    }

    fn scan_scalar_blanks(
        &mut self,
        blanks: &mut Blanks,
        mut leading_blanks: bool,
        tab_limit: Option<(i64, Mark)>,
    ) -> Option<bool> {
        while self.is_blank(self.buffer_pos) || self.is_break(self.buffer_pos) {
            if self.is_blank(self.buffer_pos) {
                // Check for tab characters that abuse indentation.
                if let Some((indent, start_mark)) = tab_limit
                    && leading_blanks
                    && self.mark.column < indent
                    && self.is_tab(self.buffer_pos)
                {
                    // "while scanning a plain scalar"
                    let _ = self.set_scanner_error(
                        start_mark,
                        "found a tab character that violates indentation",
                    );
                    return None;
                }

                // Consume a space or a tab character.
                if leading_blanks {
                    self.skip();
                } else {
                    self.read_char(&mut blanks.whitespaces);
                }
            } else {
                if !self.cache(2) {
                    return None;
                }

                // Check if it is a first line break.
                if leading_blanks {
                    self.read_line(&mut blanks.trailing_breaks);
                } else {
                    blanks.whitespaces.clear();
                    self.read_line(&mut blanks.leading_break);
                    leading_blanks = true;
                }
            }
            if !self.cache(1) {
                return None;
            }
        }
        Some(leading_blanks)
    }

    /// The escape sequence branch of `yaml_parser_scan_flow_scalar`.
    fn scan_escape(&mut self, start_mark: Mark, s: &mut Vec<u8>) -> bool {
        let mut code_length: usize = 0;

        // Check the escape character.
        match self.peek_byte(1) {
            b'0' => s.push(0),
            b'a' => s.push(0x07),
            b'b' => s.push(0x08),
            b't' | b'\t' => s.push(0x09),
            b'n' => s.push(0x0A),
            b'v' => s.push(0x0B),
            b'f' => s.push(0x0C),
            b'r' => s.push(0x0D),
            b'e' => s.push(0x1B),
            b' ' => s.push(0x20),
            b'"' => s.push(b'"'),
            b'\'' => s.push(b'\''),
            b'\\' => s.push(b'\\'),
            // NEL (#x85)
            b'N' => s.extend_from_slice(&[0xC2, 0x85]),
            // #xA0
            b'_' => s.extend_from_slice(&[0xC2, 0xA0]),
            // LS (#x2028)
            b'L' => s.extend_from_slice(&[0xE2, 0x80, 0xA8]),
            // PS (#x2029)
            b'P' => s.extend_from_slice(&[0xE2, 0x80, 0xA9]),
            b'x' => code_length = 2,
            b'u' => code_length = 4,
            b'U' => code_length = 8,
            _ => {
                // "while parsing a quoted scalar"
                return self.set_scanner_error(start_mark, "found unknown escape character");
            }
        }

        self.skip();
        self.skip();

        // Consume an arbitrary escape code.
        if code_length > 0 {
            let mut value: i64 = 0;

            // Scan the character value.
            if !self.cache(as_int(code_length)) {
                return false;
            }
            for k in 0..code_length {
                if !self.is_hex(self.buffer_pos + k) {
                    // "while parsing a quoted scalar"
                    return self
                        .set_scanner_error(start_mark, "did not find expected hexdecimal number");
                }
                value = (value << 4) + self.as_hex(self.buffer_pos + k);
            }

            // Check the value and write the character.
            if (0xD800..=0xDFFF).contains(&value) || value > 0x10_FFFF {
                // "while parsing a quoted scalar"
                return self
                    .set_scanner_error(start_mark, "found invalid Unicode character escape code");
            }
            if let Some(c) = u32::try_from(value).ok().and_then(char::from_u32) {
                let mut utf8 = [0; 4];
                s.extend_from_slice(c.encode_utf8(&mut utf8).as_bytes());
            }

            // Advance the pointer.
            for _ in 0..code_length {
                self.skip();
            }
        }
        true
    }

    /// Go: `yaml_parser_scan_plain_scalar`.
    fn scan_plain_scalar(&mut self) -> Option<Token> {
        let mut s = Vec::new();
        let mut blanks = Blanks::default();
        let mut leading_blanks = false;
        let indent = self.indent + 1;

        let start_mark = self.mark;
        let mut end_mark = self.mark;

        // Consume the content of the plain scalar.
        loop {
            // Check for a document indicator.
            if !self.cache(4) {
                return None;
            }
            if self.mark.column == 0
                && ((self.peek_byte(0) == b'-'
                    && self.peek_byte(1) == b'-'
                    && self.peek_byte(2) == b'-')
                    || (self.peek_byte(0) == b'.'
                        && self.peek_byte(1) == b'.'
                        && self.peek_byte(2) == b'.'))
                && self.is_blankz(self.buffer_pos + 3)
            {
                break;
            }

            // Check for a comment.
            if self.peek_byte(0) == b'#' {
                break;
            }

            // Consume non-blank characters.
            while !self.is_blankz(self.buffer_pos) {
                // Check for indicators that may end a plain scalar.
                let c = self.peek_byte(0);
                if (c == b':' && self.is_blankz(self.buffer_pos + 1))
                    || (self.flow_level > 0 && matches!(c, b',' | b'?' | b'[' | b']' | b'{' | b'}'))
                {
                    break;
                }

                // Check if we need to join whitespaces and breaks.
                if leading_blanks || !blanks.whitespaces.is_empty() {
                    blanks.join(&mut s, leading_blanks);
                    leading_blanks = false;
                }

                // Copy the character.
                self.read_char(&mut s);

                end_mark = self.mark;
                if !self.cache(2) {
                    return None;
                }
            }

            // Is it the end?
            if !(self.is_blank(self.buffer_pos) || self.is_break(self.buffer_pos)) {
                break;
            }

            // Consume blank characters.
            if !self.cache(1) {
                return None;
            }
            leading_blanks =
                self.scan_scalar_blanks(&mut blanks, leading_blanks, Some((indent, start_mark)))?;

            // Check indentation level.
            if self.flow_level == 0 && self.mark.column < indent {
                break;
            }
        }

        // Note that we change the 'simple_key_allowed' flag.
        if leading_blanks {
            self.simple_key_allowed = true;
        }

        // Create a token.
        Some(Token {
            typ: TokenType::Scalar,
            start_mark,
            end_mark,
            value: s,
            style: ScalarStyle::Plain,
            ..Token::default()
        })
    }

    /// Go: `yaml_parser_scan_line_comment`: consumes a comment that follows a token on its
    /// line.
    fn scan_line_comment(&mut self, token_mark: Mark) -> bool {
        if self.newlines > 0 {
            return true;
        }

        let mut start_mark = Mark::default();
        let mut text = Vec::new();

        let mut peek: usize = 0;
        while peek < 512 {
            // Go ignores a failure to fill the buffer here.
            if self.unread < as_int(peek) + 1 && !self.update_buffer(as_int(peek) + 1) {
                break;
            }
            if self.is_blank(self.buffer_pos + peek) {
                peek += 1;
                continue;
            }
            if self.peek_byte(peek) == b'#' {
                let seen = self.mark.index + as_int(peek);
                loop {
                    if !self.cache(1) {
                        return false;
                    }
                    if self.is_breakz(self.buffer_pos) {
                        if self.mark.index >= seen {
                            break;
                        }
                        if !self.cache(2) {
                            return false;
                        }
                        self.skip_line();
                    } else if self.mark.index >= seen {
                        if text.is_empty() {
                            start_mark = self.mark;
                        }
                        self.read_char(&mut text);
                    } else {
                        self.skip();
                    }
                }
            }
            break;
        }
        if !text.is_empty() {
            self.comments.push(Comment {
                token_mark,
                start_mark,
                line: text,
                ..Comment::default()
            });
        }
        true
    }

    /// The foot line of `yaml_parser_scan_comments`: the line where a comment must start to
    /// still be considered as a foot of the prior content (-1 at the start of the stream). If
    /// there's some content in the currently parsed line, then the foot is the line below it.
    fn comment_foot_line(&self, scan_mark_in: Mark) -> i64 {
        let mut foot_line = -1;
        if scan_mark_in.line > 0 {
            foot_line = self.mark.line - self.newlines + 1;
            if self.newlines == 0 && self.mark.column > 1 {
                foot_line += 1;
            }
        }
        foot_line
    }

    /// Go: `yaml_parser_scan_comments`: consumes the comment lines (and the blank lines
    /// between them) that start at the current position, splitting them into foot comments of
    /// the prior token and a head comment of the next one.
    fn scan_comments(&mut self, scan_mark_in: Mark) -> bool {
        let token = self.comment_prior_token();
        let mut scan = CommentScan {
            scan_mark: scan_mark_in,
            token_mark: token.start_mark,
            start_mark: Mark::default(),
            line: self.mark.line,
            column: self.mark.column,
            text: Vec::new(),
        };
        let mut next_indent = self.indent.max(0);

        let mut recent_empty = false;
        let mut first_empty = self.newlines <= 1;
        let foot_line = self.comment_foot_line(scan_mark_in);

        let mut peek: usize = 0;
        while peek < 512 {
            // Go ignores a failure to fill the buffer here.
            if self.unread < as_int(peek) + 1 && !self.update_buffer(as_int(peek) + 1) {
                break;
            }
            scan.column += 1;
            if self.is_blank(self.buffer_pos + peek) {
                peek += 1;
                continue;
            }
            let index = self.mark.index + as_int(peek);
            let c = self.peek_byte(peek);
            let close_flow = self.flow_level > 0 && (c == b']' || c == b'}');
            if close_flow || self.is_breakz(self.buffer_pos + peek) {
                // Got line break or terminator.
                if close_flow || !recent_empty {
                    if close_flow
                        || (first_empty
                            && ((scan.start_mark.line == foot_line
                                && token.typ != TokenType::Value)
                                || scan.start_mark.column - 1 < next_indent))
                    {
                        // This is the first empty line and there were no empty lines before, so
                        // this initial part of the comment is a foot of the prior token instead
                        // of being a head for the following one. Split it up. Alternatively,
                        // this might also be the last comment inside a flow scope, so it must
                        // be a footer.
                        if !scan.text.is_empty() {
                            if scan.start_mark.column - 1 < next_indent {
                                // If dedented it's unrelated to the prior token.
                                scan.token_mark = scan.start_mark;
                            }
                            scan.split_foot(&mut self.comments, index);
                        }
                    } else if !scan.text.is_empty() && c != 0 {
                        scan.text.push(b'\n');
                    }
                }
                if !self.is_break(self.buffer_pos + peek) {
                    break;
                }
                first_empty = false;
                recent_empty = true;
                scan.column = 0;
                scan.line += 1;
                peek += 1;
                continue;
            }

            if !scan.text.is_empty()
                && (close_flow
                    || (scan.column - 1 < next_indent && scan.column != scan.start_mark.column))
            {
                // The comment at the different indentation is a foot of the preceding data
                // rather than a head of the upcoming one.
                scan.split_foot(&mut self.comments, index);
            }

            if c != b'#' {
                break;
            }

            if scan.text.is_empty() {
                scan.start_mark = Mark {
                    index,
                    line: scan.line,
                    column: scan.column,
                };
            } else {
                scan.text.push(b'\n');
            }

            recent_empty = false;

            // Consume until after the consumed comment line.
            if !self.consume_comment_line(index, &mut scan.text) {
                return false;
            }

            scan.column = 0;
            scan.line = self.mark.line;
            next_indent = self.indent.max(0);
            // Go resets `peek` to 0 here, and the loop's post statement then increments it.
            peek = 1;
        }

        if !scan.text.is_empty() {
            self.comments.push(Comment {
                scan_mark: scan.scan_mark,
                token_mark: scan.start_mark,
                start_mark: scan.start_mark,
                end_mark: Mark {
                    index: self.mark.index + as_int(peek) - 1,
                    line: scan.line,
                    column: scan.column,
                },
                head: scan.text,
                ..Comment::default()
            });
        }
        true
    }

    fn comment_prior_token(&self) -> TokenRef {
        let token = self.last_token().unwrap_or_default();
        if token.typ == TokenType::FlowEntry
            && let Some(prior) = self.second_last_token()
        {
            return prior;
        }
        token
    }

    fn consume_comment_line(&mut self, seen: i64, text: &mut Vec<u8>) -> bool {
        loop {
            if !self.cache(1) {
                return false;
            }
            if self.is_breakz(self.buffer_pos) {
                if self.mark.index >= seen {
                    return true;
                }
                if !self.cache(2) {
                    return false;
                }
                self.skip_line();
            } else if self.mark.index >= seen {
                self.read_char(text);
            } else {
                self.skip();
            }
        }
    }
}
