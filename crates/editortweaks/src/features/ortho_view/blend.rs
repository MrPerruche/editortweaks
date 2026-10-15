//! The math of the orthographic view: how wide a perspective view is at some depth, the switch
//! between the two views as a dolly zoom, and the zoom gliding to where its steps take it.

use core::f32::consts::PI;

/// The narrowest field of view of a switch, in degrees: from it, the view becomes orthographic.
/// Narrower looks more like the orthographic view but backs the camera further away (about 29
/// times the view's width at 2°).
pub const NARROWEST_FOV: f32 = 2.0;

/// How wide (cm) a perspective view with a horizontal field of view of `fov` degrees is at `depth`
/// cm in front of it.
pub fn width_at(fov: f32, depth: f32) -> f32 {
    2.0 * depth * half_tan(fov)
}

/// At which depth (cm) a perspective view with a field of view of `fov` degrees is `width` cm wide.
pub fn depth_for(fov: f32, width: f32) -> f32 {
    width / (2.0 * half_tan(fov))
}

/// A perspective view on the way between a field of view of `fov` degrees (`progress` 0) and
/// [`NARROWEST_FOV`] (`progress` 1), keeping what lies at `depth` in front of the camera `width`
/// wide: its field of view, and how far behind the camera it's seen from.
pub fn dolly(fov: f32, width: f32, depth: f32, progress: f32) -> (f32, f32) {
    let progress = progress.clamp(0.0, 1.0);
    // The tangents go from one to the other by the same factor each step of progress.
    let tan = half_tan(fov).powf(1.0 - progress) * half_tan(NARROWEST_FOV).powf(progress);
    let fov = 2.0 * tan.atan() * 180.0 / PI;
    (fov, width / (2.0 * tan) - depth)
}

/// How far a switch lasting `duration` seconds is after `elapsed` seconds, 0 to 1, easing in and
/// out.
pub fn progress(elapsed: f32, duration: f32) -> f32 {
    let linear = if duration > 0.0 {
        (elapsed / duration).clamp(0.0, 1.0)
    } else {
        1.0
    };
    linear * linear * (3.0 - 2.0 * linear)
}

/// `current` moved towards `target` over `delta` seconds like the game's zoom (`FInterpTo` at
/// `speed`, at once for 0), by factors: each zoom step takes as long.
pub fn glide(current: f32, target: f32, speed: f32, delta: f32) -> f32 {
    if speed <= 0.0 || current <= 0.0 || target <= 0.0 {
        return target;
    }
    towards(current, target, (delta * speed).clamp(0.0, 1.0))
}

/// The value `share` of the way from `from` to `to` (both above 0) by factors: halfway from 100 to
/// 400 is 200.
pub fn towards(from: f32, to: f32, share: f32) -> f32 {
    from * (to / from).powf(share)
}

fn half_tan(fov: f32) -> f32 {
    (fov.to_radians() / 2.0).tan()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 1e-3 * a.abs().max(b.abs()).max(1.0)
    }

    #[test]
    fn a_90_degree_view_is_twice_as_wide_as_deep() {
        assert!(close(width_at(90.0, 500.0), 1000.0));
        assert!(close(depth_for(90.0, 1000.0), 500.0));
    }

    #[test]
    fn a_dolly_starts_from_the_camera() {
        let width = width_at(90.0, 500.0);
        let (fov, back) = dolly(90.0, width, 500.0, 0.0);
        assert!(close(fov, 90.0));
        assert!(back.abs() < 1e-2);
    }

    #[test]
    fn a_dolly_keeps_the_width_at_its_depth() {
        let width = width_at(70.0, 800.0);
        for step in 0..=10 {
            let (fov, back) = dolly(70.0, width, 800.0, step as f32 / 10.0);
            assert!(close(width_at(fov, 800.0 + back), width));
        }
    }

    #[test]
    fn a_dolly_ends_far_back_and_narrow() {
        let width = 1000.0;
        let (fov, back) = dolly(90.0, width, 500.0, 1.0);
        assert!(close(fov, NARROWEST_FOV));
        assert!(back > 25.0 * width);
    }

    #[test]
    fn progress_eases_in_and_out() {
        assert_eq!(progress(0.0, 0.3), 0.0);
        assert_eq!(progress(0.3, 0.3), 1.0);
        assert_eq!(progress(1.0, 0.3), 1.0);
        assert!(close(progress(0.15, 0.3), 0.5));
        assert!(progress(0.03, 0.3) < 0.1);
        assert_eq!(progress(0.0, 0.0), 1.0);
    }

    #[test]
    fn towards_goes_by_factors() {
        assert!(close(towards(100.0, 400.0, 0.0), 100.0));
        assert!(close(towards(100.0, 400.0, 0.5), 200.0));
        assert!(close(towards(100.0, 400.0, 1.0), 400.0));
    }

    #[test]
    fn gliding_reaches_its_target() {
        let mut width = 100.0;
        for _ in 0..200 {
            width = glide(width, 1000.0, 10.0, 1.0 / 60.0);
        }
        assert!(close(width, 1000.0));
        assert_eq!(glide(100.0, 1000.0, 0.0, 0.016), 1000.0);
        // A step in and a step out take as long.
        let into = glide(100.0, 200.0, 10.0, 0.05) / 100.0;
        let out_of = 200.0 / glide(200.0, 100.0, 10.0, 0.05);
        assert!(close(into, out_of));
    }
}
