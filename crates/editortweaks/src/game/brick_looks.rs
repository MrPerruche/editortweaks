//! How a brick looks, as numbers: its color and what its brick material (plastic, steel, glass,
//! ...) makes of it. All reflected, found by name in BRMK 1.11.6 (`UBrickMaterial`'s generated
//! property records) and game 1.11.7 (build 25831956); see `docs/re/model-export.md`.
//!
//! - `UBrick::BrickColor` (`FColor`: blue, green, red, alpha bytes; sRGB like every `FColor`) and
//!   `UBrick::BrickMaterial` (`UBrickMaterial*`).
//! - `UBrickMaterial` (one asset per material, e.g. `/Game/BrickRigs/Bricks/BrickMaterials/Steel`):
//!   `Metallic`, `Roughness`, `Opacity` (below 1 for the see-through ones, which the game draws with
//!   its translucent material), `Brightness` (how much it glows), `BaseTiling`,
//!   `PaintMetallicAlpha`, and the textures the game's material blends with them
//!   (`DiffuseTexture`, `NormalTexture`, `MRAOTexture`, `EmissiveTexture`, `UTexture2D*`s, null
//!   when unused).

use brickrust::ue::coreuobject::UObjectBase;

use crate::game::reflection;

/// A brick's looks.
#[derive(Clone, Debug, PartialEq)]
pub struct Looks {
    /// Its material's object name, e.g. `"Steel"` (empty without one).
    pub material: String,
    /// sRGB red, green, blue, alpha.
    pub color: [u8; 4],
    pub metallic: f32,
    pub roughness: f32,
    /// 1 for opaque materials.
    pub opacity: f32,
    pub brightness: f32,
    pub base_tiling: f32,
    pub paint_metallic_alpha: f32,
    /// The material's textures (null when it has none of a kind); all null without a material.
    pub textures: Textures,
}

/// A brick material's textures, live assets or null.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Textures {
    pub diffuse: *mut UObjectBase,
    pub normal: *mut UObjectBase,
    /// Metallic, roughness and ambient occlusion (red, green, blue).
    pub mrao: *mut UObjectBase,
    pub emissive: *mut UObjectBase,
}

impl Default for Textures {
    fn default() -> Self {
        Self {
            diffuse: core::ptr::null_mut(),
            normal: core::ptr::null_mut(),
            mrao: core::ptr::null_mut(),
            emissive: core::ptr::null_mut(),
        }
    }
}

/// Reads bricks' looks (after engine init).
pub struct BrickLooks {
    /// Offsets of `UBrick::BrickColor` and `UBrick::BrickMaterial`.
    color: usize,
    material: usize,
    /// Offsets of `UBrickMaterial`'s `Metallic`, `Roughness`, `Opacity` and `Brightness`.
    metallic: usize,
    roughness: usize,
    opacity: usize,
    brightness: usize,
    base_tiling: usize,
    paint_metallic_alpha: usize,
    /// Offsets of `DiffuseTexture`, `NormalTexture`, `MRAOTexture`, `EmissiveTexture`.
    textures: [usize; 4],
}

impl BrickLooks {
    pub fn find() -> Result<Self, String> {
        let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
        let brick = class("Brick")?;
        let material = class("BrickMaterial")?;
        let float = |name| reflection::field_offset(material, name, "FloatProperty", 4);
        let texture = |name| reflection::field_offset(material, name, "ObjectProperty", 8);
        Ok(Self {
            color: reflection::field_offset(brick, "BrickColor", "StructProperty", 4)?,
            material: reflection::field_offset(brick, "BrickMaterial", "ObjectProperty", 8)?,
            metallic: float("Metallic")?,
            roughness: float("Roughness")?,
            opacity: float("Opacity")?,
            brightness: float("Brightness")?,
            base_tiling: float("BaseTiling")?,
            paint_metallic_alpha: float("PaintMetallicAlpha")?,
            textures: [
                texture("DiffuseTexture")?,
                texture("NormalTexture")?,
                texture("MRAOTexture")?,
                texture("EmissiveTexture")?,
            ],
        })
    }

    /// The looks of `brick`.
    ///
    /// # Safety
    /// `brick` must be a live brick (a `UBrick`). Game thread only.
    pub unsafe fn of(&self, brick: *mut UObjectBase) -> Looks {
        // SAFETY: per the contract; the offsets come from reflection, and the brick material is
        // null or a live asset (bricks hold theirs).
        unsafe {
            let base = brick.cast::<u8>();
            let [blue, green, red, alpha] = base.add(self.color).cast::<[u8; 4]>().read();
            let material = base.add(self.material).cast::<*mut UObjectBase>().read();
            let mut looks = Looks {
                material: String::new(),
                color: [red, green, blue, alpha],
                metallic: 0.0,
                roughness: 0.5,
                opacity: 1.0,
                brightness: 0.0,
                base_tiling: 1.0,
                paint_metallic_alpha: 0.0,
                textures: Textures::default(),
            };
            if !material.is_null() {
                let float = |offset| material.cast::<u8>().add(offset).cast::<f32>().read();
                looks.material = reflection::name_text(&(*material).name_private);
                looks.metallic = float(self.metallic);
                looks.roughness = float(self.roughness);
                looks.opacity = float(self.opacity);
                looks.brightness = float(self.brightness);
                looks.base_tiling = float(self.base_tiling);
                looks.paint_metallic_alpha = float(self.paint_metallic_alpha);
                let [diffuse, normal, mrao, emissive] = self.textures.map(|offset| {
                    material
                        .cast::<u8>()
                        .add(offset)
                        .cast::<*mut UObjectBase>()
                        .read()
                });
                looks.textures = Textures {
                    diffuse,
                    normal,
                    mrao,
                    emissive,
                };
            }
            looks
        }
    }
}
