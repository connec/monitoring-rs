// loadgen/src/lib.rs
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use smol::future::Future;
use smol::stream::{Stream, StreamExt};
use smol::Timer;

pub struct Generator {
    streams: Vec<Pin<Box<dyn Stream<Item = ()>>>>,
}

impl Generator {
    pub fn new<E: Fn() + 'static>(
        duration: Duration,
        stream_count: u32,
        event_count: u32,
        distribution: Distribution,
        event: E,
    ) -> Self {
        let mut streams = Vec::new();
        let event: Arc<dyn Fn() + 'static> = Arc::new(event);

        for event_count in distribution.distribute(event_count, stream_count) {
            let events_per_second = f64::from(event_count) / duration.as_secs_f64();
            let interval = Duration::from_secs_f64(1.0 / events_per_second);
            let event = Arc::clone(&event);

            let emitter = Timer::interval(interval)
                .map(move |_| event())
                .take(event_count as usize);

            let stream: Pin<Box<dyn Stream<Item = _>>> = Box::pin(emitter);
            streams.push(stream);
        }

        Generator { streams }
    }

    pub async fn run(self) {
        GeneratorRun {
            streams: self.streams,
        }
        .await
    }
}

struct GeneratorRun {
    streams: Vec<Pin<Box<dyn Stream<Item = ()>>>>,
}

impl Future for GeneratorRun {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.streams
            .iter_mut()
            .fold(Poll::Ready(()), |poll, stream| {
                if let (Poll::Pending, _) | (_, Poll::Pending) | (_, Poll::Ready(Some(_))) =
                    (poll, stream.poll_next(cx))
                {
                    Poll::Pending
                } else {
                    poll
                }
            })
    }
}

#[derive(Clone, Copy)]
pub enum Distribution {
    Uniform,
    Linear,
}

impl Distribution {
    fn distribute(self, event_count: u32, stream_count: u32) -> Vec<u32> {
        let event_count_f64 = f64::from(event_count);
        let stream_count_f64 = f64::from(stream_count);

        match self {
            Self::Uniform => {
                // Same number of events to each stream. Casts is OK because u32 / u32 is u32,
                // we just want the division rounded rather than floored.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let per_stream = (event_count_f64 / stream_count_f64).round() as u32;

                vec![per_stream; stream_count as usize]
            }
            Self::Linear => {
                // Calculate the height of a triangle with `base = stream_count + 1` and `area =
                // event_count`.
                let max = 2.0 * event_count_f64 / (1.0 + stream_count_f64);

                // We want progress evenly up to `max`.
                let inc = max / stream_count_f64;

                (1..=stream_count)
                    .map(|i| {
                        let stream_events = (f64::from(i) * inc).round();

                        // Cast is OK because `stream_events` cannot be negative, greater than
                        // `u32::MAX`, and has been rounded.
                        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                        let stream_events = stream_events as u32;

                        stream_events
                    })
                    .filter(|i| *i != 0)
                    .collect()
            }
        }
    }
}
