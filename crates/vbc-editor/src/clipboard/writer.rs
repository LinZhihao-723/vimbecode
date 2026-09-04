//! Putting a yank on the clipboard without the render loop waiting on the program that does it.
//!
//! A write costs a process. `clip.exe` is about thirty milliseconds end to end when nothing is
//! holding the clipboard and longer when something is, and thirty milliseconds on the drawing
//! thread is two frames a yank. So the write happens on a worker thread of its own, and what the
//! keystroke does is hand the text over and carry on.
//!
//! Nothing is waited for and nothing is retried. A yank the clipboard refused is reported once,
//! where a reader can see it, and the next yank is a fresh call rather than another go at the old
//! one: the failing calls are the slow ones, and a queue that retries them is a queue that grows
//! faster than it drains.
//!
//! The one thing that is waited for is the end of the session. A yank handed over a moment before
//! the editor exits is a yank the reader means to paste into another window, so dropping the
//! writer closes the queue and joins the worker rather than abandoning what is still in it.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use super::clip::{Clip, Error};

/// Somewhere a yank is put, by a call that blocks for as long as the system takes to take it.
pub trait Sink: Send + 'static {
    /// Puts `text` on the clipboard.
    ///
    /// # Parameters
    ///
    /// * `text` - What the clipboard is to hold.
    ///
    /// # Errors
    ///
    /// Returns an error if the text did not reach the clipboard.
    fn write_clipboard(&mut self, text: &str) -> Result<(), Error>;
}

impl Sink for Clip {
    fn write_clipboard(&mut self, text: &str) -> Result<(), Error> {
        self.put(text)
    }
}

/// The clipboard as a yank reaches it: a worker thread that runs the program, and a handover that
/// returns before it has.
#[derive(Debug)]
pub struct Writer {
    yanks: Option<Sender<String>>,
    refusals: Receiver<String>,
    worker: Option<JoinHandle<()>>,
    issued: u64,
}

impl Writer {
    /// Starts the worker thread, which builds the sink and then waits to be handed something.
    ///
    /// # Type Parameters
    ///
    /// * `SinkType` - Where the yank is put.
    ///
    /// # Returns
    ///
    /// The writer.
    pub fn start<SinkType: Sink>(sink: SinkType) -> Self {
        let (yanks, handed) = mpsc::channel();
        let (reported, refusals) = mpsc::channel();
        let worker = thread::spawn(move || serve(sink, &handed, &reported));

        Self {
            yanks: Some(yanks),
            refusals,
            worker: Some(worker),
            issued: 0,
        }
    }

    /// Hands `text` to the worker, which puts it on the clipboard.
    pub fn write(&mut self, text: String) {
        let sent = self
            .yanks
            .as_ref()
            .is_some_and(|yanks| yanks.send(text).is_ok());
        self.issued += u64::from(sent);
    }

    /// # Returns
    ///
    /// What a yank the clipboard refused said about it, oldest first, and [`None`] where every
    /// yank handed over so far was taken or is still on its way.
    pub fn refusal(&mut self) -> Option<String> {
        self.refusals.try_recv().ok()
    }

    /// # Returns
    ///
    /// The number of yanks this writer has handed to the sink.
    pub fn writes_issued(&self) -> u64 {
        self.issued
    }

    /// Closes the queue and waits for the worker, so that a yank handed over as the session ended
    /// still reaches the clipboard.
    pub fn shutdown(&mut self) {
        self.yanks = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Puts every yank handed over on the clipboard, one at a time, until there is nobody left to hand
/// one over.
///
/// # Type Parameters
///
/// * `SinkType` - Where the yank is put.
fn serve<SinkType: Sink>(mut sink: SinkType, handed: &Receiver<String>, reported: &Sender<String>) {
    while let Ok(text) = handed.recv() {
        if let Err(error) = sink.write_clipboard(&text) {
            if reported.send(error.to_string()).is_err() {
                return;
            }
        }
    }
}
