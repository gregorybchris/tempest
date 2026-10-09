//! Random draws for the simulated world, each made from one uniform sample in `[0, 1)` so that
//! the world's generator draws the same numbers whatever the distributions used (D-073). The
//! math is `libm`'s rather than the platform's, whose results differ in the last bits between
//! systems, so a seed gives the same world everywhere.

use std::time::Duration;

/// A sample from the exponential distribution with mean `mean`.
pub fn exponential(mean: Duration, u: f64) -> Duration {
    from_secs(mean.as_secs_f64() * -libm::log(1.0 - u))
}

/// A sample from the Weibull distribution with mean `mean` and shape `shape`. Shape 1 is the
/// exponential distribution; smaller shapes have heavier tails.
pub fn weibull(mean: Duration, shape: f64, u: f64) -> Duration {
    if shape == 1.0 {
        return exponential(mean, u);
    }
    let scale = mean.as_secs_f64() / gamma(1.0 + 1.0 / shape);
    from_secs(scale * libm::pow(-libm::log(1.0 - u), 1.0 / shape))
}

fn from_secs(secs: f64) -> Duration {
    Duration::try_from_secs_f64(secs).unwrap_or(Duration::MAX)
}

/// The gamma function for `x > 0`, by the Lanczos approximation (accurate to about 15
/// significant digits).
pub fn gamma(x: f64) -> f64 {
    const G: f64 = 7.0;
    const C: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];
    if x < 0.5 {
        // The reflection formula.
        return std::f64::consts::PI / (libm::sin(std::f64::consts::PI * x) * gamma(1.0 - x));
    }
    let x = x - 1.0;
    let mut sum = C[0];
    for (i, &c) in C.iter().enumerate().skip(1) {
        sum += c / (x + i as f64);
    }
    let t = x + G + 0.5;
    (2.0 * std::f64::consts::PI).sqrt() * libm::pow(t, x + 0.5) * libm::exp(-t) * sum
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rand::SeedableRng;

    use super::*;

    #[test]
    fn gamma_matches_known_values() {
        let close = |a: f64, b: f64| (a - b).abs() < 1e-12 * b.abs().max(1.0);
        assert!(close(gamma(1.0), 1.0));
        assert!(close(gamma(5.0), 24.0));
        assert!(close(gamma(0.5), std::f64::consts::PI.sqrt()));
        assert!(close(gamma(1.5), std::f64::consts::PI.sqrt() / 2.0));
        assert!(close(gamma(11.0), 3_628_800.0));
    }

    /// The sample mean of many draws.
    fn mean_of(draw: impl Fn(f64) -> Duration) -> f64 {
        let mut rng = tempest_core::Rng::seed_from_u64(9);
        let n = 200_000;
        (0..n)
            .map(|_| draw(rng.random()).as_secs_f64())
            .sum::<f64>()
            / f64::from(n)
    }

    #[test]
    fn weibull_draws_have_the_requested_mean() {
        let mean = Duration::from_secs(600);
        for shape in [0.5, 1.0, 2.0] {
            let sample = mean_of(|u| weibull(mean, shape, u));
            assert!(
                (sample / 600.0 - 1.0).abs() < 0.02,
                "shape {shape}: {sample}"
            );
        }
    }

    /// Exact draws, the same on every platform, so a seed gives the same world everywhere.
    #[test]
    fn draws_are_exact() {
        let mean = Duration::from_secs(600);
        let draws = [
            exponential(mean, 0.3),
            weibull(mean, 0.5, 0.3),
            weibull(mean, 2.5, 0.77),
        ];
        let nanos = draws.map(|d| d.as_nanos());
        assert_eq!(nanos, [214_004_966_363, 38_165_104_690, 788_836_715_775]);
        assert_eq!(gamma(0.3).to_bits(), 4_613_918_833_289_118_890);
    }

    #[test]
    fn shape_one_is_exactly_exponential() {
        let mean = Duration::from_secs(90);
        for u in [0.0, 0.1, 0.5, 0.999] {
            assert_eq!(weibull(mean, 1.0, u), exponential(mean, u));
        }
    }

    proptest! {
        /// A larger sample gives a longer time, whatever the shape.
        #[test]
        fn weibull_is_monotonic(shape in 0.05f64..5.0, u in 0.0f64..0.999, v in 0.0f64..0.999) {
            let mean = Duration::from_secs(60);
            let (lo, hi) = if u <= v { (u, v) } else { (v, u) };
            prop_assert!(weibull(mean, shape, lo) <= weibull(mean, shape, hi));
        }
    }
}
