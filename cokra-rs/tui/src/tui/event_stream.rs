use std::time::Duration;

use crossterm::event::Event;
use crossterm::event::KeyEvent;
use tokio::sync::broadcast;
use tokio::sync::mpsc;

#[derive(Clone, Debug)]
pub enum TuiEvent {
  Key(KeyEvent),
  Paste(String),
  Resize(u16, u16),
  Draw,
  Tick,
}

pub(crate) struct TuiEventStream {
  input_rx: mpsc::UnboundedReceiver<TuiEvent>,
  draw_rx: broadcast::Receiver<()>,
  tick: tokio::time::Interval,
}

impl TuiEventStream {
  pub(crate) fn new(draw_rx: broadcast::Receiver<()>, tick_every: Duration) -> Self {
    let (tx, rx) = mpsc::unbounded_channel();

    std::thread::spawn(move || {
      loop {
        let Ok(event) = crossterm::event::read() else {
          break;
        };

        let mapped = match event {
          Event::Key(key) => Some(TuiEvent::Key(key)),
          Event::Paste(pasted) => Some(TuiEvent::Paste(pasted)),
          Event::Resize(w, h) => Some(TuiEvent::Resize(w, h)),
          _ => None,
        };

        if let Some(evt) = mapped
          && tx.send(evt).is_err()
        {
          break;
        }
      }
    });

    Self {
      input_rx: rx,
      draw_rx,
      tick: tokio::time::interval(tick_every),
    }
  }

  pub(crate) async fn next(&mut self) -> Option<TuiEvent> {
    tokio::select! {
      biased;
      maybe = self.input_rx.recv() => maybe,
      draw = self.draw_rx.recv() => match draw {
        Ok(()) => Some(TuiEvent::Draw),
        Err(_) => None,
      },
      _ = self.tick.tick() => Some(TuiEvent::Tick),
    }
  }
}
