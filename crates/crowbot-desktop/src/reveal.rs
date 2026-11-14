use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub const FLOOR_PER_SECOND: f32 = 120.;
pub const CATCH_UP: Duration = Duration::from_millis(400);
pub const LONGEST_LAG: Duration = Duration::from_millis(900);
pub const FINISH_WITHIN: Duration = Duration::from_millis(400);
pub const HIDDEN_GAP: Duration = Duration::from_millis(250);

pub struct Reveal {
    shown: usize,
    received: usize,
    budget: f32,
    last_tick: Instant,
    arrivals: VecDeque<(Instant, usize)>,
    finished_at: Option<Instant>,
}

impl Reveal {
    pub fn shown_from(received: usize, now: Instant) -> Self {
        Self {
            shown: received,
            received,
            budget: 0.,
            last_tick: now,
            arrivals: VecDeque::new(),
            finished_at: None,
        }
    }

    #[cfg(test)]
    pub fn shown(&self) -> usize {
        self.shown
    }

    pub fn done(&self) -> bool {
        self.shown >= self.received
    }

    pub fn receive(&mut self, length: usize, now: Instant) {
        if length <= self.received {
            return;
        }
        if self.done() {
            self.last_tick = now;
            self.budget = 0.;
        }
        self.received = length;
        self.arrivals.push_back((now, length));
    }

    pub fn finish(&mut self, now: Instant) {
        self.finished_at.get_or_insert(now);
    }

    pub fn advance(&mut self, text: &str, now: Instant) -> usize {
        self.received = text.len();
        if self.done() {
            self.shown = self.received;
            self.last_tick = now;
            self.arrivals.clear();
            return self.shown;
        }
        let elapsed = now.saturating_duration_since(self.last_tick);
        self.last_tick = now;
        let overdue = self
            .finished_at
            .is_some_and(|finished| now.saturating_duration_since(finished) >= FINISH_WITHIN);
        if elapsed >= HIDDEN_GAP || overdue {
            return self.show_all();
        }

        let backlog = (self.received - self.shown) as f32;
        let mut rate = FLOOR_PER_SECOND.max(backlog / CATCH_UP.as_secs_f32());
        if let Some(finished) = self.finished_at {
            let left = FINISH_WITHIN.saturating_sub(now.saturating_duration_since(finished));
            rate = rate.max(backlog / left.as_secs_f32().max(0.001));
        }
        self.budget += rate * elapsed.as_secs_f32();
        let whole = self.budget.floor();
        self.budget -= whole;
        let target = (self.shown + whole as usize).min(self.received);

        let mut due = self.shown;
        while let Some((arrived, length)) = self.arrivals.front().copied() {
            if now.saturating_duration_since(arrived) < LONGEST_LAG {
                break;
            }
            due = due.max(length.min(self.received));
            self.arrivals.pop_front();
        }

        let streaming = self.finished_at.is_none();
        let stepped = word_boundary(text, target, self.received, streaming).max(self.shown);
        self.shown = floor_boundary(text, stepped.max(due));
        if self.done() {
            self.arrivals.clear();
        }
        self.shown
    }

    fn show_all(&mut self) -> usize {
        self.shown = self.received;
        self.budget = 0.;
        self.arrivals.clear();
        self.shown
    }
}

fn floor_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn word_boundary(text: &str, target: usize, received: usize, streaming: bool) -> usize {
    let target = floor_boundary(text, target);
    let received = floor_boundary(text, received);
    let word_end = text[target..received]
        .find(char::is_whitespace)
        .map(|offset| target + offset);
    match word_end {
        Some(end) => end,
        None if streaming => text[..target]
            .rfind(char::is_whitespace)
            .map_or(0, |start| start + 1),
        None => received,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: Duration = Duration::from_millis(16);

    fn words(count: usize) -> String {
        (0..count).map(|index| format!("word{index} ")).collect()
    }

    #[test]
    fn a_burst_appears_word_by_word_at_the_floor_speed_or_faster() {
        let text = words(40);
        let start = Instant::now();
        let mut reveal = Reveal::shown_from(0, start);
        reveal.receive(text.len(), start);
        let mut now = start;
        let mut shown = Vec::new();
        while !reveal.done() {
            now += FRAME;
            let length = reveal.advance(&text, now);
            assert!(
                length == text.len() || text[..length].ends_with(char::is_alphanumeric),
                "{:?} cuts a word",
                &text[..length]
            );
            shown.push(length);
        }
        assert!(
            shown.len() > 10,
            "the text appeared in {} steps",
            shown.len()
        );
        assert!(shown.windows(2).all(|pair| pair[0] <= pair[1]));
        let took = now - start;
        let floor = Duration::from_secs_f32(text.len() as f32 / FLOOR_PER_SECOND);
        assert!(
            took <= floor + FRAME,
            "{took:?} for {} characters",
            text.len()
        );
    }

    #[test]
    fn the_shown_text_is_never_more_than_a_second_behind() {
        let text = words(2_000);
        let start = Instant::now();
        let mut reveal = Reveal::shown_from(0, start);
        let mut now = start;
        let mut arrivals: Vec<(Instant, usize)> = Vec::new();
        let mut received = 0;
        for frame in 0..600 {
            now += FRAME;
            if frame % 20 == 0 && received < text.len() {
                received = floor_boundary(&text, (received + 3_000).min(text.len()));
                reveal.receive(received, now);
                arrivals.push((now, received));
            }
            let shown = reveal.advance(&text[..received], now);
            for (arrived, length) in &arrivals {
                if now.duration_since(*arrived) >= Duration::from_secs(1) {
                    assert!(
                        shown >= *length,
                        "{shown} shown, {length} received a second ago"
                    );
                }
            }
        }
        assert!(reveal.done());
    }

    #[test]
    fn a_slow_stream_shows_each_word_as_it_completes() {
        let text = "alpha beta gamma";
        let start = Instant::now();
        let mut reveal = Reveal::shown_from(0, start);
        reveal.receive(8, start);
        let mut now = start;
        for _ in 0..30 {
            now += FRAME;
            reveal.advance(&text[..8], now);
        }
        assert_eq!(text[..reveal.shown()].trim_end(), "alpha");
        now += Duration::from_millis(100);
        reveal.receive(text.len(), now);
        for _ in 0..30 {
            now += FRAME;
            reveal.advance(text, now);
        }
        assert_eq!(text[..reveal.shown()].trim_end(), "alpha beta");
    }

    #[test]
    fn the_end_of_the_turn_shows_everything_within_half_a_second() {
        let text = words(1_000);
        let start = Instant::now();
        let mut reveal = Reveal::shown_from(0, start);
        reveal.receive(text.len(), start);
        let mut now = start + FRAME;
        reveal.advance(&text, now);
        assert!(!reveal.done());
        reveal.finish(now);
        let finished = now;
        while !reveal.done() {
            now += FRAME;
            reveal.advance(&text, now);
        }
        assert!(now - finished <= Duration::from_millis(500));
        assert_eq!(reveal.shown(), text.len());

        let tail = "last";
        let mut reveal = Reveal::shown_from(0, start);
        reveal.receive(tail.len(), start);
        reveal.advance(tail, start + FRAME);
        assert_eq!(reveal.shown(), 0);
        reveal.finish(start + FRAME);
        let mut now = start + FRAME;
        while !reveal.done() {
            now += FRAME;
            reveal.advance(tail, now);
        }
        assert!(now - start <= Duration::from_millis(500));
    }

    #[test]
    fn a_window_that_was_hidden_shows_all_received_text_at_once() {
        let text = words(500);
        let start = Instant::now();
        let mut reveal = Reveal::shown_from(0, start);
        reveal.receive(text.len(), start);
        reveal.advance(&text, start + FRAME);
        assert!(reveal.shown() < text.len() / 2);
        let back = start + FRAME + HIDDEN_GAP;
        assert_eq!(reveal.advance(&text, back), text.len());
        assert!(reveal.done());
    }

    #[test]
    fn text_that_arrives_after_a_pause_is_still_revealed_in_steps() {
        let text = words(100);
        let half = floor_boundary(&text, text.len() / 2);
        let start = Instant::now();
        let mut reveal = Reveal::shown_from(0, start);
        reveal.receive(half, start);
        let mut now = start;
        while !reveal.done() {
            now += FRAME;
            reveal.advance(&text[..half], now);
        }
        now += Duration::from_secs(5);
        reveal.receive(text.len(), now);
        now += FRAME;
        let shown = reveal.advance(&text, now);
        assert!(shown < text.len(), "the second burst appeared at once");
        assert!(shown >= half);
    }

    #[test]
    fn a_snapshot_shows_its_text_at_once_and_accents_are_never_cut() {
        let now = Instant::now();
        let mut reveal = Reveal::shown_from(12, now);
        assert!(reveal.done());
        assert_eq!(reveal.advance("hello there!", now + FRAME), 12);

        let text = "été éééé ééé";
        let mut reveal = Reveal::shown_from(0, now);
        reveal.receive(text.len(), now);
        let mut tick = now;
        while !reveal.done() {
            tick += Duration::from_millis(3);
            let shown = reveal.advance(text, tick);
            assert!(text.is_char_boundary(shown));
        }
    }
}
