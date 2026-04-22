//! Terminal event handling with tick-based polling.
//!
//! Runs a background thread that reads crossterm events and emits
//! periodic tick events for data refresh.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crossterm::event::{self, KeyEvent};

/// Events produced by the event handler.
#[derive(Debug)]
pub enum Event {
    /// A keyboard event.
    Key(KeyEvent),
    /// A periodic tick (for polling data updates).
    Tick,
    /// Terminal resize.
    Resize(u16, u16),
}

/// Reads crossterm events in a background thread and forwards them
/// through an `mpsc` channel, interleaved with periodic tick events.
pub struct EventHandler {
    rx: mpsc::Receiver<Event>,
    // Keep the sender alive so the thread doesn't get an error on send
    _tx: mpsc::Sender<Event>,
}

impl EventHandler {
    /// Create a new event handler with the given tick rate in milliseconds.
    pub fn new(tick_rate_ms: u64) -> Self {
        let (tx, rx) = mpsc::channel();
        let event_tx = tx.clone();
        let tick_rate = Duration::from_millis(tick_rate_ms);

        thread::spawn(move || {
            loop {
                // Poll for crossterm events with a timeout equal to tick_rate
                if event::poll(tick_rate).unwrap_or(false) {
                    match event::read() {
                        Ok(event::Event::Key(key)) => {
                            if event_tx.send(Event::Key(key)).is_err() {
                                return; // Receiver dropped, exit thread
                            }
                        }
                        Ok(event::Event::Resize(w, h)) => {
                            if event_tx.send(Event::Resize(w, h)).is_err() {
                                return;
                            }
                        }
                        _ => {}
                    }
                } else {
                    // No event within tick_rate — emit a Tick
                    if event_tx.send(Event::Tick).is_err() {
                        return;
                    }
                }
            }
        });

        Self { rx, _tx: tx }
    }

    /// Block until the next event is available.
    pub fn next(&self) -> Result<Event, mpsc::RecvError> {
        self.rx.recv()
    }
}
