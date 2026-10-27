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
//! - Patterns ([`BrickPatterns`]): `UBrick::BrickPattern` (a `UBrickPattern` class, e.g.
//!   `/Game/BrickRigs/Bricks/Patterns/C_Army`; null without one), whose default object holds
//!   `Texture` (`UTexture2D*`) and `Tiling` (cm per repeat); the brick's `PatternOffset` and
//!   `PatternScale` (`FVector2D`s) and `PatternRotation` (degrees). `UBrick::UpdatePrimitiveData`
//!   hands the material the offset, `1 / PatternScale` and the rotation in radians.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UClass;

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

/// A brick's pattern, as the brick material draws it over the colour.
#[derive(Clone, Debug, PartialEq)]
pub struct Pattern {
    /// Its class's name, e.g. `"C_Army_C"`.
    pub name: String,
    /// A live texture.
    pub texture: *mut UObjectBase,
    /// The pattern's `Tiling`: cm per repeat.
    pub tiling: f32,
    /// The brick's `PatternOffset`, in repeats.
    pub offset: [f32; 2],
    /// The brick's `PatternScale`.
    pub scale: [f32; 2],
    /// The brick's `PatternRotation`, in degrees.
    pub rotation: f32,
}

/// Reads bricks' patterns (after engine init).
pub struct BrickPatterns {
    /// `UBrickPattern`, to check the pattern classes' default objects against.
    class: *const UClass,
    /// Offsets of `UBrick`'s `BrickPattern`, `PatternOffset`, `PatternScale`, `PatternRotation`.
    pattern: usize,
    offset: usize,
    scale: usize,
    rotation: usize,
    /// Offsets of `UBrickPattern`'s `Texture` and `Tiling`.
    texture: usize,
    tiling: usize,
}

// SAFETY: class records live as long as the game; only used on the game thread.
unsafe impl Send for BrickPatterns {}
// SAFETY: as above.
unsafe impl Sync for BrickPatterns {}

impl BrickPatterns {
    pub fn find() -> Result<Self, String> {
        let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
        let brick = class("Brick")?;
        let pattern = class("BrickPattern")?;
        Ok(Self {
            class: pattern,
            pattern: reflection::field_offset(brick, "BrickPattern", "ClassProperty", 8)?,
            offset: reflection::field_offset(brick, "PatternOffset", "StructProperty", 8)?,
            scale: reflection::field_offset(brick, "PatternScale", "StructProperty", 8)?,
            rotation: reflection::field_offset(brick, "PatternRotation", "FloatProperty", 4)?,
            texture: reflection::field_offset(pattern, "Texture", "ObjectProperty", 8)?,
            tiling: reflection::field_offset(pattern, "Tiling", "FloatProperty", 4)?,
        })
    }

    /// The pattern of `brick`, `None` if it has none (or one without a texture).
    ///
    /// # Safety
    /// `brick` must be a live brick (a `UBrick`). Game thread only.
    pub unsafe fn of(&self, brick: *mut UObjectBase) -> Option<Pattern> {
        // SAFETY: per the contract; the offsets come from reflection, the brick's pattern class is
        // null or a live class (bricks hold theirs), and its default object is checked to be a
        // `UBrickPattern` before its fields are read.
        unsafe {
            let base = brick.cast::<u8>();
            let class = base.add(self.pattern).cast::<*const UClass>().read();
            if class.is_null() {
                return None;
            }
            let defaults = reflection::default_object(class)?;
            if !reflection::is_a(defaults, self.class.cast()) {
                return None;
            }
            let fields = defaults.cast::<u8>();
            let texture = fields.add(self.texture).cast::<*mut UObjectBase>().read();
            if texture.is_null() {
                return None;
            }
            Some(Pattern {
                name: reflection::name_text(reflection::struct_name(class.cast())),
                texture,
                tiling: fields.add(self.tiling).cast::<f32>().read(),
                offset: base.add(self.offset).cast::<[f32; 2]>().read(),
                scale: base.add(self.scale).cast::<[f32; 2]>().read(),
                rotation: base.add(self.rotation).cast::<f32>().read(),
            })
        }
    }
}
