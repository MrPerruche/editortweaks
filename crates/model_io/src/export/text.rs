//! Text laid out like Unreal's text render components draw it (UE 4.27
//! `FTextRenderSceneProxy::BuildStringMesh` with `UTextRenderComponent::GetRenderMatrix`'s
//! vertical alignment): one quad per glyph of an offline font, cut from the font's pages.
//!
//! The quads are in the component's own space (centimetres): they face its +X, the text runs
//! toward its -Y, line after line toward its -Z.

use std::collections::HashMap;

/// A character of an offline font (`FFontCharacter`), in pixels of its page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Glyph {
    /// Its corner on the page, and its size.
    pub start: [i32; 2],
    pub size: [i32; 2],
    /// Which page (texture) it's on.
    pub page: usize,
    pub vertical_offset: i32,
}

/// An offline font (`UFont`).
#[derive(Clone, Debug, Default)]
pub struct Font {
    pub glyphs: Vec<Glyph>,
    /// Character code -> glyph, for remapped fonts (`CharRemap`); `None`: the code is the glyph.
    pub remap: Option<HashMap<u16, u16>>,
    /// Each page's size in pixels, `None` for a missing page (its glyphs aren't drawn).
    pub pages: Vec<Option<[u32; 2]>>,
    pub kerning: i32,
}

/// The glyph `UFont::RemapChar` gives for characters a remapped font lacks.
const NULL_CHARACTER: u16 = 127;

impl Font {
    /// The glyph drawing `character`, if the font has one on a page it has.
    fn glyph(&self, character: char) -> Option<(&Glyph, [u32; 2])> {
        // Unreal's characters are UTF-16 code units: past them, the null character.
        let code = u16::try_from(u32::from(character)).unwrap_or(NULL_CHARACTER);
        let index = match &self.remap {
            Some(remap) => *remap.get(&code).unwrap_or(&NULL_CHARACTER),
            None => code,
        };
        let glyph = self.glyphs.get(usize::from(index))?;
        let page = (*self.pages.get(glyph.page)?)?;
        Some((glyph, page))
    }
}

/// Where text goes along its lines (`EHorizTextAligment`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Horizontal {
    #[default]
    Left,
    Center,
    Right,
}

/// Where text goes across its lines (`EVerticalTextAligment`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Vertical {
    #[default]
    TextTop,
    TextCenter,
    TextBottom,
    QuadTop,
}

/// A text render component's settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Centimetres per font pixel along and across the lines: `WorldSize * InvDefaultSize` times
    /// `XScale` and `YScale`.
    pub scale: [f32; 2],
    /// `HorizSpacingAdjust`, `VertSpacingAdjust` (font pixels).
    pub spacing: [f32; 2],
    pub horizontal: Horizontal,
    pub vertical: Vertical,
    /// How far an empty line moves the next one down: the font's tallest character, unscaled (as
    /// Unreal does).
    pub empty_line: f32,
}

/// A glyph's quad: its corners (top left, top right, bottom left, bottom right) and their UVs on
/// its page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quad {
    pub page: usize,
    pub corners: [[f32; 3]; 4],
    pub uvs: [[f32; 2]; 4],
}

/// A line of text, measured.
struct Line {
    characters: Vec<char>,
    /// Each character's advance (`None` for those the font lacks).
    advances: Vec<Option<f32>>,
    width: f32,
    /// The tallest glyph's bottom, 0 for an empty line.
    height: f32,
}

/// `text`'s lines (ending at `\n` and `<br>`), measured like `ComputeTextSize`. A `\r` (text typed
/// on Windows ends lines with `\r\n`) is a character like any other: one a font lacks, so the null
/// character, which makes a line of only `\r` as tall as its glyph rather than empty.
fn lines(text: &str, font: &Font, settings: &Settings) -> Vec<Line> {
    let [sx, sy] = settings.scale;
    let increment = (font.kerning as f32 + settings.spacing[0]) * sx;
    let mut lines = Vec::new();
    for line in text.replace("<br>", "\n").split('\n') {
        let characters: Vec<char> = line.chars().collect();
        let mut advances = Vec::with_capacity(characters.len());
        let (mut width, mut height) = (0.0f32, 0.0f32);
        for (index, &character) in characters.iter().enumerate() {
            let Some((glyph, _)) = font.glyph(character) else {
                advances.push(None);
                continue;
            };
            let mut advance = glyph.size[0] as f32 * sx;
            if characters
                .get(index + 1)
                .is_some_and(|next| !next.is_whitespace())
            {
                advance += increment;
            }
            let x = advances.iter().flatten().sum::<f32>() + advance;
            width = width.max(x);
            let top = glyph.vertical_offset as f32 * sy;
            height = height.max(top + (glyph.size[1] as f32 + settings.spacing[1]) * sy);
            advances.push(Some(advance));
        }
        lines.push(Line {
            characters,
            advances,
            width,
            height,
        });
    }
    lines
}

impl Line {
    /// How far the line moves the next one down.
    fn step(&self, settings: &Settings) -> f32 {
        if self.height > 0.0 {
            self.height
        } else {
            settings.empty_line
        }
    }
}

/// `text`'s size along and across its lines, as `UTextRenderComponent::GetTextLocalSize` measures
/// it: the widest line, and the lines' heights added up.
pub fn size(text: &str, font: &Font, settings: &Settings) -> [f32; 2] {
    lines(text, font, settings)
        .iter()
        .fold([0.0, 0.0], |[width, height], line| {
            [width.max(line.width), height + line.step(settings)]
        })
}

/// How much `UTextBrick::UpdateText` shrinks `text` laid out with `settings` (its font size) to
/// fit `room` (the brick's top face, along and across the lines): never grown, so at most 1.
pub fn fit(text: &str, font: &Font, settings: &Settings, room: [f32; 2]) -> f32 {
    shrink(size(text, font, settings), room)
}

/// How much `UTextBrick::UpdateText` shrinks text of `size` (along and across its lines, at the
/// brick's font size) to fit `room`: never grown, so at most 1.
pub fn shrink([width, height]: [f32; 2], room: [f32; 2]) -> f32 {
    let ratio = (room[0] / width).min(room[1] / height);
    if ratio < 1.0 { ratio } else { 1.0 }
}

/// The quads drawing `text`. Lines end at `\n` and `<br>`.
pub fn layout(text: &str, font: &Font, settings: &Settings) -> Vec<Quad> {
    let [sx, sy] = settings.scale;
    let mut quads = Vec::new();
    let mut line_y = 0.0;
    let mut first_line = None;
    for line in lines(text, font, settings) {
        let start_x = match settings.horizontal {
            Horizontal::Left => 0.0,
            Horizontal::Center => -line.width / 2.0,
            Horizontal::Right => -line.width,
        };
        first_line.get_or_insert(line.height);
        let mut line_x = 0.0;
        for (&character, advance) in line.characters.iter().zip(&line.advances) {
            let (Some((glyph, page)), Some(advance)) = (font.glyph(character), advance) else {
                continue;
            };
            if !character.is_whitespace() {
                let left = -(start_x + line_x);
                let right = left - glyph.size[0] as f32 * sx;
                let top = -(glyph.vertical_offset as f32 * sy + line_y);
                let bottom = top - glyph.size[1] as f32 * sy;
                let [u0, v0] = [
                    glyph.start[0] as f32 / page[0] as f32,
                    glyph.start[1] as f32 / page[1] as f32,
                ];
                let [u1, v1] = [
                    (glyph.start[0] + glyph.size[0]) as f32 / page[0] as f32,
                    (glyph.start[1] + glyph.size[1]) as f32 / page[1] as f32,
                ];
                quads.push(Quad {
                    page: glyph.page,
                    corners: [
                        [0.0, left, top],
                        [0.0, right, top],
                        [0.0, left, bottom],
                        [0.0, right, bottom],
                    ],
                    uvs: [[u0, v0], [u1, v0], [u0, v1], [u1, v1]],
                });
            }
            line_x += *advance;
        }
        line_y += line.step(settings);
    }
    let shift = match settings.vertical {
        Vertical::TextTop => 0.0,
        Vertical::TextCenter => line_y / 2.0,
        Vertical::TextBottom => line_y,
        Vertical::QuadTop => -first_line.unwrap_or(0.0),
    };
    for quad in &mut quads {
        for corner in &mut quad.corners {
            corner[2] += shift;
        }
    }
    quads
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A font of 10 x 20 pixel cells on one 100 x 100 page: 'A' and 'B' remapped to glyphs 0 and
    /// 1, space to 2.
    fn font() -> Font {
        let cell = |index: i32| Glyph {
            start: [index * 10, 0],
            size: [10, 20],
            page: 0,
            vertical_offset: 0,
        };
        Font {
            glyphs: vec![cell(0), cell(1), cell(2)],
            remap: Some(HashMap::from([(65, 0), (66, 1), (32, 2)])),
            pages: vec![Some([100, 100])],
            kerning: 0,
        }
    }

    fn settings() -> Settings {
        Settings {
            scale: [0.5, 0.5],
            spacing: [0.0, 0.0],
            horizontal: Horizontal::Center,
            vertical: Vertical::TextCenter,
            empty_line: 20.0,
        }
    }

    #[test]
    fn centred_line() {
        let quads = layout("AB", &font(), &settings());
        assert_eq!(quads.len(), 2);
        // 10 cm wide, 10 cm tall, centred on the origin: running toward -Y.
        assert_eq!(quads[0].corners[0], [0.0, 5.0, 5.0]);
        assert_eq!(quads[0].corners[3], [0.0, 0.0, -5.0]);
        assert_eq!(quads[1].corners[3], [0.0, -5.0, -5.0]);
        assert_eq!(quads[1].uvs[0], [0.1, 0.0]);
        assert_eq!(quads[1].uvs[3], [0.2, 0.2]);
    }

    #[test]
    fn spaces_lines_and_missing_characters() {
        // The space takes room but draws nothing; 'C' isn't in the font (no null character
        // either); the second line goes below.
        let quads = layout("A B\nC<br>A", &font(), &settings());
        assert_eq!(quads.len(), 3);
        assert_eq!(quads[1].corners[0][1] - quads[0].corners[0][1], -10.0);
        // Three lines of 10 cm (the empty one 20 cm, unscaled), centred: the first line's top at
        // half of 40.
        assert_eq!(quads[0].corners[0][2], 20.0);
        assert_eq!(quads[2].corners[0][2], -10.0);
    }

    #[test]
    fn measured_and_fitted() {
        // The widest line (15 cm), the lines' heights added up (10 cm each, 20 for the empty one).
        assert_eq!(size("A B\n\nA", &font(), &settings()), [15.0, 40.0]);
        // Shrunk by the tighter side, never grown.
        assert_eq!(fit("AB", &font(), &settings(), [5.0, 20.0]), 0.5);
        assert_eq!(fit("AB", &font(), &settings(), [20.0, 5.0]), 0.5);
        assert_eq!(fit("AB", &font(), &settings(), [100.0, 100.0]), 1.0);
        assert_eq!(shrink([40.0, 10.0], [10.0, 10.0]), 0.25);
    }

    #[test]
    fn carriage_returns_are_null_characters() {
        // With a null character (glyph 127), each `\r` is one: it takes its room, so the
        // `\r`-only line is as tall as a glyph (10 cm, not the empty line's 20), but isn't drawn.
        let mut font = font();
        font.glyphs.resize(128, font.glyphs[0]);
        assert_eq!(size("A\r\n\r\nA", &font, &settings()), [10.0, 30.0]);
        assert_eq!(layout("A\r\n\r\nA", &font, &settings()).len(), 2);
    }
}
