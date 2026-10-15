//! Where a switch to the orthographic view looks for the vehicle: points across the screen, from its
//! middle outwards, so that the first brick found is the one nearest the middle.

/// How many points across and down the screen (odd, so that the middle is one of them).
const COLUMNS: usize = 9;
const ROWS: usize = 5;
/// How far towards the screen's edges the points go, as a share of the way.
const REACH: f32 = 0.9;

/// The points, as shares of the screen's half width and half height from its middle (-1 to 1, right
/// and up), on a screen `aspect` times as high as wide; the nearest to the middle first.
pub fn screen_points(aspect: f32) -> Vec<[f32; 2]> {
    let spread =
        |index: usize, count: usize| (2.0 * index as f32 / (count - 1) as f32 - 1.0) * REACH;
    let mut points: Vec<[f32; 2]> = (0..ROWS)
        .flat_map(|row| {
            (0..COLUMNS).map(move |column| [spread(column, COLUMNS), spread(row, ROWS)])
        })
        .collect();
    // On screen, a share of the half height is `aspect` times as long as one of the half width.
    let from_middle = |[x, y]: [f32; 2]| x * x + (y * aspect) * (y * aspect);
    points.sort_by(|a, b| from_middle(*a).total_cmp(&from_middle(*b)));
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_middle_comes_first() {
        let points = screen_points(9.0 / 16.0);
        assert_eq!(points.len(), COLUMNS * ROWS);
        assert_eq!(points[0], [0.0, 0.0]);
    }

    #[test]
    fn points_go_outwards_and_stay_on_screen() {
        let aspect = 9.0 / 16.0;
        let points = screen_points(aspect);
        let from_middle = |[x, y]: [f32; 2]| x * x + (y * aspect) * (y * aspect);
        for pair in points.windows(2) {
            assert!(from_middle(pair[0]) <= from_middle(pair[1]));
        }
        assert!(
            points
                .iter()
                .all(|[x, y]| x.abs() <= REACH + 1e-6 && y.abs() <= REACH + 1e-6)
        );
    }
}
