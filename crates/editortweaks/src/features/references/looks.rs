//! The materials references show with: instances of materials the game already has (materials
//! can't be made at run time). Found in game 1.11.7; see `docs/re/references.md`.
//!
//! - Images: the engine's widget materials (unlit, a texture and a tint with opacity). The
//!   one-sided ones, since the two-sided ones draw their back black: a two-sided picture gets
//!   triangles facing back too. Masked when opaque (cut-outs follow the picture's alpha, and the
//!   selection outline works), translucent otherwise (no outline: translucent materials don't
//!   draw into custom depth).
//! - Models: `M_Simple_Master`, the game's plain lit material (a color, metallic, roughness), and
//!   for textured parts the Bricksville sign's material (`M_LegacyProp_Master`, lit, with its
//!   `HasBaseColorMap` switch on) with the part's texture as `BaseColor` and a flat normal map.
//!   No map needs to have loaded the sign: it's loaded when a textured part first needs it. If it
//!   can't be, textured parts show unlit with the masked image material (no shading, no shadows).
//! - See-through models, by the setting [`SEE_THROUGH_LOOK`]:
//!   - Dithered (the default): the editor's see-through character material (`M_Character_Editor`,
//!     lit, masked, its `Opacity` a screen-space dither). Its color is a shirt texture and color,
//!     picked by two mask textures: a red one for both shows the shirt everywhere, so the part's
//!     texture and color go there. Masked materials draw depth and motion like opaque ones, so
//!     the engine's temporal anti-aliasing doesn't smear them as it does translucent ones.
//!   - Smooth: the glass bricks' base material (`M_Brick_Translucent`, lit and translucent) with
//!     its `Opacity` and `EdgeOpacity` set to the reference's opacity. It's a layered material: those
//!     are parameters of its one layer, set as such ([`SEE_THROUGH_LAYER`]). It takes its color
//!     from the component's custom primitive data, as bricks do ([`see_through_data`]), and projects
//!     its own textures from the world: a model's textures can't show in it, so textured parts take
//!     their texture's average color. Or they show their texture unlit with the translucent image
//!     material.
//! - Hidden references (the Hide key): the hidden bricks' material (`M_HiddenObject`) as it is.

use std::cell::Cell;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UClass;

use crate::framework::settings::{Alternative, Choice};
use crate::game::assets;
use crate::game::reflection;
use crate::game::textures::Textures;
use crate::game::weak_object::WeakObject;
use crate::prelude::*;

/// How reference models look while see-through, a setting.
pub static SEE_THROUGH_LOOK: Choice = Choice::new(
    "ReferencesSeeThroughLook",
    "See-through models",
    "How a reference model looks while see-through (opacity below 100%). Dithered: a pattern of dots, lit, with its textures, and steady while the camera moves (like hidden bricks). Smooth: really see-through, but the image smears while the camera moves; lit with each texture shown as its average color, or with its textures unlit (no shading). Images are always smooth.",
    &[
        Alternative {
            key: "Dithered",
            name: "Dithered",
        },
        Alternative {
            key: "Smooth",
            name: "Smooth, average colors",
        },
        Alternative {
            key: "SmoothUnlit",
            name: "Smooth, textures unlit",
        },
    ],
    0,
);

/// The alternatives of [`SEE_THROUGH_LOOK`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SeeThroughLook {
    Dithered,
    Smooth,
    SmoothUnlit,
}

/// How see-through models look ([`SEE_THROUGH_LOOK`]).
pub fn see_through_look() -> SeeThroughLook {
    match SEE_THROUGH_LOOK.get() {
        1 => SeeThroughLook::Smooth,
        2 => SeeThroughLook::SmoothUnlit,
        _ => SeeThroughLook::Dithered,
    }
}

/// The image materials' parameters.
const IMAGE_TEXTURE: &str = "SlateUI";
const IMAGE_TINT: &str = "TintColorAndOpacity";
const IMAGE_ALPHA_FROM_TEXTURE: &str = "OpacityFromTexture";

/// The plain material's parameters.
const PLAIN_COLOR: &str = "Color";
const PLAIN_METALLIC: &str = "Metallic";
const PLAIN_ROUGHNESS: &str = "Roughness";

/// The lit textured material, its parameters, and the flat normal map it gets.
const LIT_TEXTURED: &str = "/Game/BrickRigs/Blueprints/Props/BricksvilleSign/Materials/M_Bricksville_Sign.M_Bricksville_Sign";
const LIT_BASE_COLOR: &str = "BaseColor";
const LIT_NORMAL: &str = "Normal";
const LIT_METALLIC: &str = "Metallic";
const FLAT_NORMAL: &str = "/Engine/EngineMaterials/DefaultNormal.DefaultNormal";

/// The see-through material (the glass bricks' base) and its parameters (`docs/re/references.md`).
const SEE_THROUGH: &str =
    "/Game/BrickRigs/Bricks/Materials/M_Brick_Translucent.M_Brick_Translucent";
/// The layer (`ML_Brick_Master`, the only one) whose parameters these are.
const SEE_THROUGH_LAYER: i32 = 0;
const SEE_THROUGH_OPACITY: &str = "Opacity";
const SEE_THROUGH_EDGE_OPACITY: &str = "EdgeOpacity";
/// Plastic, not metal (the material's defaults are 1).
const SEE_THROUGH_PLAIN: [(&str, f32); 4] = [
    ("Metallic", 0.0),
    ("MetallicAlpha", 0.0),
    ("Roughness", 0.5),
    ("Brightness", 0.0),
];

/// The dithered see-through material (the editor's see-through character) and its parameters
/// (`docs/re/references.md`).
const DITHERED: &str = "/Game/BrickRigs/Character/Materials/M_Character_Editor.M_Character_Editor";
const DITHERED_TEXTURE: &str = "Shirt_Texture";
/// Its alpha is how much the shirt shows: all of it.
const DITHERED_COLOR: &str = "Shirt_Color";
/// The masks picking the shirt, the skin, the face...: red in both is the shirt everywhere.
const DITHERED_MASKS: [&str; 2] = ["Mask", "Mask2"];
const DITHERED_NORMAL: &str = "Normal";
const DITHERED_OPACITY: &str = "Opacity";
const DITHERED_PLAIN: [(&str, f32); 2] = [("Metallic", 0.0), ("Roughness", 0.6)];

/// The hidden bricks' material.
const HIDDEN: &str = "/Game/BrickRigs/BrickEditor/Materials/M_HiddenObject.M_HiddenObject";

/// The custom primitive data a see-through part needs (what the brick material reads there): its
/// color (slots 0-3), then the brick's place and axes its textures are projected along (4-6, 7-9,
/// 10-12), the axes' defaults.
pub fn see_through_data(color: [f32; 4]) -> [f32; 13] {
    let [r, g, b, _] = color;
    [r, g, b, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0]
}

/// The base materials, found once the editor is open (they're loaded by then).
pub struct Looks {
    masked: *mut UObjectBase,
    translucent: *mut UObjectBase,
    plain: *mut UObjectBase,
    /// The lit textured material and its flat normal map.
    lit: Lazy,
    flat_normal: Lazy,
    see_through: Lazy,
    dithered: Lazy,
    hidden: Lazy,
    /// A red texture for the dithered material's masks, made when first needed.
    red: Cell<Option<WeakObject>>,
}

// SAFETY: engine materials loaded for good (engine content and the game's brick materials); only
// used on the game thread.
unsafe impl Send for Looks {}

/// An asset loaded when first needed, and again if the engine freed it (nothing holds it while no
/// part uses it).
struct Lazy {
    class: *const UClass,
    path: &'static str,
    loaded: Cell<Option<WeakObject>>,
    /// Set once loading failed (logged then).
    missing: Cell<bool>,
}

impl Looks {
    pub fn find() -> Result<Self, String> {
        let find = |name, class| {
            reflection::find_object(name, class).ok_or(format!("material {name} not loaded"))
        };
        let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
        let instance = "MaterialInstanceConstant";
        let material = class("MaterialInterface")?;
        Ok(Self {
            masked: find("Widget3DPassThrough_Masked_OneSided", instance)?,
            translucent: find("Widget3DPassThrough_Translucent_OneSided", instance)?,
            plain: find("M_Simple_Master", "Material")?,
            lit: Lazy::new(material, LIT_TEXTURED),
            flat_normal: Lazy::new(class("Texture")?, FLAT_NORMAL),
            see_through: Lazy::new(material, SEE_THROUGH),
            dithered: Lazy::new(material, DITHERED),
            hidden: Lazy::new(material, HIDDEN),
            red: Cell::new(None),
        })
    }

    /// The hidden bricks' material; null if it can't be had.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn hidden(&self) -> *mut UObjectBase {
        // SAFETY: per the contract.
        unsafe { self.hidden.get() }.unwrap_or(core::ptr::null_mut())
    }

    /// A lit, dithered see-through material for a model part showing `texture` (null for none)
    /// tinted with `color` (linear RGBA; alpha unused), with `opacity`; null if the material can't
    /// be had or the engine made none.
    ///
    /// # Safety
    /// As for [`Self::image`].
    pub unsafe fn dithered(
        &self,
        textures: &Textures,
        context: *mut UObjectBase,
        texture: *mut UObjectBase,
        color: [f32; 4],
        opacity: f32,
    ) -> *mut UObjectBase {
        // SAFETY: per the contract.
        unsafe {
            let (Some(parent), Some(flat_normal)) = (self.dithered.get(), self.flat_normal.get())
            else {
                return core::ptr::null_mut();
            };
            let red = self.red(textures, context);
            if red.is_null() {
                return core::ptr::null_mut();
            }
            let material = textures.instance(context, parent);
            if !material.is_null() {
                for mask in DITHERED_MASKS {
                    textures.set_texture(material, mask, red);
                }
                // Without a texture, the material's own: white.
                if !texture.is_null() {
                    textures.set_texture(material, DITHERED_TEXTURE, texture);
                }
                textures.set_color(
                    material,
                    DITHERED_COLOR,
                    [color[0], color[1], color[2], 1.0],
                );
                textures.set_texture(material, DITHERED_NORMAL, flat_normal);
                for (name, value) in DITHERED_PLAIN {
                    textures.set_scalar(material, name, value);
                }
                Self::set_dithered_opacity(textures, material, opacity);
            }
            material
        }
    }

    /// Changes the opacity of a material made by [`Self::dithered`].
    ///
    /// # Safety
    /// `material` must be a live dithered material. Game thread only.
    pub unsafe fn set_dithered_opacity(
        textures: &Textures,
        material: *mut UObjectBase,
        opacity: f32,
    ) {
        // SAFETY: per the contract.
        unsafe { textures.set_scalar(material, DITHERED_OPACITY, opacity.clamp(0.0, 1.0)) };
    }

    /// A 1 x 1 red texture, imported once (again if the engine freed it); null if it can't be.
    ///
    /// # Safety
    /// As for [`Self::image`].
    unsafe fn red(&self, textures: &Textures, context: *mut UObjectBase) -> *mut UObjectBase {
        if let Some(red) = self.red.get().and_then(|weak| weak.get_live()) {
            return red;
        }
        let image = model_io::Image {
            width: 1,
            height: 1,
            rgba: vec![255, 0, 0, 255],
        };
        let Ok(png) = image.to_png() else {
            return core::ptr::null_mut();
        };
        // SAFETY: per the contract.
        let red = unsafe { textures.import_buffer(context, &png) };
        if !red.is_null() {
            // SAFETY: just made, live.
            self.red.set(Some(unsafe { WeakObject::new(red) }));
        }
        red
    }

    /// A material showing `texture` (null for none) with `opacity`; null if the engine made none.
    ///
    /// # Safety
    /// `context` must be a live object in the editor's world, `texture` null or a live texture.
    /// Game thread only.
    pub unsafe fn image(
        &self,
        textures: &Textures,
        context: *mut UObjectBase,
        texture: *mut UObjectBase,
        opacity: f32,
    ) -> *mut UObjectBase {
        let parent = if is_opaque(opacity) {
            self.masked
        } else {
            self.translucent
        };
        // SAFETY: per the contract.
        unsafe {
            let material = textures.instance(context, parent);
            if !material.is_null() {
                textures.set_texture(material, IMAGE_TEXTURE, texture);
                self.set_image_opacity(textures, material, opacity);
                textures.set_scalar(material, IMAGE_ALPHA_FROM_TEXTURE, 1.0);
            }
            material
        }
    }

    /// Changes the opacity of an image material made by [`Self::image`] (with the same opacity
    /// kind: a masked one stays opaque).
    ///
    /// # Safety
    /// `material` must be a live image material. Game thread only.
    pub unsafe fn set_image_opacity(
        &self,
        textures: &Textures,
        material: *mut UObjectBase,
        opacity: f32,
    ) {
        // SAFETY: per the contract.
        unsafe {
            textures.set_color(
                material,
                IMAGE_TINT,
                [1.0, 1.0, 1.0, opacity.clamp(0.0, 1.0)],
            );
        }
    }

    /// A lit material of `color` (linear RGBA; alpha unused); null if the engine made none.
    ///
    /// # Safety
    /// As for [`Self::image`].
    pub unsafe fn plain(
        &self,
        textures: &Textures,
        context: *mut UObjectBase,
        color: [f32; 4],
    ) -> *mut UObjectBase {
        // SAFETY: per the contract.
        unsafe {
            let material = textures.instance(context, self.plain);
            if !material.is_null() {
                textures.set_color(material, PLAIN_COLOR, [color[0], color[1], color[2], 1.0]);
                textures.set_scalar(material, PLAIN_METALLIC, 0.0);
                textures.set_scalar(material, PLAIN_ROUGHNESS, 0.7);
            }
            material
        }
    }

    /// A lit material showing `texture` (a live texture) for a model part, or the unlit image one
    /// if the lit one can't be had; null if the engine made none.
    ///
    /// # Safety
    /// As for [`Self::image`].
    pub unsafe fn textured(
        &self,
        textures: &Textures,
        context: *mut UObjectBase,
        texture: *mut UObjectBase,
    ) -> *mut UObjectBase {
        // SAFETY: per the contract.
        unsafe {
            let (Some(parent), Some(flat_normal)) = (self.lit.get(), self.flat_normal.get()) else {
                return self.image(textures, context, texture, 1.0);
            };
            let material = textures.instance(context, parent);
            if !material.is_null() {
                textures.set_texture(material, LIT_BASE_COLOR, texture);
                textures.set_texture(material, LIT_NORMAL, flat_normal);
                textures.set_scalar(material, LIT_METALLIC, 0.0);
            }
            material
        }
    }

    /// A lit see-through material for model parts with `opacity`, their color in their custom
    /// primitive data ([`see_through_data`]); null if the material can't be had or the engine made
    /// none.
    ///
    /// # Safety
    /// As for [`Self::image`].
    pub unsafe fn see_through(
        &self,
        textures: &Textures,
        context: *mut UObjectBase,
        opacity: f32,
    ) -> *mut UObjectBase {
        // SAFETY: per the contract.
        unsafe {
            let Some(parent) = self.see_through.get() else {
                return core::ptr::null_mut();
            };
            let material = textures.instance(context, parent);
            if !material.is_null() {
                for (name, value) in SEE_THROUGH_PLAIN {
                    set_see_through_scalar(textures, material, name, value);
                }
                Self::set_see_through_opacity(textures, material, opacity);
            }
            material
        }
    }

    /// Changes the opacity of a material made by [`Self::see_through`].
    ///
    /// # Safety
    /// `material` must be a live see-through material. Game thread only.
    pub unsafe fn set_see_through_opacity(
        textures: &Textures,
        material: *mut UObjectBase,
        opacity: f32,
    ) {
        let opacity = opacity.clamp(0.0, 1.0);
        // SAFETY: per the contract. The same opacity at the edges: no glassy rim.
        unsafe {
            set_see_through_scalar(textures, material, SEE_THROUGH_OPACITY, opacity);
            set_see_through_scalar(textures, material, SEE_THROUGH_EDGE_OPACITY, opacity);
        }
    }
}

/// Sets the scalar `name` of the see-through material's layer (logged once if it can't be: the
/// material then keeps the glass bricks' look).
///
/// # Safety
/// `material` must be a live see-through material. Game thread only.
unsafe fn set_see_through_scalar(
    textures: &Textures,
    material: *mut UObjectBase,
    name: &str,
    value: f32,
) {
    static MISSING: std::sync::Once = std::sync::Once::new();
    // SAFETY: per the contract.
    if !unsafe { textures.set_layer_scalar(material, SEE_THROUGH_LAYER, name, value) } {
        MISSING.call_once(|| log!("see-through models keep the glass look: no layer setter"));
    }
}

/// Whether `opacity` shows a reference opaque (its own materials) rather than see-through.
pub fn is_opaque(opacity: f32) -> bool {
    opacity >= 1.0
}

impl Lazy {
    fn new(class: *const UClass, path: &'static str) -> Self {
        Self {
            class,
            path,
            loaded: Cell::new(None),
            missing: Cell::new(false),
        }
    }

    /// The asset, loaded now if needed; `None` if it can't be (logged the first time).
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn get(&self) -> Option<*mut UObjectBase> {
        if self.missing.get() {
            return None;
        }
        if let Some(object) = self.loaded.get().and_then(|weak| weak.get_live()) {
            return Some(object);
        }
        // SAFETY: per the contract; the class is live (a native class).
        let object = unsafe { assets::load(self.class, self.path) };
        if object.is_null() {
            log!(
                "{} not found: the parts needing it show another way",
                self.path
            );
            self.missing.set(true);
            return None;
        }
        // SAFETY: just loaded, live.
        self.loaded.set(Some(unsafe { WeakObject::new(object) }));
        Some(object)
    }
}
