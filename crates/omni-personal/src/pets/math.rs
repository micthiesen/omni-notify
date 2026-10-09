//! Least-squares trend.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Regression {
    pub slope: f64,
    pub intercept: f64,
    pub r2: f64,
}

/// Ordinary least squares; fewer than two points or a vertical spread of
/// zero yield a flat line with `r2 = 0`; a perfectly flat series has `r2 = 1`.
#[allow(clippy::cast_precision_loss)]
pub fn linear_regression(points: &[Point]) -> Regression {
    let n = points.len() as f64;
    if points.len() < 2 {
        return Regression {
            slope: 0.0,
            intercept: points.first().map_or(0.0, |p| p.y),
            r2: 0.0,
        };
    }
    let (mut sum_x, mut sum_y, mut sum_xy, mut sum_xx) = (0.0, 0.0, 0.0, 0.0);
    for p in points {
        sum_x += p.x;
        sum_y += p.y;
        sum_xy += p.x * p.y;
        sum_xx += p.x * p.x;
    }
    let denom = n * sum_xx - sum_x * sum_x;
    if denom == 0.0 {
        return Regression {
            slope: 0.0,
            intercept: sum_y / n,
            r2: 0.0,
        };
    }
    let slope = (n * sum_xy - sum_x * sum_y) / denom;
    let intercept = (sum_y - slope * sum_x) / n;
    let mean_y = sum_y / n;
    let (mut ss_res, mut ss_tot) = (0.0, 0.0);
    for p in points {
        let predicted = intercept + slope * p.x;
        ss_res += (p.y - predicted).powi(2);
        ss_tot += (p.y - mean_y).powi(2);
    }
    let r2 = if ss_tot == 0.0 {
        1.0
    } else {
        1.0 - ss_res / ss_tot
    };
    Regression {
        slope,
        intercept,
        r2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_a_line() {
        let points: Vec<Point> = (0..5)
            .map(|i| Point {
                x: f64::from(i),
                y: 2.0 * f64::from(i) + 1.0,
            })
            .collect();
        let fit = linear_regression(&points);
        assert!((fit.slope - 2.0).abs() < 1e-12);
        assert!((fit.intercept - 1.0).abs() < 1e-12);
        assert!((fit.r2 - 1.0).abs() < 1e-12);
        assert_eq!(linear_regression(&[]).intercept, 0.0);
        assert_eq!(
            linear_regression(&[Point { x: 1.0, y: 3.0 }, Point { x: 1.0, y: 5.0 }]),
            Regression {
                slope: 0.0,
                intercept: 4.0,
                r2: 0.0
            }
        );
    }
}
