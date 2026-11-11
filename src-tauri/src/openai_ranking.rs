use crate::models::OpenAiPolicy;

pub const BENCHMARK_VERSION: u32 = 2;
pub const MIN_STREAM_SAMPLES: usize = 3;

pub fn is_ranked(policy: &OpenAiPolicy) -> bool {
    policy.benchmark_version >= BENCHMARK_VERSION
}

#[derive(Clone, Copy, Default)]
pub struct RankSignals {
    pub stream_successes: usize,
    pub stream_samples: usize,
    pub connection_successes: usize,
    pub connection_samples: usize,
    pub latency_ms: u32,
    pub jitter_ms: u32,
    pub traffic_multiplier: Option<f64>,
}

pub fn score(signals: RankSignals) -> f64 {
    let (quality, possible) = quality_points(&signals);
    // An unknown price earns no cost points, without inventing a multiplier.
    let cost = signals
        .traffic_multiplier
        .filter(|rate| rate.is_finite() && *rate > 0.0)
        .map_or(0.0, |rate| 20.0 / (1.0 + rate).sqrt());
    (quality + cost) / (possible + 20.0) * 100.0
}

pub fn quality_score(signals: &RankSignals) -> Option<f64> {
    if signals.connection_samples == 0 && signals.stream_samples < MIN_STREAM_SAMPLES {
        return None;
    }
    let (points, possible) = quality_points(signals);
    Some(points / possible * 100.0)
}

fn quality_points(signals: &RankSignals) -> (f64, f64) {
    let mut points = 0.0;
    let mut possible = 25.0;
    if signals.stream_samples >= MIN_STREAM_SAMPLES {
        possible += 45.0;
        points += 45.0 * ratio(signals.stream_successes, signals.stream_samples);
    }
    if signals.connection_samples > 0 {
        points += 25.0 * ratio(signals.connection_successes, signals.connection_samples);
    }
    if signals.latency_ms > 0 {
        possible += 10.0;
        let latency = (1.0 - f64::from(signals.latency_ms) / 3_000.0).clamp(0.0, 1.0);
        let jitter = (1.0 - f64::from(signals.jitter_ms) / 1_500.0).clamp(0.0, 1.0);
        points += 7.0 * latency + 3.0 * jitter;
    }
    (points, possible)
}

fn ratio(passed: usize, total: usize) -> f64 {
    passed.min(total) as f64 / total as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signals(rate: Option<f64>, latency_ms: u32) -> RankSignals {
        RankSignals {
            connection_successes: 4,
            connection_samples: 4,
            latency_ms,
            jitter_ms: 10,
            traffic_multiplier: rate,
            ..Default::default()
        }
    }

    #[test]
    fn comparable_connections_favor_known_low_multipliers_over_speed() {
        let economical = score(signals(Some(1.0), 180));
        assert!(economical > score(signals(Some(3.0), 90)));
        assert!(score(signals(Some(0.5), 180)) > economical);
        assert!(score(signals(Some(3.0), 90)) > score(signals(None, 90)));
    }

    #[test]
    fn unverified_streams_do_not_count_and_confirmed_interruptions_outweigh_cost() {
        let pending = score(signals(Some(1.0), 180));
        assert_eq!(
            pending,
            score(RankSignals {
                stream_successes: 1,
                stream_samples: 2,
                ..signals(Some(1.0), 180)
            })
        );
        assert!(
            score(RankSignals {
                stream_successes: 2,
                stream_samples: 4,
                ..signals(Some(1.0), 180)
            }) < score(RankSignals {
                stream_successes: 4,
                stream_samples: 4,
                ..signals(Some(3.0), 90)
            })
        );
    }
}
