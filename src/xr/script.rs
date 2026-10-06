//! A timed list of actions for testing `play` over SSH with nobody wearing the
//! headset (`JUST_VIDEO_SCRIPT`): pause, jumps, the A-B loop and taking the
//! headset off and putting it on again.
//!
//! Format: space- or comma-separated `SECONDS:ACTION`, seconds counted from
//! the start of `play`. Actions: `pause` (toggle), `jump=N` / `jump+N` /
//! `jump-N` (absolute / relative, seconds), `loop` (the A-B button), `off`,
//! `on`. Example: `60:pause 65:pause 90:jump+600 120:loop 125:loop 200:off 215:on`.

use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Pause,
    JumpTo(f64),
    JumpBy(f64),
    Loop,
    Off,
    On,
}

#[derive(Debug, Default)]
pub struct Script {
    /// Sorted by time, soonest last so `due` pops from the end.
    pending: Vec<(Duration, Action)>,
}

impl Script {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut pending = Vec::new();
        for item in text.split([' ', ',']).filter(|s| !s.is_empty()) {
            let (at, action) = item
                .split_once(':')
                .ok_or_else(|| format!("{item}: expected SECONDS:ACTION"))?;
            let at: f64 = at.parse().map_err(|_| format!("{item}: bad time"))?;
            let number = |rest: &str| {
                rest.parse::<f64>()
                    .map_err(|_| format!("{item}: bad number"))
            };
            let action = match action {
                "pause" => Action::Pause,
                "loop" => Action::Loop,
                "off" => Action::Off,
                "on" => Action::On,
                a if a.starts_with("jump=") => Action::JumpTo(number(&a[5..])?),
                a if a.starts_with("jump+") => Action::JumpBy(number(&a[5..])?),
                a if a.starts_with("jump-") => Action::JumpBy(-number(&a[5..])?),
                _ => return Err(format!("{item}: unknown action")),
            };
            pending.push((Duration::from_secs_f64(at.max(0.0)), action));
        }
        // Stable: actions at the same time keep their written order.
        pending.reverse();
        pending.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        Ok(Self { pending })
    }

    /// From the environment; a bad script is reported and ignored.
    pub fn from_env() -> Self {
        match std::env::var("JUST_VIDEO_SCRIPT") {
            Ok(text) => Self::parse(&text).unwrap_or_else(|e| {
                eprintln!("JUST_VIDEO_SCRIPT: {e}");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// The actions whose time has come, in order.
    pub fn due(&mut self, elapsed: Duration) -> Vec<Action> {
        let mut out = Vec::new();
        while self.pending.last().is_some_and(|(at, _)| *at <= elapsed) {
            out.push(self.pending.pop().expect("checked").1);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_releases_in_time_order() {
        let mut s =
            Script::parse("30:jump-10 5:pause,10:off 20:on 25:jump=100 26:jump+7 27:loop 27:loop")
                .unwrap();
        assert!(s.due(Duration::from_secs(4)).is_empty());
        assert_eq!(s.due(Duration::from_secs(12)), [Action::Pause, Action::Off]);
        assert_eq!(
            s.due(Duration::from_secs(100)),
            [
                Action::On,
                Action::JumpTo(100.0),
                Action::JumpBy(7.0),
                Action::Loop,
                Action::Loop,
                Action::JumpBy(-10.0)
            ]
        );
        assert!(s.due(Duration::from_secs(200)).is_empty());
    }

    #[test]
    fn rejects_nonsense() {
        assert!(Script::parse("5:dance").is_err());
        assert!(Script::parse("pause").is_err());
        assert!(Script::parse("x:pause").is_err());
        assert!(Script::parse("").unwrap().due(Duration::MAX).is_empty());
    }
}
