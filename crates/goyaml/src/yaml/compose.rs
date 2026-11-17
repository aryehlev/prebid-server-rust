//! The composer (Go: the `parser` type of `decode.go`): builds the node tree of a document
//! from the event stream.

use std::{collections::HashMap, mem};

use super::{
    UnmarshalError,
    decode::NodeId,
    parserc::Events as _,
    resolve::{MAP_TAG, MERGE_TAG, SEQ_TAG, STR_TAG, resolve, short_tag},
    yamlh::{ErrorType, Event, EventType, Parser, ScalarStyle},
};

/// Go: `yaml.Kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// Go: `DocumentNode`.
    Document,
    /// Go: `SequenceNode`.
    Sequence,
    /// Go: `MappingNode`.
    Mapping,
    /// Go: `ScalarNode`.
    Scalar,
    /// Go: `AliasNode`.
    Alias,
}

/// Go: `yaml.Node`, as far as decoding reads it.
#[derive(Clone, Debug)]
pub(super) struct Node {
    /// Go: `Kind`.
    pub(crate) kind: Kind,
    /// Whether the scalar is quoted, literal or folded (Go: `Style` has one of those bits).
    pub(crate) quoted: bool,
    /// Go: `Tag`: the short form of an explicit tag, or the default or resolved tag.
    pub(crate) tag: String,
    /// Go: `Value`: the scalar text, or the anchor name of an alias.
    pub(crate) value: String,
    /// Go: `Alias`: the anchored node an alias refers to.
    pub(crate) alias: NodeId,
    /// Go: `Content`.
    pub(crate) content: Vec<NodeId>,
    /// Go: `Line`.
    pub(crate) line: i64,
}

/// Go: `parser` of `decode.go`.
#[derive(Debug)]
pub(super) struct Composer<'a> {
    /// Go: `parser`.
    parser: Parser<'a>,
    /// Go: `event`, the peeked event.
    event: Event,
    /// The node arena.
    pub(crate) nodes: Vec<Node>,
    /// Go: `anchors`.
    anchors: HashMap<Vec<u8>, NodeId>,
    /// Go: `doneInit`.
    done_init: bool,
}

impl<'a> Composer<'a> {
    /// Go: `newParser` / `newParserFromReader` (the caller substitutes empty input).
    pub(crate) fn new(input: &'a [u8]) -> Self {
        Self {
            parser: Parser::new(input),
            event: Event::default(),
            nodes: Vec::new(),
            anchors: HashMap::new(),
            done_init: false,
        }
    }

    /// Go: `init`.
    fn init(&mut self) -> Result<(), UnmarshalError> {
        if self.done_init {
            return Ok(());
        }
        self.expect(EventType::StreamStart)?;
        self.done_init = true;
        Ok(())
    }

    /// Go: `expect`: consumes an event of type `e`.
    fn expect(&mut self, e: EventType) -> Result<(), UnmarshalError> {
        if self.event.typ == EventType::NoEvent && !self.parser.parse(&mut self.event) {
            return Err(self.fail());
        }
        if self.event.typ == EventType::StreamEnd {
            return Err(UnmarshalError::failf(
                "attempted to go past the end of stream; corrupted value?",
            ));
        }
        if self.event.typ != e {
            self.parser.problem = format!(
                "expected {} event but got {}",
                e.name(),
                self.event.typ.name()
            );
            return Err(self.fail());
        }
        self.event = Event::default();
        Ok(())
    }

    /// Go: `peek`: the type of the next event.
    fn peek(&mut self) -> Result<EventType, UnmarshalError> {
        if self.event.typ != EventType::NoEvent {
            return Ok(self.event.typ);
        }
        // It's curious choice from the underlying API to generally return a positive result
        // on success, but on this case return true in an error scenario.
        if !self.parser.parse(&mut self.event) || self.parser.error != ErrorType::None {
            return Err(self.fail());
        }
        Ok(self.event.typ)
    }

    /// Go: `fail`: the parser's error as a fatal error.
    fn fail(&self) -> UnmarshalError {
        let p = &self.parser;
        let mut line = 0;
        if p.context_mark.line != 0 {
            line = p.context_mark.line;
            // Scanner errors don't iterate line before returning error.
            if p.error == ErrorType::Scanner {
                line += 1;
            }
        } else if p.problem_mark.line != 0 {
            line = p.problem_mark.line;
            // Scanner errors don't iterate line before returning error.
            if p.error == ErrorType::Scanner {
                line += 1;
            }
        }
        let place = if line == 0 {
            String::new()
        } else {
            format!("line {line}: ")
        };
        let msg = if p.problem.is_empty() {
            "unknown problem parsing YAML content"
        } else {
            &p.problem
        };
        UnmarshalError::failf(&format!("{place}{msg}"))
    }

    /// Go: `anchor`: registers the node under the event's anchor.
    fn anchor(&mut self, n: NodeId, anchor: Vec<u8>) {
        if !anchor.is_empty() {
            let _previous = self.anchors.insert(anchor, n);
        }
    }

    /// Go: `parse`: the next node, or `None` at the end of the stream.
    pub(crate) fn parse(&mut self) -> Result<Option<NodeId>, UnmarshalError> {
        self.init()?;
        let node = match self.peek()? {
            EventType::Scalar => self.scalar(),
            EventType::Alias => self.alias(),
            EventType::MappingStart => self.mapping(),
            EventType::SequenceStart => self.sequence(),
            EventType::DocumentStart => self.document(),
            // Happens when attempting to decode an empty buffer.
            EventType::StreamEnd => return Ok(None),
            other @ (EventType::NoEvent
            | EventType::StreamStart
            | EventType::DocumentEnd
            | EventType::SequenceEnd
            | EventType::MappingEnd) => {
                return Err(UnmarshalError::Panic(format!(
                    "internal error: attempted to parse unknown event (please report): {}",
                    other.name()
                )));
            }
        }?;
        Ok(Some(node))
    }

    /// Go: `node`: a new node of the current event.
    fn node(&mut self, kind: Kind, default_tag: &[u8], tag: &[u8], value: String) -> NodeId {
        let tag = if !tag.is_empty() && tag != b"!" {
            String::from_utf8_lossy(&short_tag(tag)).into_owned()
        } else if !default_tag.is_empty() {
            String::from_utf8_lossy(default_tag).into_owned()
        } else if kind == Kind::Scalar {
            resolve(b"", &value).map_or_else(
                |_| String::new(),
                |(rtag, _)| String::from_utf8_lossy(&rtag).into_owned(),
            )
        } else {
            String::new()
        };
        let id = self.nodes.len();
        self.nodes.push(Node {
            kind,
            quoted: false,
            tag,
            value,
            alias: id,
            content: Vec::new(),
            line: self.event.start_mark.line + 1,
        });
        id
    }

    /// Go: `parseChild`.
    fn parse_child(&mut self, parent: NodeId) -> Result<NodeId, UnmarshalError> {
        let Some(child) = self.parse()? else {
            // Go appends a nil child, which the decoder then dereferences.
            return Err(UnmarshalError::Panic(
                "runtime error: invalid memory address or nil pointer dereference".to_owned(),
            ));
        };
        if let Some(node) = self.nodes.get_mut(parent) {
            node.content.push(child);
        }
        Ok(child)
    }

    /// Go: `document`.
    fn document(&mut self) -> Result<NodeId, UnmarshalError> {
        let n = self.node(Kind::Document, b"", b"", String::new());
        self.expect(EventType::DocumentStart)?;
        let _child = self.parse_child(n)?;
        let _end = self.peek()?;
        self.expect(EventType::DocumentEnd)?;
        Ok(n)
    }

    /// Go: `alias`.
    fn alias(&mut self) -> Result<NodeId, UnmarshalError> {
        let name = String::from_utf8_lossy(&self.event.anchor).into_owned();
        let n = self.node(Kind::Alias, b"", b"", name);
        let Some(&target) = self.anchors.get(&self.event.anchor) else {
            let anchor = self.nodes.get(n).map_or_default(|node| node.value.as_str());
            return Err(UnmarshalError::failf(&format!(
                "unknown anchor '{anchor}' referenced"
            )));
        };
        if let Some(node) = self.nodes.get_mut(n) {
            node.alias = target;
        }
        self.expect(EventType::Alias)?;
        Ok(n)
    }

    /// Go: `scalar`.
    fn scalar(&mut self) -> Result<NodeId, UnmarshalError> {
        let quoted = !matches!(
            self.event.scalar_style,
            ScalarStyle::Any | ScalarStyle::Plain
        );
        let value = String::from_utf8_lossy(&self.event.value).into_owned();
        let default_tag: &[u8] = if quoted {
            STR_TAG
        } else if value == "<<" {
            MERGE_TAG
        } else {
            b""
        };
        let tag = mem::take(&mut self.event.tag);
        let n = self.node(Kind::Scalar, default_tag, &tag, value);
        if let Some(node) = self.nodes.get_mut(n) {
            node.quoted = quoted;
        }
        let anchor = mem::take(&mut self.event.anchor);
        self.anchor(n, anchor);
        self.expect(EventType::Scalar)?;
        Ok(n)
    }

    /// Go: `sequence`.
    fn sequence(&mut self) -> Result<NodeId, UnmarshalError> {
        let tag = mem::take(&mut self.event.tag);
        let n = self.node(Kind::Sequence, SEQ_TAG, &tag, String::new());
        let anchor = mem::take(&mut self.event.anchor);
        self.anchor(n, anchor);
        self.expect(EventType::SequenceStart)?;
        while self.peek()? != EventType::SequenceEnd {
            let _child = self.parse_child(n)?;
        }
        self.expect(EventType::SequenceEnd)?;
        Ok(n)
    }

    /// Go: `mapping`.
    fn mapping(&mut self) -> Result<NodeId, UnmarshalError> {
        let tag = mem::take(&mut self.event.tag);
        let n = self.node(Kind::Mapping, MAP_TAG, &tag, String::new());
        let anchor = mem::take(&mut self.event.anchor);
        self.anchor(n, anchor);
        self.expect(EventType::MappingStart)?;
        while self.peek()? != EventType::MappingEnd {
            let _key = self.parse_child(n)?;
            let _value = self.parse_child(n)?;
        }
        self.expect(EventType::MappingEnd)?;
        Ok(n)
    }
}
