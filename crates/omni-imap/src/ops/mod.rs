//! Mailbox workflows over [`crate::protocol::ImapClient`]. They run while the
//! transport holds the operation permit.

pub mod archive;
pub mod auto_read;
pub mod drafts;
pub mod sent;
