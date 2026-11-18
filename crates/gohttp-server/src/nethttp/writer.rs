//! The write side of a server connection: Go's `conn.bufw`, a 4096-byte `bufio.Writer` over
//! `checkConnErrorWriter` (which records the first write error and cancels the connection's
//! context).

use core::mem;
use std::sync::Arc;

use tokio::sync::Mutex;

use super::{context::RequestContext, netconn::NetWriter};

/// Go: `newBufioWriterSize(checkConnErrorWriter{c}, 4<<10)`.
const CONN_BUF_SIZE: usize = 4 << 10;

/// Go: the connection's buffered writer.
#[derive(Debug)]
pub(super) struct WriteSide {
    net: NetWriter,
    buf: Vec<u8>,
    /// Go: `bufio.Writer.err` (sticky).
    err: Option<String>,
    /// Go: `conn.werr`.
    pub(super) werr: Option<String>,
    /// The context `checkConnErrorWriter` cancels.
    pub(super) cancel_on_error: Option<RequestContext>,
    /// Set once a write failed (Go cancels the conn context).
    pub(super) conn_cancelled: bool,
}

impl WriteSide {
    pub(super) fn new(net: NetWriter) -> Self {
        Self {
            net,
            buf: Vec::with_capacity(CONN_BUF_SIZE),
            err: None,
            werr: None,
            cancel_on_error: None,
            conn_cancelled: false,
        }
    }

    pub(super) const fn net_mut(&mut self) -> &mut NetWriter {
        &mut self.net
    }

    /// Go: `checkConnErrorWriter.Write`.
    async fn conn_write(&mut self, p: &[u8]) -> Result<(), String> {
        let result = self.net.write_all(p).await;
        if let Err(e) = &result
            && self.werr.is_none()
        {
            self.werr = Some(e.clone());
            self.conn_cancelled = true;
            if let Some(ctx) = &self.cancel_on_error {
                ctx.cancel();
            }
        }
        result
    }

    /// Go: `bufio.Writer.Write`.
    pub(super) async fn write(&mut self, mut p: &[u8]) -> Result<usize, String> {
        let mut nn = 0;
        while p.len() > CONN_BUF_SIZE - self.buf.len() && self.err.is_none() {
            let n = if self.buf.is_empty() {
                match self.conn_write(p).await {
                    Ok(()) => p.len(),
                    Err(e) => {
                        self.err = Some(e);
                        0
                    }
                }
            } else {
                let n = CONN_BUF_SIZE - self.buf.len();
                self.buf.extend_from_slice(p.get(..n).unwrap_or_default());
                let _flushed = self.flush().await;
                n
            };
            nn += n;
            p = p.get(n..).unwrap_or_default();
        }
        if let Some(e) = &self.err {
            return Err(e.clone());
        }
        self.buf.extend_from_slice(p);
        Ok(nn + p.len())
    }

    /// Go: `bufio.Writer.Flush`.
    pub(super) async fn flush(&mut self) -> Result<(), String> {
        if let Some(e) = &self.err {
            return Err(e.clone());
        }
        if self.buf.is_empty() {
            return Ok(());
        }
        let buf = mem::take(&mut self.buf);
        let result = self.conn_write(&buf).await;
        self.buf = buf;
        match result {
            Ok(()) => {
                self.buf.clear();
                Ok(())
            }
            Err(e) => {
                self.err = Some(e.clone());
                Err(e)
            }
        }
    }

    /// Go: `fmt.Fprintf(c.rwc, ...)`: a write that bypasses the buffer.
    pub(super) async fn write_direct(&mut self, p: &[u8]) {
        let _ignored = self.conn_write(p).await;
    }

    /// Go: `cw.res.conn.rwc.Close()` after a failed chunk write.
    pub(super) fn close_conn(&mut self) {
        self.net.close();
    }
}

/// Shared ownership of the write side.
pub(super) type SharedWriter = Arc<Mutex<WriteSide>>;
