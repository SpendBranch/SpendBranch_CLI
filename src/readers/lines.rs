//! Reads complete lines from a stream with bounded memory: a line longer than the cap is
//! skipped without being stored (it cannot be a usage record), and a final line with no
//! line break yet is left for the next read.
use std::io::{self, BufRead};

/// Longest line we keep. Prompts and tool output can be huge; usage records are not.
pub const MAX_LINE: usize = 4 * 1024 * 1024;

pub enum Next {
    /// A complete line is in the buffer, unless it was `too_long` (then the buffer is empty).
    Line { too_long: bool },
    /// No more complete lines.
    End,
}

pub struct Lines<R> {
    reader: R,
    max: usize,
    /// Absolute offset just after the last complete line returned.
    pub complete_end: u64,
}

impl<R: BufRead> Lines<R> {
    pub fn new(reader: R, start: u64, max: usize) -> Self {
        Self {
            reader,
            max,
            complete_end: start,
        }
    }

    pub fn into_inner(self) -> R {
        self.reader
    }

    /// The next line, without its `\n`, in `buf` (cleared first).
    pub fn next(&mut self, buf: &mut Vec<u8>) -> io::Result<Next> {
        buf.clear();
        let (mut too_long, mut taken) = (false, 0u64);
        loop {
            let chunk = self.reader.fill_buf()?;
            if chunk.is_empty() {
                return Ok(Next::End);
            }
            let end = chunk.iter().position(|&b| b == b'\n');
            let part = &chunk[..end.unwrap_or(chunk.len())];
            if !too_long {
                if buf.len() + part.len() > self.max {
                    too_long = true;
                    buf.clear();
                } else {
                    buf.extend_from_slice(part);
                }
            }
            let used = end.map_or(chunk.len(), |i| i + 1);
            self.reader.consume(used);
            taken += used as u64;
            if end.is_some() {
                self.complete_end += taken;
                return Ok(Next::Line { too_long });
            }
        }
    }
}
