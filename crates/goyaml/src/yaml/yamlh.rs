//! The libyaml types the parser uses (Go: `yamlh.go`).
//!
//! Go `int` fields are `i64` so that the arithmetic (and the comparisons against an indent of
//! `-1`) stays Go's. The emitter types, and the comment fields of events (which only feed
//! `yaml.Node` comments, a part of the API this port does not expose), are not ported.

use std::collections::{HashMap, VecDeque};

/// Go: `yaml_mark_t`, a position in the input stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Mark {
    /// The position index, in characters.
    pub(crate) index: i64,
    /// The position line.
    pub(crate) line: i64,
    /// The position column.
    pub(crate) column: i64,
}

/// Go: `yaml_encoding_t`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Encoding {
    /// Go: `yaml_ANY_ENCODING`: not yet determined.
    Any,
    /// Go: `yaml_UTF8_ENCODING`.
    Utf8,
    /// Go: `yaml_UTF16LE_ENCODING`.
    Utf16Le,
    /// Go: `yaml_UTF16BE_ENCODING`.
    Utf16Be,
}

/// Go: `yaml_error_type_t`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ErrorType {
    /// Go: `yaml_NO_ERROR`.
    None,
    /// Go: `yaml_READER_ERROR`.
    Reader,
    /// Go: `yaml_SCANNER_ERROR`.
    Scanner,
    /// Go: `yaml_PARSER_ERROR`.
    Parser,
}

/// Go: `yaml_scalar_style_t` (a bit set in Go; the scanner only ever sets one bit).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ScalarStyle {
    /// Go: `yaml_ANY_SCALAR_STYLE`.
    #[default]
    Any,
    /// Go: `yaml_PLAIN_SCALAR_STYLE`.
    Plain,
    /// Go: `yaml_SINGLE_QUOTED_SCALAR_STYLE`.
    SingleQuoted,
    /// Go: `yaml_DOUBLE_QUOTED_SCALAR_STYLE`.
    DoubleQuoted,
    /// Go: `yaml_LITERAL_SCALAR_STYLE`.
    Literal,
    /// Go: `yaml_FOLDED_SCALAR_STYLE`.
    Folded,
}

/// Go: `yaml_token_type_t`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum TokenType {
    /// Go: `yaml_NO_TOKEN`.
    #[default]
    NoToken,
    /// Go: `yaml_STREAM_START_TOKEN`.
    StreamStart,
    /// Go: `yaml_STREAM_END_TOKEN`.
    StreamEnd,
    /// Go: `yaml_VERSION_DIRECTIVE_TOKEN`.
    VersionDirective,
    /// Go: `yaml_TAG_DIRECTIVE_TOKEN`.
    TagDirective,
    /// Go: `yaml_DOCUMENT_START_TOKEN`.
    DocumentStart,
    /// Go: `yaml_DOCUMENT_END_TOKEN`.
    DocumentEnd,
    /// Go: `yaml_BLOCK_SEQUENCE_START_TOKEN`.
    BlockSequenceStart,
    /// Go: `yaml_BLOCK_MAPPING_START_TOKEN`.
    BlockMappingStart,
    /// Go: `yaml_BLOCK_END_TOKEN`.
    BlockEnd,
    /// Go: `yaml_FLOW_SEQUENCE_START_TOKEN`.
    FlowSequenceStart,
    /// Go: `yaml_FLOW_SEQUENCE_END_TOKEN`.
    FlowSequenceEnd,
    /// Go: `yaml_FLOW_MAPPING_START_TOKEN`.
    FlowMappingStart,
    /// Go: `yaml_FLOW_MAPPING_END_TOKEN`.
    FlowMappingEnd,
    /// Go: `yaml_BLOCK_ENTRY_TOKEN`.
    BlockEntry,
    /// Go: `yaml_FLOW_ENTRY_TOKEN`.
    FlowEntry,
    /// Go: `yaml_KEY_TOKEN`.
    Key,
    /// Go: `yaml_VALUE_TOKEN`.
    Value,
    /// Go: `yaml_ALIAS_TOKEN`.
    Alias,
    /// Go: `yaml_ANCHOR_TOKEN`.
    Anchor,
    /// Go: `yaml_TAG_TOKEN`.
    Tag,
    /// Go: `yaml_SCALAR_TOKEN`.
    Scalar,
}

/// Go: `yaml_token_t`.
#[derive(Clone, Debug, Default)]
pub(super) struct Token {
    /// The token type.
    pub(crate) typ: TokenType,
    /// The beginning of the token.
    pub(crate) start_mark: Mark,
    /// The end of the token.
    pub(crate) end_mark: Mark,
    /// The alias, anchor or scalar value, the tag handle, or the `%TAG` directive handle.
    pub(crate) value: Vec<u8>,
    /// The tag suffix.
    pub(crate) suffix: Vec<u8>,
    /// The `%TAG` directive prefix.
    pub(crate) prefix: Vec<u8>,
    /// The scalar style.
    pub(crate) style: ScalarStyle,
    /// The `%YAML` directive major version.
    pub(crate) major: i8,
    /// The `%YAML` directive minor version.
    pub(crate) minor: i8,
}

/// Go: `yaml_event_type_t`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum EventType {
    /// Go: `yaml_NO_EVENT`.
    #[default]
    NoEvent,
    /// Go: `yaml_STREAM_START_EVENT`.
    StreamStart,
    /// Go: `yaml_STREAM_END_EVENT`.
    StreamEnd,
    /// Go: `yaml_DOCUMENT_START_EVENT`.
    DocumentStart,
    /// Go: `yaml_DOCUMENT_END_EVENT`.
    DocumentEnd,
    /// Go: `yaml_ALIAS_EVENT`.
    Alias,
    /// Go: `yaml_SCALAR_EVENT`.
    Scalar,
    /// Go: `yaml_SEQUENCE_START_EVENT`.
    SequenceStart,
    /// Go: `yaml_SEQUENCE_END_EVENT`.
    SequenceEnd,
    /// Go: `yaml_MAPPING_START_EVENT`.
    MappingStart,
    /// Go: `yaml_MAPPING_END_EVENT`.
    MappingEnd,
}

impl EventType {
    /// Go: `eventStrings` (`yaml_event_type_t.String`).
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::NoEvent => "none",
            Self::StreamStart => "stream start",
            Self::StreamEnd => "stream end",
            Self::DocumentStart => "document start",
            Self::DocumentEnd => "document end",
            Self::Alias => "alias",
            Self::Scalar => "scalar",
            Self::SequenceStart => "sequence start",
            Self::SequenceEnd => "sequence end",
            Self::MappingStart => "mapping start",
            Self::MappingEnd => "mapping end",
        }
    }
}

/// Go: `yaml_event_t`, without the comment fields and the document directives (which the
/// composer never reads).
#[derive(Clone, Debug, Default)]
pub(super) struct Event {
    /// The event type.
    pub(crate) typ: EventType,
    /// The beginning of the event.
    pub(crate) start_mark: Mark,
    /// The anchor (alias and node events); empty when the node has none.
    pub(crate) anchor: Vec<u8>,
    /// The tag (node events).
    pub(crate) tag: Vec<u8>,
    /// The scalar value.
    pub(crate) value: Vec<u8>,
    /// The scalar style.
    pub(crate) scalar_style: ScalarStyle,
}

/// Go: `yaml_parser_state_t`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ParserState {
    /// Go: `yaml_PARSE_STREAM_START_STATE`.
    #[default]
    StreamStart,
    /// Go: `yaml_PARSE_IMPLICIT_DOCUMENT_START_STATE`.
    ImplicitDocumentStart,
    /// Go: `yaml_PARSE_DOCUMENT_START_STATE`.
    DocumentStart,
    /// Go: `yaml_PARSE_DOCUMENT_CONTENT_STATE`.
    DocumentContent,
    /// Go: `yaml_PARSE_DOCUMENT_END_STATE`.
    DocumentEnd,
    /// Go: `yaml_PARSE_BLOCK_NODE_STATE`.
    BlockNode,
    /// Go: `yaml_PARSE_BLOCK_SEQUENCE_FIRST_ENTRY_STATE`.
    BlockSequenceFirstEntry,
    /// Go: `yaml_PARSE_BLOCK_SEQUENCE_ENTRY_STATE`.
    BlockSequenceEntry,
    /// Go: `yaml_PARSE_INDENTLESS_SEQUENCE_ENTRY_STATE`.
    IndentlessSequenceEntry,
    /// Go: `yaml_PARSE_BLOCK_MAPPING_FIRST_KEY_STATE`.
    BlockMappingFirstKey,
    /// Go: `yaml_PARSE_BLOCK_MAPPING_KEY_STATE`.
    BlockMappingKey,
    /// Go: `yaml_PARSE_BLOCK_MAPPING_VALUE_STATE`.
    BlockMappingValue,
    /// Go: `yaml_PARSE_FLOW_SEQUENCE_FIRST_ENTRY_STATE`.
    FlowSequenceFirstEntry,
    /// Go: `yaml_PARSE_FLOW_SEQUENCE_ENTRY_STATE`.
    FlowSequenceEntry,
    /// Go: `yaml_PARSE_FLOW_SEQUENCE_ENTRY_MAPPING_KEY_STATE`.
    FlowSequenceEntryMappingKey,
    /// Go: `yaml_PARSE_FLOW_SEQUENCE_ENTRY_MAPPING_VALUE_STATE`.
    FlowSequenceEntryMappingValue,
    /// Go: `yaml_PARSE_FLOW_SEQUENCE_ENTRY_MAPPING_END_STATE`.
    FlowSequenceEntryMappingEnd,
    /// Go: `yaml_PARSE_FLOW_MAPPING_FIRST_KEY_STATE`.
    FlowMappingFirstKey,
    /// Go: `yaml_PARSE_FLOW_MAPPING_KEY_STATE`.
    FlowMappingKey,
    /// Go: `yaml_PARSE_FLOW_MAPPING_VALUE_STATE`.
    FlowMappingValue,
    /// Go: `yaml_PARSE_FLOW_MAPPING_EMPTY_VALUE_STATE`.
    FlowMappingEmptyValue,
    /// Go: `yaml_PARSE_END_STATE`.
    End,
}

/// Go: `yaml_simple_key_t`.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct SimpleKey {
    /// Is a simple key possible?
    pub(crate) possible: bool,
    /// Is a simple key required?
    pub(crate) required: bool,
    /// The number of the token.
    pub(crate) token_number: i64,
    /// The position mark.
    pub(crate) mark: Mark,
}

/// Go: `yaml_tag_directive_t`.
#[derive(Clone, Debug)]
pub(super) struct TagDirective {
    /// The tag handle.
    pub(crate) handle: Vec<u8>,
    /// The tag prefix.
    pub(crate) prefix: Vec<u8>,
}

/// Go: `yaml_comment_t`.
///
/// Comment text is unobservable through `yaml.Unmarshal`, but whether a head comment is pending
/// decides whether the parser peeks one extra time at a sequence entry
/// (`yaml_parser_split_stem_comment`), which retries a failed token fetch. The attribution is
/// therefore tracked exactly.
#[derive(Clone, Debug, Default)]
pub(super) struct Comment {
    /// Position where scanning for comments started.
    pub(crate) scan_mark: Mark,
    /// Position after which tokens will be associated with this comment.
    pub(crate) token_mark: Mark,
    /// Position of '#' comment mark.
    pub(crate) start_mark: Mark,
    /// Position where comment terminated.
    pub(crate) end_mark: Mark,
    /// A head comment.
    pub(crate) head: Vec<u8>,
    /// A line comment.
    pub(crate) line: Vec<u8>,
}

/// The type and start of a token, kept for the scanner's look-behind at the most recently
/// queued tokens (Go reads `parser.tokens[len(parser.tokens)-1]` and `-2`, which may already
/// have been consumed).
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct TokenRef {
    /// The token type.
    pub(crate) typ: TokenType,
    /// The beginning of the token.
    pub(crate) start_mark: Mark,
}

/// The scanner's progress through the stream.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct StreamFlags {
    /// Have we started to scan the input stream?
    pub(crate) start_produced: bool,
    /// Have we reached the end of the input stream?
    pub(crate) end_produced: bool,
}

/// Go: `input_raw_buffer_size`.
pub(super) const INPUT_RAW_BUFFER_SIZE: usize = 512;

/// Go: `input_buffer_size`.
pub(super) const INPUT_BUFFER_SIZE: usize = INPUT_RAW_BUFFER_SIZE * 3;

/// Go: `yaml_parser_t`, reading from a byte string (`yaml_parser_set_input_string`).
#[derive(Debug)]
pub(super) struct Parser<'a> {
    // UnmarshalError handling
    /// `UnmarshalError` type.
    pub(crate) error: ErrorType,
    /// `UnmarshalError` description.
    pub(crate) problem: String,
    /// The problem position.
    pub(crate) problem_mark: Mark,
    /// The error context mark.
    pub(crate) context_mark: Mark,

    // Reader stuff
    /// String input data.
    pub(crate) input: &'a [u8],
    /// Current input position.
    pub(crate) input_pos: usize,
    /// EOF flag.
    pub(crate) eof: bool,
    /// The working buffer's backing array (Go: `cap(parser.buffer)` bytes).
    pub(crate) buffer: Vec<u8>,
    /// The working buffer's length (Go: `len(parser.buffer)`).
    pub(crate) buffer_len: usize,
    /// The current position of the buffer.
    pub(crate) buffer_pos: usize,
    /// The number of unread characters in the buffer.
    pub(crate) unread: i64,
    /// The number of line breaks since the last non-blank character.
    pub(crate) newlines: i64,
    /// The raw buffer's backing array (Go: `cap(parser.raw_buffer)` bytes).
    pub(crate) raw_buffer: Vec<u8>,
    /// The raw buffer's length (Go: `len(parser.raw_buffer)`).
    pub(crate) raw_buffer_len: usize,
    /// The current position of the raw buffer.
    pub(crate) raw_buffer_pos: usize,
    /// The input encoding.
    pub(crate) encoding: Encoding,
    /// The mark of the current position.
    pub(crate) mark: Mark,

    // Comments
    /// The current head comment. The line, foot and stem comments only feed event comments,
    /// which the composer's decoding never reads, so they are not kept.
    pub(crate) head_comment: Vec<u8>,
    /// The folded comments for all parsed tokens.
    pub(crate) comments: Vec<Comment>,
    /// The index of the next comment to unfold.
    pub(crate) comments_head: usize,

    // Scanner stuff
    /// Go: `stream_start_produced` and `stream_end_produced`.
    pub(crate) stream: StreamFlags,
    /// The number of unclosed '[' and '{' indicators.
    pub(crate) flow_level: i64,
    /// The tokens queue, from the head (Go: `parser.tokens[parser.tokens_head:]`).
    pub(crate) tokens: VecDeque<Token>,
    /// The number of consumed tokens Go's token slice still holds before the queue head (Go:
    /// `parser.tokens_head`); `yaml_insert_token` drops them when the slice is full.
    pub(crate) tokens_head: usize,
    /// The capacity of Go's token slice, which decides when consumed tokens are dropped.
    pub(crate) tokens_cap: usize,
    /// The last two consumed tokens, oldest first, while Go's token slice still holds them.
    pub(crate) consumed: [Option<TokenRef>; 2],
    /// The number of tokens fetched from the queue.
    pub(crate) tokens_parsed: i64,
    /// Does the tokens queue contain a token ready for dequeueing?
    pub(crate) token_available: bool,
    /// The current indentation level.
    pub(crate) indent: i64,
    /// The indentation levels stack.
    pub(crate) indents: Vec<i64>,
    /// May a simple key occur at the current position?
    pub(crate) simple_key_allowed: bool,
    /// The stack of simple keys.
    pub(crate) simple_keys: Vec<SimpleKey>,
    /// Possible simple keys, by token number.
    pub(crate) simple_keys_by_tok: HashMap<i64, usize>,

    // Parser stuff
    /// The current parser state.
    pub(crate) state: ParserState,
    /// The parser states stack.
    pub(crate) states: Vec<ParserState>,
    /// The stack of marks.
    pub(crate) marks: Vec<Mark>,
    /// The list of TAG directives.
    pub(crate) tag_directives: Vec<TagDirective>,
}

impl<'a> Parser<'a> {
    /// Go: `yaml_parser_initialize` followed by `yaml_parser_set_input_string`.
    pub(crate) fn new(input: &'a [u8]) -> Self {
        Self {
            error: ErrorType::None,
            problem: String::new(),
            problem_mark: Mark::default(),
            context_mark: Mark::default(),
            input,
            input_pos: 0,
            eof: false,
            buffer: vec![0; INPUT_BUFFER_SIZE],
            buffer_len: 0,
            buffer_pos: 0,
            unread: 0,
            newlines: 0,
            raw_buffer: vec![0; INPUT_RAW_BUFFER_SIZE],
            raw_buffer_len: 0,
            raw_buffer_pos: 0,
            encoding: Encoding::Any,
            mark: Mark::default(),
            head_comment: Vec::new(),
            comments: Vec::new(),
            comments_head: 0,
            stream: StreamFlags::default(),
            flow_level: 0,
            tokens: VecDeque::new(),
            tokens_head: 0,
            tokens_cap: 0,
            consumed: [None, None],
            tokens_parsed: 0,
            token_available: false,
            indent: 0,
            indents: Vec::new(),
            simple_key_allowed: false,
            simple_keys: Vec::new(),
            simple_keys_by_tok: HashMap::new(),
            state: ParserState::StreamStart,
            states: Vec::new(),
            marks: Vec::new(),
            tag_directives: Vec::new(),
        }
    }

    /// Go: `parser.buffer[i]`. Go panics past `len(parser.buffer)`; this returns NUL there,
    /// which only an out-of-bounds Go read could observe.
    pub(crate) fn at(&self, i: usize) -> u8 {
        if i < self.buffer_len {
            self.buffer.get(i).copied().unwrap_or(0)
        } else {
            0
        }
    }

    /// Go: `parser.buffer[parser.buffer_pos+k]`.
    pub(crate) fn peek_byte(&self, k: usize) -> u8 {
        self.at(self.buffer_pos + k)
    }

    /// The consumed token `back` places before the queue head, if Go's slice still holds it.
    const fn consumed_token(&self, back: usize) -> Option<TokenRef> {
        if self.tokens_head < back {
            return None;
        }
        match back {
            1 => self.consumed[1],
            2 => self.consumed[0],
            _ => None,
        }
    }

    /// The last token of Go's token slice (`parser.tokens[len(parser.tokens)-1]`).
    pub(crate) fn last_token(&self) -> Option<TokenRef> {
        self.tokens
            .back()
            .map(|t| TokenRef {
                typ: t.typ,
                start_mark: t.start_mark,
            })
            .or_else(|| self.consumed_token(1))
    }

    /// The second to last token of Go's token slice (`parser.tokens[len(parser.tokens)-2]`).
    pub(crate) fn second_last_token(&self) -> Option<TokenRef> {
        let n = self.tokens.len();
        match n {
            0 => self.consumed_token(2),
            1 => self.consumed_token(1),
            _ => self.tokens.get(n - 2).map(|t| TokenRef {
                typ: t.typ,
                start_mark: t.start_mark,
            }),
        }
    }

    /// Go: `is_alpha`: an alphanumeric character, `_` or `-`.
    pub(crate) fn is_alpha(&self, i: usize) -> bool {
        let b = self.at(i);
        b.is_ascii_digit()
            || b.is_ascii_uppercase()
            || b.is_ascii_lowercase()
            || b == b'_'
            || b == b'-'
    }

    /// Go: `is_digit`.
    pub(crate) fn is_digit(&self, i: usize) -> bool {
        self.at(i).is_ascii_digit()
    }

    /// Go: `as_digit`.
    pub(crate) fn as_digit(&self, i: usize) -> i64 {
        i64::from(self.at(i)) - i64::from(b'0')
    }

    /// Go: `is_hex`.
    pub(crate) fn is_hex(&self, i: usize) -> bool {
        self.at(i).is_ascii_hexdigit()
    }

    /// Go: `as_hex`.
    pub(crate) fn as_hex(&self, i: usize) -> i64 {
        let b = self.at(i);
        if b.is_ascii_uppercase() && b <= b'F' {
            return i64::from(b) - i64::from(b'A') + 10;
        }
        if b.is_ascii_lowercase() && b <= b'f' {
            return i64::from(b) - i64::from(b'a') + 10;
        }
        i64::from(b) - i64::from(b'0')
    }

    /// Go: `is_z`: NUL, the end of the stream.
    pub(crate) fn is_z(&self, i: usize) -> bool {
        self.at(i) == 0
    }

    /// Go: `is_bom`. Go ignores `i` and checks the first three bytes of the buffer.
    pub(crate) fn is_bom(&self) -> bool {
        self.at(0) == 0xEF && self.at(1) == 0xBB && self.at(2) == 0xBF
    }

    /// Go: `is_space`.
    pub(crate) fn is_space(&self, i: usize) -> bool {
        self.at(i) == b' '
    }

    /// Go: `is_tab`.
    pub(crate) fn is_tab(&self, i: usize) -> bool {
        self.at(i) == b'\t'
    }

    /// Go: `is_blank`: a space or a tab.
    pub(crate) fn is_blank(&self, i: usize) -> bool {
        let b = self.at(i);
        b == b' ' || b == b'\t'
    }

    /// Go: `is_break`: CR, LF, NEL, LS or PS.
    pub(crate) fn is_break(&self, i: usize) -> bool {
        let b = self.at(i);
        b == b'\r'
            || b == b'\n'
            || (b == 0xC2 && self.at(i + 1) == 0x85)
            || (b == 0xE2 && self.at(i + 1) == 0x80 && self.at(i + 2) == 0xA8)
            || (b == 0xE2 && self.at(i + 1) == 0x80 && self.at(i + 2) == 0xA9)
    }

    /// Go: `is_crlf`.
    pub(crate) fn is_crlf(&self, i: usize) -> bool {
        self.at(i) == b'\r' && self.at(i + 1) == b'\n'
    }

    /// Go: `is_breakz`: a break or NUL.
    pub(crate) fn is_breakz(&self, i: usize) -> bool {
        self.is_break(i) || self.at(i) == 0
    }

    /// Go: `is_blankz`: a blank, a break or NUL.
    pub(crate) fn is_blankz(&self, i: usize) -> bool {
        self.is_blank(i) || self.is_breakz(i)
    }
}
