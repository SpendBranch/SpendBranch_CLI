//! Holding back the last message of a file that may still be growing.
//!
//! One request streams in as several lines whose counters only grow. Sending the lines of
//! the last message while the file is still being written would freeze a smaller counter,
//! so they wait until a different message follows or the file has settled.
//! Pure ordering logic: it knows nothing about files.

struct Line {
    bytes: Vec<u8>,
    too_long: bool,
    /// Offset in the file where the line starts.
    start: u64,
}

#[derive(Default)]
pub struct OpenMessage {
    key: Option<String>,
    pending: Vec<Line>,
}

impl OpenMessage {
    /// Takes the next line of the file. Lines that are settled go to `feed`, in order; lines of
    /// the message that may still grow are kept. A line with no key stays with the open message.
    pub fn push(
        &mut self,
        bytes: &[u8],
        too_long: bool,
        start: u64,
        key: Option<&str>,
        feed: &mut impl FnMut(&[u8], bool),
    ) {
        if self.key.is_none() && key.is_none() {
            return feed(bytes, too_long); // before any keyed line: nothing to wait for
        }
        let same_message = match (&self.key, key) {
            (Some(open), Some(k)) => open == k,
            (Some(_), None) => true,
            (None, _) => false,
        };
        if !same_message {
            self.flush(feed);
            self.key = key.map(str::to_owned);
        }
        self.pending.push(Line {
            bytes: bytes.to_vec(),
            too_long,
            start,
        });
    }

    fn flush(&mut self, feed: &mut impl FnMut(&[u8], bool)) {
        for line in self.pending.drain(..) {
            feed(&line.bytes, line.too_long);
        }
    }

    /// The file ended. With `hold`, the open message stays unsent and the offset where it
    /// starts is returned, so the next read begins there. Otherwise everything is sent.
    pub fn finish(mut self, hold: bool, feed: &mut impl FnMut(&[u8], bool)) -> Option<u64> {
        match self.pending.first() {
            Some(first) if hold => Some(first.start),
            _ => {
                self.flush(feed);
                None
            }
        }
    }
}
