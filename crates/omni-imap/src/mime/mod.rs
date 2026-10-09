//! A mailparser-compatible MIME parser (mailparser 3.9 / mailsplit / libmime
//! semantics) for the fields this crate persists or compares: Message-ID
//! normalization, threading headers, addresses, text and HTML bodies, and
//! attachments with mailparser's `partId` numbering.

pub mod address;
mod charset;
mod date;
mod header_value;
mod mimetypes;
mod parse;
mod splitter;
mod transfer;
mod words;

pub use address::Address;
pub use parse::{Attachment, ParseError, ParsedMail, parse_message};
pub use splitter::{HeaderLine, SplitError};
