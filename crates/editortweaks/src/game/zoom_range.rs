//! A view target's zoom range (`FViewTargetZoomParams`): how far its field of view, or the factor
//! it's multiplied by, goes. Plain numbers, see `game::view_target` for where they come from.

/// `ZoomRange` and `Exponent` of an `FViewTargetZoomParams`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoomRange {
    /// `ZoomRange.Min` (zoomed in the most), `ZoomRange.Max` (unzoomed).
    pub min: f32,
    pub max: f32,
    pub exponent: f32,
}

impl ZoomRange {
    /// `IViewTargetInterface::GetZoomFOVFactor`: `Lerp(Max, Min, ratio^Exponent)`.
    pub fn value_at(&self, ratio: f32) -> f32 {
        let along = ratio.clamp(0.0, 1.0).powf(self.exponent);
        along * (self.min - self.max) + self.max
    }

    /// The ratio giving `value`, the nearest if out of range.
    pub fn ratio_for(&self, value: f32) -> f32 {
        let span = self.max - self.min;
        if span.abs() < f32::EPSILON || self.exponent <= 0.0 {
            return 0.0;
        }
        ((self.max - value) / span)
            .clamp(0.0, 1.0)
            .powf(1.0 / self.exponent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The standard camera brick's (`BP_Camera_2x1x1`, the exponent is the C++ default).
    const CAMERA: ZoomRange = ZoomRange {
        min: 15.0,
        max: 70.0,
        exponent: 0.75,
    };
    /// A factor range like the editor's.
    const EDITOR: ZoomRange = ZoomRange {
        min: 0.1,
        max: 1.0,
        exponent: 0.75,
    };

    #[test]
    fn a_camera_brick_starts_at_its_widest_view() {
        assert_eq!(CAMERA.value_at(0.0), 70.0);
        assert!((CAMERA.value_at(1.0) - 15.0).abs() < 1e-5);
    }

    #[test]
    fn the_ratio_found_gives_the_value_back() {
        for value in [0.1, 0.2, 0.5, 0.7777, 0.95, 1.0] {
            let ratio = EDITOR.ratio_for(value);
            assert!((EDITOR.value_at(ratio) - value).abs() < 1e-4, "{value}");
        }
    }

    #[test]
    fn a_view_wider_than_unzoomed_stays_unzoomed() {
        assert_eq!(EDITOR.ratio_for(1.3), 0.0);
    }

    #[test]
    fn a_view_narrower_than_the_range_stops_at_its_end() {
        assert_eq!(EDITOR.ratio_for(0.01), 1.0);
    }

    #[test]
    fn a_range_without_span_never_divides_by_zero() {
        let flat = ZoomRange {
            min: 1.0,
            max: 1.0,
            exponent: 1.0,
        };
        assert_eq!(flat.ratio_for(0.5), 0.0);
    }
}
