//! The parser (Go: `parserc.go`): turns the token stream into events.
//!
//! The grammar, the states and the error texts are Go's. Event comments are not built: only
//! the parser's pending head comment is tracked, because its length decides whether a sequence
//! entry peeks one more token (`yaml_parser_split_stem_comment`), and that peek retries a token
//! fetch that failed.

use std::mem;

use super::{
    scannerc::Scanner as _,
    yamlh::{
        ErrorType, Event, EventType, Mark, Parser, ParserState, ScalarStyle, TagDirective,
        TokenRef, TokenType,
    },
};

/// The head token of the queue, as `peek_token` returns it.
#[derive(Clone, Copy, Debug)]
pub(super) struct Peeked {
    /// The token type.
    typ: TokenType,
    /// The beginning of the token.
    start_mark: Mark,
    /// The end of the token.
    end_mark: Mark,
}

/// The properties of a node (Go: the locals of `yaml_parser_parse_node`).
#[derive(Debug)]
pub(super) struct NodeProperties {
    /// The start of the node: its first property, or its content.
    start_mark: Mark,
    /// The ANCHOR value.
    anchor: Vec<u8>,
    /// The resolved TAG.
    tag: Vec<u8>,
}

/// Go: `default_tag_directives`.
const DEFAULT_TAG_DIRECTIVES: [(&[u8], &[u8]); 2] = [(b"!", b"!"), (b"!!", b"tag:yaml.org,2002:")];

/// The event parser functions of the parser (Go: `parserc.go`).
pub(super) trait Events {
    /// Go: `peek_token`: the head of the token queue, fetching tokens as needed.
    fn peek_token(&mut self) -> Option<Peeked>;

    /// Go: `yaml_parser_unfold_comments`: joins the comments behind the token into the parser's
    /// pending comment. Only the head comment is kept (see the module docs).
    fn unfold_comments(&mut self, token: Peeked);

    /// Go: `skip_token`: removes the head token from the queue.
    fn skip_token(&mut self);

    /// Takes the value of the head token (Go shares the token's bytes with the event).
    fn take_token_value(&mut self) -> Vec<u8>;

    /// Go: `yaml_parser_parse`: the next event. As in Go, it returns `true` with an empty event
    /// after the end of the stream or once an error is set.
    fn parse(&mut self, event: &mut Event) -> bool;

    /// Go: `yaml_parser_set_parser_error`.
    fn set_parser_error(&mut self, problem: &str, problem_mark: Mark) -> bool;

    /// Go: `yaml_parser_set_parser_error_context`. The context text is not kept.
    fn set_parser_error_context(
        &mut self,
        context_mark: Mark,
        problem: &str,
        problem_mark: Mark,
    ) -> bool;

    /// Pops the parser state (Go: `parser.states[len(parser.states)-1]`).
    fn pop_state(&mut self) -> ParserState;

    /// Pops the marks stack.
    fn pop_mark(&mut self) -> Mark;

    /// Go: `set_event_comments`: hands the pending comments to the event.
    fn set_event_comments(&mut self);

    /// Go: `yaml_parser_state_machine`.
    fn state_machine(&mut self, event: &mut Event) -> bool;

    /// Go: `yaml_parser_parse_stream_start`.
    ///
    /// ```text
    /// stream   ::= STREAM-START implicit_document? explicit_document* STREAM-END
    ///              ************
    /// ```
    fn parse_stream_start(&mut self, event: &mut Event) -> bool;

    /// Go: `yaml_parser_parse_document_start`.
    ///
    /// ```text
    /// implicit_document    ::= block_node DOCUMENT-END*
    ///                          *
    /// explicit_document    ::= DIRECTIVE* DOCUMENT-START block_node? DOCUMENT-END*
    ///                          *************************
    /// ```
    fn parse_document_start(&mut self, event: &mut Event, implicit: bool) -> bool;

    /// The head comment split of an implicit document start: the part before the last empty
    /// line goes to the document, the rest stays pending.
    fn split_document_head_comment(&mut self);

    /// Go: `yaml_parser_parse_document_content`.
    ///
    /// ```text
    /// explicit_document    ::= DIRECTIVE* DOCUMENT-START block_node? DOCUMENT-END*
    ///                                                    ***********
    /// ```
    fn parse_document_content(&mut self, event: &mut Event) -> bool;

    /// Go: `yaml_parser_parse_document_end`.
    ///
    /// ```text
    /// implicit_document    ::= block_node DOCUMENT-END*
    ///                                     *************
    /// explicit_document    ::= DIRECTIVE* DOCUMENT-START block_node? DOCUMENT-END*
    /// ```
    fn parse_document_end(&mut self, event: &mut Event) -> bool;

    /// Go: `yaml_parser_parse_node`.
    ///
    /// ```text
    /// block_node_or_indentless_sequence    ::=
    ///                          ALIAS
    ///                          *****
    ///                          | properties (block_content | indentless_block_sequence)?
    ///                            **********  *
    ///                          | block_content | indentless_block_sequence
    ///                            *
    /// block_node           ::= ALIAS
    ///                          *****
    ///                          | properties block_content?
    ///                            ********** *
    ///                          | block_content
    ///                            *
    /// flow_node            ::= ALIAS
    ///                          *****
    ///                          | properties flow_content?
    ///                            ********** *
    ///                          | flow_content
    ///                            *
    /// properties           ::= TAG ANCHOR? | ANCHOR TAG?
    ///                          *************************
    /// block_content        ::= block_collection | flow_collection | SCALAR
    ///                                                               ******
    /// flow_content         ::= flow_collection | SCALAR
    ///                                            ******
    /// ```
    fn parse_node(&mut self, event: &mut Event, block: bool, indentless_sequence: bool) -> bool;

    /// The `properties` of `yaml_parser_parse_node`: the ANCHOR and TAG tokens (in either
    /// order) before the content, with the tag resolved against the tag directives. Returns
    /// the token after them, or `None` once an error is set.
    fn parse_node_properties(&mut self, token: Peeked) -> Option<(Peeked, NodeProperties)>;

    /// The content of `yaml_parser_parse_node`, after the properties.
    fn parse_node_content(
        &mut self,
        event: &mut Event,
        block: bool,
        indentless_sequence: bool,
        token: Peeked,
        properties: NodeProperties,
    ) -> bool;

    /// Takes the handle and suffix of the head (TAG) token.
    fn take_tag(&mut self) -> (Vec<u8>, Vec<u8>);

    /// Go: `yaml_parser_parse_block_sequence_entry`.
    ///
    /// ```text
    /// block_sequence ::= BLOCK-SEQUENCE-START (BLOCK-ENTRY block_node?)* BLOCK-END
    ///                    ********************  *********** *             *********
    /// ```
    fn parse_block_sequence_entry(&mut self, event: &mut Event, first: bool) -> bool;

    /// Go: `yaml_parser_parse_indentless_sequence_entry`.
    ///
    /// ```text
    /// indentless_sequence  ::= (BLOCK-ENTRY block_node?)+
    ///                           *********** *
    /// ```
    fn parse_indentless_sequence_entry(&mut self, event: &mut Event) -> bool;

    /// Go: `yaml_parser_split_stem_comment`: when a sequence or mapping starts under a
    /// sequence entry, the prior head comment becomes the collection's stem comment.
    fn split_stem_comment(&mut self, stem_len: usize);

    /// Go: `yaml_parser_parse_block_mapping_key`.
    ///
    /// ```text
    /// block_mapping        ::= BLOCK-MAPPING_START
    ///                          *******************
    ///                          ((KEY block_node_or_indentless_sequence?)?
    ///                            *** *
    ///                          (VALUE block_node_or_indentless_sequence?)?)*
    ///
    ///                          BLOCK-END
    ///                          *********
    /// ```
    fn parse_block_mapping_key(&mut self, event: &mut Event, first: bool) -> bool;

    /// Go: `yaml_parser_parse_block_mapping_value`.
    ///
    /// ```text
    /// block_mapping        ::= BLOCK-MAPPING_START
    ///
    ///                          ((KEY block_node_or_indentless_sequence?)?
    ///
    ///                          (VALUE block_node_or_indentless_sequence?)?)*
    ///                           ***** *
    ///                          BLOCK-END
    /// ```
    fn parse_block_mapping_value(&mut self, event: &mut Event) -> bool;

    /// Go: `yaml_parser_parse_flow_sequence_entry`.
    ///
    /// ```text
    /// flow_sequence        ::= FLOW-SEQUENCE-START
    ///                          *******************
    ///                          (flow_sequence_entry FLOW-ENTRY)*
    ///                           *                   **********
    ///                          flow_sequence_entry?
    ///                          *
    ///                          FLOW-SEQUENCE-END
    ///                          *****************
    /// flow_sequence_entry  ::= flow_node | KEY flow_node? (VALUE flow_node?)?
    ///                          *
    /// ```
    fn parse_flow_sequence_entry(&mut self, event: &mut Event, first: bool) -> bool;

    /// Go: `yaml_parser_parse_flow_sequence_entry_mapping_key`. As in Go, the token after the
    /// implicit key is consumed when the key is empty.
    ///
    /// ```text
    /// flow_sequence_entry  ::= flow_node | KEY flow_node? (VALUE flow_node?)?
    ///                                      *** *
    /// ```
    fn parse_flow_sequence_entry_mapping_key(&mut self, event: &mut Event) -> bool;

    /// Go: `yaml_parser_parse_flow_sequence_entry_mapping_value`.
    ///
    /// ```text
    /// flow_sequence_entry  ::= flow_node | KEY flow_node? (VALUE flow_node?)?
    ///                                                      ***** *
    /// ```
    fn parse_flow_sequence_entry_mapping_value(&mut self, event: &mut Event) -> bool;

    /// Go: `yaml_parser_parse_flow_sequence_entry_mapping_end`.
    ///
    /// ```text
    /// flow_sequence_entry  ::= flow_node | KEY flow_node? (VALUE flow_node?)?
    ///                                                                      *
    /// ```
    fn parse_flow_sequence_entry_mapping_end(&mut self, event: &mut Event) -> bool;

    /// Go: `yaml_parser_parse_flow_mapping_key`.
    ///
    /// ```text
    /// flow_mapping         ::= FLOW-MAPPING-START
    ///                          ******************
    ///                          (flow_mapping_entry FLOW-ENTRY)*
    ///                           *                  **********
    ///                          flow_mapping_entry?
    ///                          ******************
    ///                          FLOW-MAPPING-END
    ///                          ****************
    /// flow_mapping_entry   ::= flow_node | KEY flow_node? (VALUE flow_node?)?
    ///                          *           *** *
    /// ```
    fn parse_flow_mapping_key(&mut self, event: &mut Event, first: bool) -> bool;

    /// Go: `yaml_parser_parse_flow_mapping_value`.
    ///
    /// ```text
    /// flow_mapping_entry   ::= flow_node | KEY flow_node? (VALUE flow_node?)?
    ///                                   *                  ***** *
    /// ```
    fn parse_flow_mapping_value(&mut self, event: &mut Event, empty: bool) -> bool;

    /// Go: `yaml_parser_process_empty_scalar`.
    fn process_empty_scalar(event: &mut Event, mark: Mark) -> bool;

    /// Go: `yaml_parser_process_directives`.
    fn process_directives(&mut self) -> bool;

    /// Go: `yaml_parser_append_tag_directive`.
    fn append_tag_directive(
        &mut self,
        value: TagDirective,
        allow_duplicates: bool,
        mark: Mark,
    ) -> bool;
}

impl Events for Parser<'_> {
    /// Go: `peek_token`: the head of the token queue, fetching tokens as needed.
    fn peek_token(&mut self) -> Option<Peeked> {
        if !(self.token_available || self.fetch_more_tokens()) {
            return None;
        }
        let token = self.tokens.front()?;
        let peeked = Peeked {
            typ: token.typ,
            start_mark: token.start_mark,
            end_mark: token.end_mark,
        };
        self.unfold_comments(peeked);
        Some(peeked)
    }

    /// Go: `yaml_parser_unfold_comments`: joins the comments behind the token into the parser's
    /// pending comment. Only the head comment is kept (see the module docs).
    fn unfold_comments(&mut self, token: Peeked) {
        while let Some(comment) = self.comments.get_mut(self.comments_head) {
            if token.start_mark.index < comment.token_mark.index {
                break;
            }
            if !comment.head.is_empty() {
                if token.typ == TokenType::BlockEnd {
                    // No heads on ends, so keep comment.head for a follow up token.
                    break;
                }
                if !self.head_comment.is_empty() {
                    self.head_comment.push(b'\n');
                }
                self.head_comment.extend_from_slice(&comment.head);
            }
            *comment = super::yamlh::Comment::default();
            self.comments_head += 1;
        }
    }

    /// Go: `skip_token`: removes the head token from the queue.
    fn skip_token(&mut self) {
        self.token_available = false;
        self.tokens_parsed += 1;
        if let Some(token) = self.tokens.pop_front() {
            self.tokens_head += 1;
            self.stream.end_produced = token.typ == TokenType::StreamEnd;
            self.consumed = [
                self.consumed[1],
                Some(TokenRef {
                    typ: token.typ,
                    start_mark: token.start_mark,
                }),
            ];
        }
    }

    /// Takes the value of the head token (Go shares the token's bytes with the event).
    fn take_token_value(&mut self) -> Vec<u8> {
        self.tokens
            .front_mut()
            .map_or_default(|t| mem::take(&mut t.value))
    }

    /// Go: `yaml_parser_parse`: the next event. As in Go, it returns `true` with an empty event
    /// after the end of the stream or once an error is set.
    fn parse(&mut self, event: &mut Event) -> bool {
        // Erase the event object.
        *event = Event::default();

        // No events after the end of the stream or error.
        if self.stream.end_produced
            || self.error != ErrorType::None
            || self.state == ParserState::End
        {
            return true;
        }

        // Generate the next event.
        self.state_machine(event)
    }

    /// Go: `yaml_parser_set_parser_error`.
    fn set_parser_error(&mut self, problem: &str, problem_mark: Mark) -> bool {
        self.error = ErrorType::Parser;
        problem.clone_into(&mut self.problem);
        self.problem_mark = problem_mark;
        false
    }

    /// Go: `yaml_parser_set_parser_error_context`. The context text is not kept.
    fn set_parser_error_context(
        &mut self,
        context_mark: Mark,
        problem: &str,
        problem_mark: Mark,
    ) -> bool {
        self.error = ErrorType::Parser;
        self.context_mark = context_mark;
        problem.clone_into(&mut self.problem);
        self.problem_mark = problem_mark;
        false
    }

    /// Pops the parser state (Go: `parser.states[len(parser.states)-1]`).
    fn pop_state(&mut self) -> ParserState {
        self.states.pop().unwrap_or(ParserState::End)
    }

    /// Pops the marks stack.
    fn pop_mark(&mut self) -> Mark {
        self.marks.pop().unwrap_or_default()
    }

    /// Go: `set_event_comments`: hands the pending comments to the event.
    fn set_event_comments(&mut self) {
        self.head_comment.clear();
    }

    /// Go: `yaml_parser_state_machine`.
    fn state_machine(&mut self, event: &mut Event) -> bool {
        match self.state {
            ParserState::StreamStart => self.parse_stream_start(event),
            ParserState::ImplicitDocumentStart => self.parse_document_start(event, true),
            ParserState::DocumentStart => self.parse_document_start(event, false),
            ParserState::DocumentContent => self.parse_document_content(event),
            ParserState::DocumentEnd => self.parse_document_end(event),
            ParserState::BlockNode => self.parse_node(event, true, false),
            ParserState::BlockSequenceFirstEntry => self.parse_block_sequence_entry(event, true),
            ParserState::BlockSequenceEntry => self.parse_block_sequence_entry(event, false),
            ParserState::IndentlessSequenceEntry => self.parse_indentless_sequence_entry(event),
            ParserState::BlockMappingFirstKey => self.parse_block_mapping_key(event, true),
            ParserState::BlockMappingKey => self.parse_block_mapping_key(event, false),
            ParserState::BlockMappingValue => self.parse_block_mapping_value(event),
            ParserState::FlowSequenceFirstEntry => self.parse_flow_sequence_entry(event, true),
            ParserState::FlowSequenceEntry => self.parse_flow_sequence_entry(event, false),
            ParserState::FlowSequenceEntryMappingKey => {
                self.parse_flow_sequence_entry_mapping_key(event)
            }
            ParserState::FlowSequenceEntryMappingValue => {
                self.parse_flow_sequence_entry_mapping_value(event)
            }
            ParserState::FlowSequenceEntryMappingEnd => {
                self.parse_flow_sequence_entry_mapping_end(event)
            }
            ParserState::FlowMappingFirstKey => self.parse_flow_mapping_key(event, true),
            ParserState::FlowMappingKey => self.parse_flow_mapping_key(event, false),
            ParserState::FlowMappingValue => self.parse_flow_mapping_value(event, false),
            ParserState::FlowMappingEmptyValue => self.parse_flow_mapping_value(event, true),
            // `yaml_parser_parse` never dispatches the end state (Go panics here).
            ParserState::End => true,
        }
    }

    /// Go: `yaml_parser_parse_stream_start`.
    ///
    /// ```text
    /// stream   ::= STREAM-START implicit_document? explicit_document* STREAM-END
    ///              ************
    /// ```
    fn parse_stream_start(&mut self, event: &mut Event) -> bool {
        let Some(token) = self.peek_token() else {
            return false;
        };
        if token.typ != TokenType::StreamStart {
            return self.set_parser_error("did not find expected <stream-start>", token.start_mark);
        }
        self.state = ParserState::ImplicitDocumentStart;
        *event = Event {
            typ: EventType::StreamStart,
            start_mark: token.start_mark,
            ..Event::default()
        };
        self.skip_token();
        true
    }

    /// Go: `yaml_parser_parse_document_start`.
    ///
    /// ```text
    /// implicit_document    ::= block_node DOCUMENT-END*
    ///                          *
    /// explicit_document    ::= DIRECTIVE* DOCUMENT-START block_node? DOCUMENT-END*
    ///                          *************************
    /// ```
    fn parse_document_start(&mut self, event: &mut Event, implicit: bool) -> bool {
        let Some(mut token) = self.peek_token() else {
            return false;
        };

        // Parse extra document end indicators.
        if !implicit {
            while token.typ == TokenType::DocumentEnd {
                self.skip_token();
                let Some(next) = self.peek_token() else {
                    return false;
                };
                token = next;
            }
        }

        if implicit
            && token.typ != TokenType::VersionDirective
            && token.typ != TokenType::TagDirective
            && token.typ != TokenType::DocumentStart
            && token.typ != TokenType::StreamEnd
        {
            // Parse an implicit document.
            if !self.process_directives() {
                return false;
            }
            self.states.push(ParserState::DocumentEnd);
            self.state = ParserState::BlockNode;

            // [Go] Scan the header comment backwards, and if an empty line is found, break
            //      the header so the part before the last empty line goes into the
            //      document header, while the bottom of it goes into a follow up event.
            self.split_document_head_comment();

            *event = Event {
                typ: EventType::DocumentStart,
                start_mark: token.start_mark,
                ..Event::default()
            };
        } else if token.typ != TokenType::StreamEnd {
            // Parse an explicit document.
            let start_mark = token.start_mark;
            if !self.process_directives() {
                return false;
            }
            let Some(next) = self.peek_token() else {
                return false;
            };
            if next.typ != TokenType::DocumentStart {
                return self
                    .set_parser_error("did not find expected <document start>", next.start_mark);
            }
            self.states.push(ParserState::DocumentEnd);
            self.state = ParserState::DocumentContent;
            *event = Event {
                typ: EventType::DocumentStart,
                start_mark,
                ..Event::default()
            };
            self.skip_token();
        } else {
            // Parse the stream end.
            self.state = ParserState::End;
            *event = Event {
                typ: EventType::StreamEnd,
                start_mark: token.start_mark,
                ..Event::default()
            };
            self.skip_token();
        }

        true
    }

    /// The head comment split of an implicit document start: the part before the last empty
    /// line goes to the document, the rest stays pending.
    fn split_document_head_comment(&mut self) {
        let len = self.head_comment.len();
        let mut i = len.saturating_sub(1);
        while i > 0 {
            if self.head_comment.get(i) == Some(&b'\n')
                && (i == len - 1 || self.head_comment.get(i - 1) == Some(&b'\n'))
            {
                self.head_comment = self
                    .head_comment
                    .get(i + 1..)
                    .map_or_default(<[u8]>::to_vec);
                break;
            }
            i -= 1;
        }
    }

    /// Go: `yaml_parser_parse_document_content`.
    ///
    /// ```text
    /// explicit_document    ::= DIRECTIVE* DOCUMENT-START block_node? DOCUMENT-END*
    ///                                                    ***********
    /// ```
    fn parse_document_content(&mut self, event: &mut Event) -> bool {
        let Some(token) = self.peek_token() else {
            return false;
        };

        if matches!(
            token.typ,
            TokenType::VersionDirective
                | TokenType::TagDirective
                | TokenType::DocumentStart
                | TokenType::DocumentEnd
                | TokenType::StreamEnd
        ) {
            self.state = self.pop_state();
            return Self::process_empty_scalar(event, token.start_mark);
        }
        self.parse_node(event, true, false)
    }

    /// Go: `yaml_parser_parse_document_end`.
    ///
    /// ```text
    /// implicit_document    ::= block_node DOCUMENT-END*
    ///                                     *************
    /// explicit_document    ::= DIRECTIVE* DOCUMENT-START block_node? DOCUMENT-END*
    /// ```
    fn parse_document_end(&mut self, event: &mut Event) -> bool {
        let Some(token) = self.peek_token() else {
            return false;
        };

        let start_mark = token.start_mark;
        if token.typ == TokenType::DocumentEnd {
            self.skip_token();
        }

        self.tag_directives.clear();

        self.state = ParserState::DocumentStart;
        *event = Event {
            typ: EventType::DocumentEnd,
            start_mark,
            ..Event::default()
        };
        self.set_event_comments();
        true
    }

    /// Go: `yaml_parser_parse_node`.
    ///
    /// ```text
    /// block_node_or_indentless_sequence    ::=
    ///                          ALIAS
    ///                          *****
    ///                          | properties (block_content | indentless_block_sequence)?
    ///                            **********  *
    ///                          | block_content | indentless_block_sequence
    ///                            *
    /// block_node           ::= ALIAS
    ///                          *****
    ///                          | properties block_content?
    ///                            ********** *
    ///                          | block_content
    ///                            *
    /// flow_node            ::= ALIAS
    ///                          *****
    ///                          | properties flow_content?
    ///                            ********** *
    ///                          | flow_content
    ///                            *
    /// properties           ::= TAG ANCHOR? | ANCHOR TAG?
    ///                          *************************
    /// block_content        ::= block_collection | flow_collection | SCALAR
    ///                                                               ******
    /// flow_content         ::= flow_collection | SCALAR
    ///                                            ******
    /// ```
    fn parse_node(&mut self, event: &mut Event, block: bool, indentless_sequence: bool) -> bool {
        let Some(token) = self.peek_token() else {
            return false;
        };

        if token.typ == TokenType::Alias {
            self.state = self.pop_state();
            *event = Event {
                typ: EventType::Alias,
                start_mark: token.start_mark,
                anchor: self.take_token_value(),
                ..Event::default()
            };
            self.set_event_comments();
            self.skip_token();
            return true;
        }

        let Some((content, properties)) = self.parse_node_properties(token) else {
            return false;
        };
        self.parse_node_content(event, block, indentless_sequence, content, properties)
    }

    fn parse_node_properties(&mut self, mut token: Peeked) -> Option<(Peeked, NodeProperties)> {
        let mut start_mark = token.start_mark;

        let mut tag_token = false;
        let mut tag_handle = Vec::new();
        let mut tag_suffix = Vec::new();
        let mut anchor = Vec::new();
        let mut tag_mark = Mark::default();
        if token.typ == TokenType::Anchor {
            anchor = self.take_token_value();
            start_mark = token.start_mark;
            self.skip_token();
            token = self.peek_token()?;
            if token.typ == TokenType::Tag {
                tag_token = true;
                (tag_handle, tag_suffix) = self.take_tag();
                tag_mark = token.start_mark;
                self.skip_token();
                token = self.peek_token()?;
            }
        } else if token.typ == TokenType::Tag {
            tag_token = true;
            (tag_handle, tag_suffix) = self.take_tag();
            start_mark = token.start_mark;
            tag_mark = token.start_mark;
            self.skip_token();
            token = self.peek_token()?;
            if token.typ == TokenType::Anchor {
                anchor = self.take_token_value();
                self.skip_token();
                token = self.peek_token()?;
            }
        }

        let mut tag = Vec::new();
        if tag_token {
            if tag_handle.is_empty() {
                tag = tag_suffix;
            } else {
                if let Some(directive) = self.tag_directives.iter().find(|d| d.handle == tag_handle)
                {
                    tag.clone_from(&directive.prefix);
                    tag.extend_from_slice(&tag_suffix);
                }
                if tag.is_empty() {
                    // "while parsing a node"
                    let _set = self.set_parser_error_context(
                        start_mark,
                        "found undefined tag handle",
                        tag_mark,
                    );
                    return None;
                }
            }
        }
        Some((
            token,
            NodeProperties {
                start_mark,
                anchor,
                tag,
            },
        ))
    }

    fn parse_node_content(
        &mut self,
        event: &mut Event,
        block: bool,
        indentless_sequence: bool,
        token: Peeked,
        properties: NodeProperties,
    ) -> bool {
        let NodeProperties {
            start_mark,
            anchor,
            tag,
        } = properties;
        if indentless_sequence && token.typ == TokenType::BlockEntry {
            self.state = ParserState::IndentlessSequenceEntry;
            *event = Event {
                typ: EventType::SequenceStart,
                start_mark,
                anchor,
                tag,
                ..Event::default()
            };
            return true;
        }
        if token.typ == TokenType::Scalar {
            self.state = self.pop_state();
            let style = self.tokens.front().map_or_default(|t| t.style);
            *event = Event {
                typ: EventType::Scalar,
                start_mark,
                anchor,
                tag,
                value: self.take_token_value(),
                scalar_style: style,
            };
            self.set_event_comments();
            self.skip_token();
            return true;
        }
        if token.typ == TokenType::FlowSequenceStart {
            // [Go] Some of the events below can be merged as they differ only on style.
            self.state = ParserState::FlowSequenceFirstEntry;
            *event = Event {
                typ: EventType::SequenceStart,
                start_mark,
                anchor,
                tag,
                ..Event::default()
            };
            self.set_event_comments();
            return true;
        }
        if token.typ == TokenType::FlowMappingStart {
            self.state = ParserState::FlowMappingFirstKey;
            *event = Event {
                typ: EventType::MappingStart,
                start_mark,
                anchor,
                tag,
                ..Event::default()
            };
            self.set_event_comments();
            return true;
        }
        if block && token.typ == TokenType::BlockSequenceStart {
            self.state = ParserState::BlockSequenceFirstEntry;
            *event = Event {
                typ: EventType::SequenceStart,
                start_mark,
                anchor,
                tag,
                ..Event::default()
            };
            return true;
        }
        if block && token.typ == TokenType::BlockMappingStart {
            self.state = ParserState::BlockMappingFirstKey;
            *event = Event {
                typ: EventType::MappingStart,
                start_mark,
                anchor,
                tag,
                ..Event::default()
            };
            return true;
        }
        if !anchor.is_empty() || !tag.is_empty() {
            self.state = self.pop_state();
            *event = Event {
                typ: EventType::Scalar,
                start_mark,
                anchor,
                tag,
                scalar_style: ScalarStyle::Plain,
                ..Event::default()
            };
            return true;
        }

        // "while parsing a block node" / "while parsing a flow node"
        self.set_parser_error_context(
            start_mark,
            "did not find expected node content",
            token.start_mark,
        )
    }

    /// Takes the handle and suffix of the head (TAG) token.
    fn take_tag(&mut self) -> (Vec<u8>, Vec<u8>) {
        self.tokens
            .front_mut()
            .map_or_default(|t| (mem::take(&mut t.value), mem::take(&mut t.suffix)))
    }

    /// Go: `yaml_parser_parse_block_sequence_entry`.
    ///
    /// ```text
    /// block_sequence ::= BLOCK-SEQUENCE-START (BLOCK-ENTRY block_node?)* BLOCK-END
    ///                    ********************  *********** *             *********
    /// ```
    fn parse_block_sequence_entry(&mut self, event: &mut Event, first: bool) -> bool {
        if first {
            let Some(token) = self.peek_token() else {
                return false;
            };
            self.marks.push(token.start_mark);
            self.skip_token();
        }

        let Some(token) = self.peek_token() else {
            return false;
        };

        if token.typ == TokenType::BlockEntry {
            let mark = token.end_mark;
            let prior_head_len = self.head_comment.len();
            self.skip_token();
            self.split_stem_comment(prior_head_len);
            let Some(next) = self.peek_token() else {
                return false;
            };
            if next.typ != TokenType::BlockEntry && next.typ != TokenType::BlockEnd {
                self.states.push(ParserState::BlockSequenceEntry);
                return self.parse_node(event, true, false);
            }
            self.state = ParserState::BlockSequenceEntry;
            return Self::process_empty_scalar(event, mark);
        }
        if token.typ == TokenType::BlockEnd {
            self.state = self.pop_state();
            let _mark = self.pop_mark();

            *event = Event {
                typ: EventType::SequenceEnd,
                start_mark: token.start_mark,
                ..Event::default()
            };

            self.skip_token();
            return true;
        }

        let context_mark = self.pop_mark();
        // "while parsing a block collection"
        self.set_parser_error_context(
            context_mark,
            "did not find expected '-' indicator",
            token.start_mark,
        )
    }

    /// Go: `yaml_parser_parse_indentless_sequence_entry`.
    ///
    /// ```text
    /// indentless_sequence  ::= (BLOCK-ENTRY block_node?)+
    ///                           *********** *
    /// ```
    fn parse_indentless_sequence_entry(&mut self, event: &mut Event) -> bool {
        let Some(token) = self.peek_token() else {
            return false;
        };

        if token.typ == TokenType::BlockEntry {
            let mark = token.end_mark;
            let prior_head_len = self.head_comment.len();
            self.skip_token();
            self.split_stem_comment(prior_head_len);
            let Some(next) = self.peek_token() else {
                return false;
            };
            if next.typ != TokenType::BlockEntry
                && next.typ != TokenType::Key
                && next.typ != TokenType::Value
                && next.typ != TokenType::BlockEnd
            {
                self.states.push(ParserState::IndentlessSequenceEntry);
                return self.parse_node(event, true, false);
            }
            self.state = ParserState::IndentlessSequenceEntry;
            return Self::process_empty_scalar(event, mark);
        }
        self.state = self.pop_state();

        *event = Event {
            typ: EventType::SequenceEnd,
            start_mark: token.start_mark,
            ..Event::default()
        };
        true
    }

    /// Go: `yaml_parser_split_stem_comment`: when a sequence or mapping starts under a
    /// sequence entry, the prior head comment becomes the collection's stem comment.
    fn split_stem_comment(&mut self, stem_len: usize) {
        if stem_len == 0 {
            return;
        }

        let Some(token) = self.peek_token() else {
            return;
        };
        if token.typ != TokenType::BlockSequenceStart && token.typ != TokenType::BlockMappingStart {
            return;
        }

        // The stem (`head_comment[:stem_len]`) only feeds event comments.
        if self.head_comment.len() == stem_len {
            self.head_comment.clear();
        } else {
            self.head_comment = self
                .head_comment
                .get(stem_len + 1..)
                .map_or_default(<[u8]>::to_vec);
        }
    }

    /// Go: `yaml_parser_parse_block_mapping_key`.
    ///
    /// ```text
    /// block_mapping        ::= BLOCK-MAPPING_START
    ///                          *******************
    ///                          ((KEY block_node_or_indentless_sequence?)?
    ///                            *** *
    ///                          (VALUE block_node_or_indentless_sequence?)?)*
    ///
    ///                          BLOCK-END
    ///                          *********
    /// ```
    fn parse_block_mapping_key(&mut self, event: &mut Event, first: bool) -> bool {
        if first {
            let Some(token) = self.peek_token() else {
                return false;
            };
            self.marks.push(token.start_mark);
            self.skip_token();
        }

        let Some(token) = self.peek_token() else {
            return false;
        };

        // [Go] The parser never sets a tail comment, so no TAIL-COMMENT event is produced.

        if token.typ == TokenType::Key {
            let mark = token.end_mark;
            self.skip_token();
            let Some(next) = self.peek_token() else {
                return false;
            };
            if next.typ != TokenType::Key
                && next.typ != TokenType::Value
                && next.typ != TokenType::BlockEnd
            {
                self.states.push(ParserState::BlockMappingValue);
                return self.parse_node(event, true, true);
            }
            self.state = ParserState::BlockMappingValue;
            return Self::process_empty_scalar(event, mark);
        } else if token.typ == TokenType::BlockEnd {
            self.state = self.pop_state();
            let _mark = self.pop_mark();
            *event = Event {
                typ: EventType::MappingEnd,
                start_mark: token.start_mark,
                ..Event::default()
            };
            self.set_event_comments();
            self.skip_token();
            return true;
        }

        let context_mark = self.pop_mark();
        // "while parsing a block mapping"
        self.set_parser_error_context(context_mark, "did not find expected key", token.start_mark)
    }

    /// Go: `yaml_parser_parse_block_mapping_value`.
    ///
    /// ```text
    /// block_mapping        ::= BLOCK-MAPPING_START
    ///
    ///                          ((KEY block_node_or_indentless_sequence?)?
    ///
    ///                          (VALUE block_node_or_indentless_sequence?)?)*
    ///                           ***** *
    ///                          BLOCK-END
    /// ```
    fn parse_block_mapping_value(&mut self, event: &mut Event) -> bool {
        let Some(token) = self.peek_token() else {
            return false;
        };
        if token.typ == TokenType::Value {
            let mark = token.end_mark;
            self.skip_token();
            let Some(next) = self.peek_token() else {
                return false;
            };
            if next.typ != TokenType::Key
                && next.typ != TokenType::Value
                && next.typ != TokenType::BlockEnd
            {
                self.states.push(ParserState::BlockMappingKey);
                return self.parse_node(event, true, true);
            }
            self.state = ParserState::BlockMappingKey;
            return Self::process_empty_scalar(event, mark);
        }
        self.state = ParserState::BlockMappingKey;
        Self::process_empty_scalar(event, token.start_mark)
    }

    /// Go: `yaml_parser_parse_flow_sequence_entry`.
    ///
    /// ```text
    /// flow_sequence        ::= FLOW-SEQUENCE-START
    ///                          *******************
    ///                          (flow_sequence_entry FLOW-ENTRY)*
    ///                           *                   **********
    ///                          flow_sequence_entry?
    ///                          *
    ///                          FLOW-SEQUENCE-END
    ///                          *****************
    /// flow_sequence_entry  ::= flow_node | KEY flow_node? (VALUE flow_node?)?
    ///                          *
    /// ```
    fn parse_flow_sequence_entry(&mut self, event: &mut Event, first: bool) -> bool {
        if first {
            let Some(token) = self.peek_token() else {
                return false;
            };
            self.marks.push(token.start_mark);
            self.skip_token();
        }
        let Some(mut token) = self.peek_token() else {
            return false;
        };
        if token.typ != TokenType::FlowSequenceEnd {
            if !first {
                if token.typ == TokenType::FlowEntry {
                    self.skip_token();
                    let Some(next) = self.peek_token() else {
                        return false;
                    };
                    token = next;
                } else {
                    let context_mark = self.pop_mark();
                    // "while parsing a flow sequence"
                    return self.set_parser_error_context(
                        context_mark,
                        "did not find expected ',' or ']'",
                        token.start_mark,
                    );
                }
            }

            if token.typ == TokenType::Key {
                self.state = ParserState::FlowSequenceEntryMappingKey;
                *event = Event {
                    typ: EventType::MappingStart,
                    start_mark: token.start_mark,
                    ..Event::default()
                };
                self.skip_token();
                return true;
            } else if token.typ != TokenType::FlowSequenceEnd {
                self.states.push(ParserState::FlowSequenceEntry);
                return self.parse_node(event, false, false);
            }
        }

        self.state = self.pop_state();
        let _mark = self.pop_mark();

        *event = Event {
            typ: EventType::SequenceEnd,
            start_mark: token.start_mark,
            ..Event::default()
        };
        self.set_event_comments();

        self.skip_token();
        true
    }

    /// Go: `yaml_parser_parse_flow_sequence_entry_mapping_key`. As in Go, the token after the
    /// implicit key is consumed when the key is empty.
    ///
    /// ```text
    /// flow_sequence_entry  ::= flow_node | KEY flow_node? (VALUE flow_node?)?
    ///                                      *** *
    /// ```
    fn parse_flow_sequence_entry_mapping_key(&mut self, event: &mut Event) -> bool {
        let Some(token) = self.peek_token() else {
            return false;
        };
        if token.typ != TokenType::Value
            && token.typ != TokenType::FlowEntry
            && token.typ != TokenType::FlowSequenceEnd
        {
            self.states.push(ParserState::FlowSequenceEntryMappingValue);
            return self.parse_node(event, false, false);
        }
        let mark = token.end_mark;
        self.skip_token();
        self.state = ParserState::FlowSequenceEntryMappingValue;
        Self::process_empty_scalar(event, mark)
    }

    /// Go: `yaml_parser_parse_flow_sequence_entry_mapping_value`.
    ///
    /// ```text
    /// flow_sequence_entry  ::= flow_node | KEY flow_node? (VALUE flow_node?)?
    ///                                                      ***** *
    /// ```
    fn parse_flow_sequence_entry_mapping_value(&mut self, event: &mut Event) -> bool {
        let Some(token) = self.peek_token() else {
            return false;
        };
        if token.typ == TokenType::Value {
            self.skip_token();
            let Some(next) = self.peek_token() else {
                return false;
            };
            if next.typ != TokenType::FlowEntry && next.typ != TokenType::FlowSequenceEnd {
                self.states.push(ParserState::FlowSequenceEntryMappingEnd);
                return self.parse_node(event, false, false);
            }
        }
        self.state = ParserState::FlowSequenceEntryMappingEnd;
        // [Go] The inner token shadows the outer one, so the mark is the VALUE token's (Go reads
        // it through a pointer into the token slice, which only `yaml.Node` positions could
        // observe if the slice were compacted in between).
        Self::process_empty_scalar(event, token.start_mark)
    }

    /// Go: `yaml_parser_parse_flow_sequence_entry_mapping_end`.
    ///
    /// ```text
    /// flow_sequence_entry  ::= flow_node | KEY flow_node? (VALUE flow_node?)?
    ///                                                                      *
    /// ```
    fn parse_flow_sequence_entry_mapping_end(&mut self, event: &mut Event) -> bool {
        let Some(token) = self.peek_token() else {
            return false;
        };
        self.state = ParserState::FlowSequenceEntry;
        *event = Event {
            typ: EventType::MappingEnd,
            start_mark: token.start_mark,
            ..Event::default()
        };
        true
    }

    /// Go: `yaml_parser_parse_flow_mapping_key`.
    ///
    /// ```text
    /// flow_mapping         ::= FLOW-MAPPING-START
    ///                          ******************
    ///                          (flow_mapping_entry FLOW-ENTRY)*
    ///                           *                  **********
    ///                          flow_mapping_entry?
    ///                          ******************
    ///                          FLOW-MAPPING-END
    ///                          ****************
    /// flow_mapping_entry   ::= flow_node | KEY flow_node? (VALUE flow_node?)?
    ///                          *           *** *
    /// ```
    fn parse_flow_mapping_key(&mut self, event: &mut Event, first: bool) -> bool {
        if first {
            // Go reads the token without a nil check; it is always available here.
            let Some(token) = self.peek_token() else {
                return false;
            };
            self.marks.push(token.start_mark);
            self.skip_token();
        }

        let Some(mut token) = self.peek_token() else {
            return false;
        };

        if token.typ != TokenType::FlowMappingEnd {
            if !first {
                if token.typ == TokenType::FlowEntry {
                    self.skip_token();
                    let Some(next) = self.peek_token() else {
                        return false;
                    };
                    token = next;
                } else {
                    let context_mark = self.pop_mark();
                    // "while parsing a flow mapping"
                    return self.set_parser_error_context(
                        context_mark,
                        "did not find expected ',' or '}'",
                        token.start_mark,
                    );
                }
            }

            if token.typ == TokenType::Key {
                self.skip_token();
                let Some(after_key) = self.peek_token() else {
                    return false;
                };
                if after_key.typ != TokenType::Value
                    && after_key.typ != TokenType::FlowEntry
                    && after_key.typ != TokenType::FlowMappingEnd
                {
                    self.states.push(ParserState::FlowMappingValue);
                    return self.parse_node(event, false, false);
                }
                self.state = ParserState::FlowMappingValue;
                return Self::process_empty_scalar(event, after_key.start_mark);
            } else if token.typ != TokenType::FlowMappingEnd {
                self.states.push(ParserState::FlowMappingEmptyValue);
                return self.parse_node(event, false, false);
            }
        }

        self.state = self.pop_state();
        let _mark = self.pop_mark();
        *event = Event {
            typ: EventType::MappingEnd,
            start_mark: token.start_mark,
            ..Event::default()
        };
        self.set_event_comments();
        self.skip_token();
        true
    }

    /// Go: `yaml_parser_parse_flow_mapping_value`.
    ///
    /// ```text
    /// flow_mapping_entry   ::= flow_node | KEY flow_node? (VALUE flow_node?)?
    ///                                   *                  ***** *
    /// ```
    fn parse_flow_mapping_value(&mut self, event: &mut Event, empty: bool) -> bool {
        let Some(mut token) = self.peek_token() else {
            return false;
        };
        if empty {
            self.state = ParserState::FlowMappingKey;
            return Self::process_empty_scalar(event, token.start_mark);
        }
        if token.typ == TokenType::Value {
            self.skip_token();
            let Some(next) = self.peek_token() else {
                return false;
            };
            token = next;
            if token.typ != TokenType::FlowEntry && token.typ != TokenType::FlowMappingEnd {
                self.states.push(ParserState::FlowMappingKey);
                return self.parse_node(event, false, false);
            }
        }
        self.state = ParserState::FlowMappingKey;
        Self::process_empty_scalar(event, token.start_mark)
    }

    /// Go: `yaml_parser_process_empty_scalar`.
    fn process_empty_scalar(event: &mut Event, mark: Mark) -> bool {
        *event = Event {
            typ: EventType::Scalar,
            start_mark: mark,
            scalar_style: ScalarStyle::Plain,
            ..Event::default()
        };
        true
    }

    /// Go: `yaml_parser_process_directives`.
    fn process_directives(&mut self) -> bool {
        let mut has_version = false;

        let Some(mut token) = self.peek_token() else {
            return false;
        };

        while token.typ == TokenType::VersionDirective || token.typ == TokenType::TagDirective {
            if token.typ == TokenType::VersionDirective {
                if has_version {
                    return self
                        .set_parser_error("found duplicate %YAML directive", token.start_mark);
                }
                let (major, minor) = self.tokens.front().map_or_default(|t| (t.major, t.minor));
                if major != 1 || minor != 1 {
                    return self
                        .set_parser_error("found incompatible YAML document", token.start_mark);
                }
                has_version = true;
            } else {
                let (handle, prefix) = self
                    .tokens
                    .front()
                    .map_or_default(|t| (t.value.clone(), t.prefix.clone()));
                if !self.append_tag_directive(
                    TagDirective { handle, prefix },
                    false,
                    token.start_mark,
                ) {
                    return false;
                }
            }

            self.skip_token();
            let Some(next) = self.peek_token() else {
                return false;
            };
            token = next;
        }

        for (handle, prefix) in DEFAULT_TAG_DIRECTIVES {
            let value = TagDirective {
                handle: handle.to_vec(),
                prefix: prefix.to_vec(),
            };
            if !self.append_tag_directive(value, true, token.start_mark) {
                return false;
            }
        }
        true
    }

    /// Go: `yaml_parser_append_tag_directive`.
    fn append_tag_directive(
        &mut self,
        value: TagDirective,
        allow_duplicates: bool,
        mark: Mark,
    ) -> bool {
        if self.tag_directives.iter().any(|d| d.handle == value.handle) {
            if allow_duplicates {
                return true;
            }
            return self.set_parser_error("found duplicate %TAG directive", mark);
        }
        self.tag_directives.push(value);
        true
    }
}
