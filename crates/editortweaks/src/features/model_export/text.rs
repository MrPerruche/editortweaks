//! Text bricks' text for the export: the glyph quads their text render component draws
//! (`model_io::export::text`), placed where the component is, textured with the font's pages
//! (their distance field cut out at its middle) and drawn from both sides like the game's text
//! material. Not the outline the brick's outline thickness adds.

use std::collections::HashMap;

use brickrust::ue::coreuobject::UObjectBase;

use model_io::Part;
use model_io::export::Export;
use model_io::export::text::{self, Font};
use model_io::model::unreal_facing;

use super::looks::Materials;
use crate::game::editor_object::{Transform, Vector};
use crate::game::text_render::TextRenders;
use crate::math::{self, Quat};

/// A font, and its pages' images (`None` for those that couldn't be read).
type FontPages = (Font, Vec<Option<usize>>);

/// Reads text bricks' text, each font once per export.
pub struct TextParts<'a> {
    renders: &'a TextRenders,
    /// Each font read so far (`None` if it couldn't be).
    fonts: HashMap<usize, Option<FontPages>>,
    /// Text bricks whose component wasn't the size fitting the brick (stale), so refitted here.
    pub refitted: usize,
    /// Text bricks our layout measured unlike the engine (logged, up to [`LOGGED`]).
    mismeasured: usize,
    /// Text bricks whose component wasn't where `UpdateText` puts it, so placed there here (logged,
    /// up to [`LOGGED`]).
    pub misplaced: usize,
}

/// How many text bricks of each kind of disagreement are logged per export.
const LOGGED: usize = 3;

/// How far above the brick's top `UpdateText` puts the text (cm).
const ABOVE_TOP: f64 = 1.0;

/// The text component's rotation in the brick (`UpdateText`: pitch 90, roll 90, degrees): it faces
/// the brick's +Z, its +Y along the brick's +X, its +Z along the brick's +Y.
const ON_TOP: [f64; 3] = [90.0, 0.0, 90.0];

/// How far a component may be from where `UpdateText` puts it before it's taken as not the one
/// drawn there (cm).
const MISPLACED: f32 = 1.0;

impl<'a> TextParts<'a> {
    pub fn new(renders: &'a TextRenders) -> Self {
        Self {
            renders,
            fonts: HashMap::new(),
            refitted: 0,
            mismeasured: 0,
            misplaced: 0,
        }
    }

    /// `brick`'s text as parts, one per font page used, placed in the world; none if it isn't a
    /// text brick showing text, or its font can't be read. Its size is fitted to `bounds` (the
    /// brick's local box) as `UTextBrick::UpdateText` does, rather than taken from the component,
    /// with the engine's measure of the text (`GetTextLocalSize`) where it's found. It's read from
    /// the component nearest to where `UpdateText` puts it (the middle of the brick's top, 1 cm
    /// above it, in the brick at `place`), and placed there if even that one is elsewhere.
    ///
    /// # Safety
    /// `brick` must be a live brick. Game thread only.
    pub unsafe fn parts(
        &mut self,
        materials: &mut Materials,
        export: &mut Export,
        brick: *mut UObjectBase,
        bounds: [Vector; 2],
        place: &Transform,
    ) -> Vec<Part> {
        let [low, high] = bounds.map(math::widen);
        let brick_rotation = Quat::widen(place.rotation);
        let [bx, by, bz, _] = place.translation;
        let on_top = [
            (low[0] + high[0]) / 2.0,
            (low[1] + high[1]) / 2.0,
            high[2] + ABOVE_TOP,
        ];
        let spot = math::narrow(math::add(
            math::widen([bx, by, bz]),
            brick_rotation.rotate(on_top),
        ));
        // SAFETY: per the contract; the font is the component's live asset.
        let Some(shown) = (unsafe { self.renders.of(brick, spot) }) else {
            return Vec::new();
        };
        let [x, y, z, _] = shown.place.translation;
        let away = math::length(math::sub(math::widen([x, y, z]), math::widen(spot))) as f32;
        let misplaced = away > MISPLACED;
        if misplaced {
            if self.misplaced < LOGGED {
                crate::prelude::log!(
                    "text {:?}: the nearest of the brick's {} text components is {away} cm from \n                     its top's middle; placed there",
                    shown.text,
                    shown.components
                );
            }
            self.misplaced += 1;
        }
        if shown.font.is_null() {
            return Vec::new();
        }
        let renders = self.renders;
        let font = self.fonts.entry(shown.font as usize).or_insert_with(|| {
            // SAFETY: as above.
            let (mut font, pages) = unsafe { renders.font(shown.font) }
                .map_err(|reason| crate::prelude::log!("text not exported: {reason}"))
                .ok()?;
            let mut images = Vec::with_capacity(pages.len());
            for (size, page) in font.pages.iter_mut().zip(pages) {
                // SAFETY: the font's live pages.
                let image = (!page.is_null())
                    .then(|| unsafe { materials.glyphs(export, page) })
                    .flatten();
                *size = image.map(|(_, size)| size);
                images.push(image.map(|(index, _)| index));
            }
            Some((font, images))
        });
        let Some((font, images)) = font else {
            return Vec::new();
        };
        // The text runs along the brick's X, its lines go down its Y.
        let room = [high[0] - low[0], high[1] - low[1]].map(|side| side as f32);
        // What the game fits: the engine's measure (at the component's size, grown to the font
        // size: text scales with it, but for empty lines), else our own layout's (also if the
        // component may not be this brick's).
        let measured = shown
            .measured
            .filter(|_| shown.world_size > 0.0 && !misplaced)
            .map(|[_, width, height]| {
                [width, height].map(|side| side * shown.font_size / shown.world_size)
            });
        let ratio = match measured {
            Some(size) => text::shrink(size, room),
            None => text::fit(&shown.text, font, &shown.settings, room),
        };
        if let Some([_, width, height]) = shown.measured
            && !misplaced
            && self.mismeasured < LOGGED
            && shown.font_size > 0.0
        {
            let mut at_size = shown.settings;
            let grow = shown.world_size / shown.font_size;
            at_size.scale = at_size.scale.map(|scale| scale * grow);
            let ours = text::size(&shown.text, font, &at_size);
            if (ours[0] - width).abs() > width.abs() * 0.01
                || (ours[1] - height).abs() > height.abs() * 0.01
            {
                self.mismeasured += 1;
                crate::prelude::log!(
                    "text {:?} at size {}: {ours:?} cm by our layout, [{width}, {height}] by the \
                     engine",
                    shown.text,
                    shown.world_size
                );
            }
        }
        let mut settings = shown.settings;
        settings.scale = settings.scale.map(|scale| scale * ratio);
        if (shown.font_size * ratio - shown.world_size).abs() > shown.world_size.abs() * 0.01 {
            self.refitted += 1;
        }
        let quads = text::layout(&shown.text, font, &settings);
        let (rotation, [x, y, z], [sx, sy, sz]) = if misplaced {
            (brick_rotation * Quat::from_rotator(ON_TOP), spot, [1.0; 3])
        } else {
            let [sx, sy, sz, _] = shown.place.scale;
            (Quat::widen(shown.place.rotation), [x, y, z], [sx, sy, sz])
        };
        let place = |corner: [f32; 3]| {
            let scaled = math::widen([corner[0] * sx, corner[1] * sy, corner[2] * sz]);
            math::narrow(math::add(math::widen([x, y, z]), rotation.rotate(scaled)))
        };
        // The component's front (its +X) in the world, which the quads face.
        let front = rotation.rotate([1.0, 0.0, 0.0]);
        let mut parts: HashMap<usize, Part> = HashMap::new();
        for quad in quads {
            let Some(image) = images.get(quad.page).copied().flatten() else {
                continue;
            };
            let part = parts.entry(image).or_insert_with(|| Part {
                material: Some(export.look_index(Materials::text(shown.color, image))),
                ..Part::default()
            });
            let first = part.positions.len() as u32;
            let corners = quad.corners.map(place);
            part.positions.extend(corners);
            part.uvs.extend(quad.uvs);
            // Top left, top right, bottom left, bottom right: two triangles, wound to face front.
            let mut triangles = [[0, 1, 3], [0, 3, 2]];
            let facing = unreal_facing(corners[0], corners[1], corners[3]);
            if math::dot(math::widen(facing), front) < 0.0 {
                for triangle in &mut triangles {
                    triangle.swap(1, 2);
                }
            }
            part.corners
                .extend(triangles.map(|triangle| triangle.map(|corner| first + corner)));
        }
        parts.into_values().collect()
    }
}
