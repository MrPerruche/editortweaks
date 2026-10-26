//! Reading a texture's pixels back. The shipping game keeps textures on the GPU only (their source
//! data is editor-only), so a texture is drawn onto a render target of its own and the engine
//! writes that to a PNG, all through the engine's reflected Blueprint functions (UE 4.27):
//!
//! - `UKismetRenderingLibrary::CreateRenderTarget2D(WorldContextObject, Width, Height, Format,
//!   ClearColor, bAutoGenerateMipMaps)`, then `ReleaseRenderTarget2D(TextureRenderTarget)`.
//! - `BeginDrawCanvasToRenderTarget(WorldContextObject, TextureRenderTarget, UCanvas*& Canvas,
//!   FVector2D& Size, FDrawToRenderTargetContext& Context)`, `UCanvas::K2_DrawTexture(RenderTexture,
//!   ScreenPosition, ScreenSize, CoordinatePosition, CoordinateSize, RenderColor, BlendMode,
//!   Rotation, PivotPoint)`, `EndDrawCanvasToRenderTarget(WorldContextObject, Context)`.
//! - `ExportRenderTarget(WorldContextObject, TextureRenderTarget, FilePath, FileName)`: a PNG for
//!   8-bit render targets (`FImageUtils::ExportRenderTarget2DAsPNG`, which waits for the GPU).
//!
//! How the canvas and render target treat colour (sRGB textures are sampled as linear values,
//! then the canvas applies the render target's gamma) isn't assumed: a gradient of our own
//! (`game::textures`, an sRGB texture) goes through the same steps first, and what each byte came
//! out as is undone ([`model_io::image::inverse_curve`]). Its alpha shows whether alpha comes
//! through at all.

use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UStruct;

use model_io::image::{self, Image, inverse_curve, linear_to_srgb, srgb_to_linear};

use crate::game::reflection::{self, BoolField, Function, Parameters, put, take};
use crate::game::slider_text::{FString, OwnedFString};
use crate::game::textures::{Call, LinearColor, POINTER, Textures};
use crate::prelude::log;

/// `FVector2D`.
type Vector2 = [f32; 2];

/// `FDrawToRenderTargetContext`: the render target and a draw event, two pointers.
type DrawContext = [u8; 16];

const LIBRARY: &str = "KismetRenderingLibrary";

/// The calibration gradient's width: one pixel per byte value.
const GRADIENT: usize = 256;

/// `CreateRenderTarget2D` and where its parameters go.
struct Create {
    function: Function,
    context: usize,
    width: usize,
    height: usize,
    format: usize,
    result: usize,
}

/// How bytes came out of the steps, undone: the linear value each colour byte stands for, and
/// the alpha each alpha byte stands for (`None` if alpha didn't come through).
struct Calibration {
    linear: [f32; 256],
    alpha: Option<[f32; 256]>,
}

/// A texture read back.
pub struct Read {
    /// sRGB colour for sRGB textures, else the raw values; alpha 255 if it didn't come through.
    pub image: Image,
    pub srgb: bool,
}

/// Reads textures back (after engine init).
pub struct Readback {
    /// `KismetRenderingLibrary`'s default object, which its static functions are called on.
    rendering: *mut UObjectBase,
    textures: Textures,
    create: Create,
    /// `ETextureRenderTargetFormat::RTF_RGBA8`.
    rgba8: u8,
    begin: Call<3>,
    /// The begin call's `Context`, passed on to the end call.
    begin_context: usize,
    draw: Call<7>,
    /// Where the draw call's `BlendMode` goes, and `EBlendMode::BLEND_Opaque`: the texture's
    /// alpha is written, not blended.
    blend: usize,
    opaque: u8,
    end: Call<2>,
    export: Call<4>,
    /// `None` if not found: render targets stay until they're collected.
    release: Option<Call<1>>,
    /// `UTexture::SRGB`, and `Texture`.
    srgb: BoolField,
    texture_class: *const UStruct,
    calibration: Mutex<Option<Result<Calibration, String>>>,
}

// SAFETY: reflection records and default objects living as long as the game; only used on the
// game thread.
unsafe impl Send for Readback {}
unsafe impl Sync for Readback {}

/// The offset of `function`'s one-byte enum parameter `name`.
fn byte_enum(function: &Function, name: &'static str) -> Result<usize, String> {
    function
        .parameter(name, "ByteProperty", 1)
        .or_else(|_| function.parameter(name, "EnumProperty", 1))
}

/// The value of the enum `name`'s `entry`, which must fit in a byte.
fn enum_byte(name: &'static str, entry: &str) -> Result<u8, String> {
    let value = reflection::enum_value(name, entry)
        .or_else(|_| reflection::enum_value(name, &format!("{name}::{entry}")))?;
    u8::try_from(value).map_err(|_| format!("{name}: unexpected value {value}"))
}

impl Readback {
    pub fn find() -> Result<Self, String> {
        let library =
            reflection::find_class(LIBRARY).ok_or(format!("class {LIBRARY} not found"))?;
        let create = Function::find(library, "CreateRenderTarget2D")?;
        let create = Create {
            context: create.parameter("WorldContextObject", "ObjectProperty", POINTER)?,
            width: create.parameter("Width", "IntProperty", 4)?,
            height: create.parameter("Height", "IntProperty", 4)?,
            format: byte_enum(&create, "Format")?,
            result: create.parameter("ReturnValue", "ObjectProperty", POINTER)?,
            function: create,
        };
        let vector2 = size_of::<Vector2>() as i32;
        let draw = Call::find(
            "Canvas",
            "K2_DrawTexture",
            [
                ("RenderTexture", "ObjectProperty", POINTER),
                ("ScreenPosition", "StructProperty", vector2),
                ("ScreenSize", "StructProperty", vector2),
                ("CoordinatePosition", "StructProperty", vector2),
                ("CoordinateSize", "StructProperty", vector2),
                (
                    "RenderColor",
                    "StructProperty",
                    size_of::<LinearColor>() as i32,
                ),
                ("PivotPoint", "StructProperty", vector2),
            ],
        )?;
        let context = size_of::<DrawContext>() as i32;
        let begin = Call::find(
            LIBRARY,
            "BeginDrawCanvasToRenderTarget",
            [
                ("WorldContextObject", "ObjectProperty", POINTER),
                ("TextureRenderTarget", "ObjectProperty", POINTER),
                ("Canvas", "ObjectProperty", POINTER),
            ],
        )?;
        let string = size_of::<FString>() as i32;
        let texture = reflection::find_class("Texture").ok_or("class Texture not found")?;
        Ok(Self {
            rendering: reflection::find_object("Default__KismetRenderingLibrary", LIBRARY)
                .ok_or(format!("{LIBRARY}'s default object not found"))?,
            textures: Textures::find()?,
            create,
            rgba8: enum_byte("ETextureRenderTargetFormat", "RTF_RGBA8")?,
            begin_context: begin
                .function
                .parameter("Context", "StructProperty", context)?,
            begin,
            blend: byte_enum(&draw.function, "BlendMode")?,
            draw,
            opaque: enum_byte("EBlendMode", "BLEND_Opaque")?,
            end: Call::find(
                LIBRARY,
                "EndDrawCanvasToRenderTarget",
                [
                    ("WorldContextObject", "ObjectProperty", POINTER),
                    ("Context", "StructProperty", context),
                ],
            )?,
            export: Call::find(
                LIBRARY,
                "ExportRenderTarget",
                [
                    ("WorldContextObject", "ObjectProperty", POINTER),
                    ("TextureRenderTarget", "ObjectProperty", POINTER),
                    ("FilePath", "StrProperty", string),
                    ("FileName", "StrProperty", string),
                ],
            )?,
            release: Call::find(
                LIBRARY,
                "ReleaseRenderTarget2D",
                [("TextureRenderTarget", "ObjectProperty", POINTER)],
            )
            .map_err(|reason| log!("render targets aren't released: {reason}"))
            .ok(),
            srgb: reflection::bool_field(texture.cast(), "SRGB")?,
            texture_class: texture.cast(),
            calibration: Mutex::new(None),
        })
    }

    /// `texture` (a `UTexture2D`) read back, at most `largest` pixels on its longer side (halved
    /// until it fits).
    ///
    /// # Safety
    /// `context` must be a live object in a world (e.g. an actor), `texture` a live texture. Game
    /// thread only.
    pub unsafe fn read(
        &self,
        context: *mut UObjectBase,
        texture: *mut UObjectBase,
        largest: u32,
    ) -> Result<Read, String> {
        // SAFETY: per the contract.
        unsafe {
            if !reflection::is_a(texture, self.texture_class) {
                return Err("not a texture".into());
            }
            let mut calibration = self
                .calibration
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let calibration = calibration
                .get_or_insert_with(|| self.calibrate(context))
                .as_ref()
                .map_err(Clone::clone)?;
            let [width, height] = self.textures.size(texture);
            if width <= 0 || height <= 0 {
                return Err(format!("its size is {width} x {height}"));
            }
            let (mut width, mut height) = (width as u32, height as u32);
            while width.max(height) > largest.max(1) {
                width = (width / 2).max(1);
                height = (height / 2).max(1);
            }
            let srgb = self.srgb.get(texture.cast());
            let mut image = self.drawn(context, texture, width, height)?;
            for pixel in image.rgba.as_chunks_mut::<4>().0 {
                for channel in &mut pixel[..3] {
                    let value = calibration.linear[usize::from(*channel)];
                    let value = if srgb { linear_to_srgb(value) } else { value };
                    *channel = to_byte(value);
                }
                pixel[3] = calibration
                    .alpha
                    .map_or(255, |alpha| to_byte(alpha[usize::from(pixel[3])]));
            }
            Ok(Read { image, srgb })
        }
    }

    /// Sends a gradient through the steps to see what they do to bytes.
    ///
    /// # Safety
    /// As for [`Self::read`].
    unsafe fn calibrate(&self, context: *mut UObjectBase) -> Result<Calibration, String> {
        let gradient = Image {
            width: GRADIENT as u32,
            height: 1,
            rgba: (0..GRADIENT).flat_map(|value| [value as u8; 4]).collect(),
        };
        let png = gradient.to_png()?;
        // SAFETY: per the contract; the new texture lives until the next garbage collection,
        // after this call.
        let (texture, drawn) = unsafe {
            let texture = self.textures.import_buffer(context, &png);
            if texture.is_null() {
                return Err("the calibration gradient couldn't be made".into());
            }
            (texture, self.drawn(context, texture, GRADIENT as u32, 1)?)
        };
        // SAFETY: a live texture.
        let srgb = unsafe { self.srgb.get(texture.cast()) };
        let values: [f32; 256] = core::array::from_fn(|byte| {
            let value = byte as f32 / 255.0;
            if srgb { srgb_to_linear(value) } else { value }
        });
        let channel = |channel: usize| -> [u8; 256] {
            core::array::from_fn(|pixel| drawn.rgba[pixel * 4 + channel])
        };
        let linear = inverse_curve(&values, &channel(0))
            .ok_or("the calibration gradient didn't come through")?;
        let straight: [f32; 256] = core::array::from_fn(|byte| byte as f32 / 255.0);
        let alpha = inverse_curve(&straight, &channel(3));
        log!(
            "textures read back: bytes 0, 128, 255 of an {} gradient came out as {}, {}, {}; alpha \
             {}",
            if srgb { "sRGB" } else { "linear" },
            drawn.rgba[0],
            drawn.rgba[128 * 4],
            drawn.rgba[255 * 4],
            if alpha.is_some() {
                "comes through"
            } else {
                "doesn't come through"
            }
        );
        Ok(Calibration { linear, alpha })
    }

    /// `texture` drawn onto a new `width` x `height` render target, as the engine writes it out.
    ///
    /// # Safety
    /// As for [`Self::read`].
    unsafe fn drawn(
        &self,
        context: *mut UObjectBase,
        texture: *mut UObjectBase,
        width: u32,
        height: u32,
    ) -> Result<Image, String> {
        let path = std::env::temp_dir().join("editortweaks-texture.png");
        // Left from an earlier read that failed, or not there.
        let _ = std::fs::remove_file(&path);
        // SAFETY: per the contract; each call gets the parameters its reflection lists, the
        // render target is live until it's released (no garbage collection in between).
        unsafe {
            let target = self.target(context, width, height)?;
            let result = self.draw_and_export(context, target, texture, width, height, &path);
            if let Some(release) = &self.release {
                let mut parameters = Parameters::new();
                put(&mut parameters, release.parameters[0], target);
                release.function.call(self.rendering, &mut parameters);
            }
            result?;
        }
        let read = image::read(&path);
        let _ = std::fs::remove_file(&path);
        let read = read?;
        if (read.width, read.height) != (width, height) {
            return Err(format!(
                "it came out {} x {}, not {width} x {height}",
                read.width, read.height
            ));
        }
        Ok(read)
    }

    /// A new 8-bit render target.
    ///
    /// # Safety
    /// As for [`Self::read`].
    unsafe fn target(
        &self,
        context: *mut UObjectBase,
        width: u32,
        height: u32,
    ) -> Result<*mut UObjectBase, String> {
        let create = &self.create;
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; the clear colour stays black and mip maps off (zero).
        let target: *mut UObjectBase = unsafe {
            put(&mut parameters, create.context, context);
            put(&mut parameters, create.width, width as i32);
            put(&mut parameters, create.height, height as i32);
            put(&mut parameters, create.format, self.rgba8);
            create.function.call(self.rendering, &mut parameters);
            take(&parameters, create.result)
        };
        if target.is_null() {
            return Err("no render target was made".into());
        }
        Ok(target)
    }

    /// Draws `texture` over the whole of `target`, then has the engine write it to `path`.
    ///
    /// # Safety
    /// As for [`Self::read`], `target` a live render target of that size.
    unsafe fn draw_and_export(
        &self,
        context: *mut UObjectBase,
        target: *mut UObjectBase,
        texture: *mut UObjectBase,
        width: u32,
        height: u32,
        path: &std::path::Path,
    ) -> Result<(), String> {
        let folder = path.parent().map(PathBuf::from).unwrap_or_default();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (folder, name) = (
            OwnedFString::new(&folder.to_string_lossy()),
            OwnedFString::new(&name),
        );
        // SAFETY: per the contract; the begin call fills the canvas and the context, which the
        // end call gets back as it was; the strings outlive the export call, which only reads
        // them.
        unsafe {
            let mut parameters = Parameters::new();
            put(&mut parameters, self.begin.parameters[0], context);
            put(&mut parameters, self.begin.parameters[1], target);
            self.begin.function.call(self.rendering, &mut parameters);
            let canvas: *mut UObjectBase = take(&parameters, self.begin.parameters[2]);
            let draw_context: DrawContext = take(&parameters, self.begin_context);

            if !canvas.is_null() {
                let draw = &self.draw;
                let size = [width as f32, height as f32];
                let mut parameters = Parameters::new();
                put(&mut parameters, draw.parameters[0], texture);
                put(&mut parameters, draw.parameters[1], [0.0f32; 2]);
                put(&mut parameters, draw.parameters[2], size);
                put(&mut parameters, draw.parameters[3], [0.0f32; 2]);
                put(&mut parameters, draw.parameters[4], [1.0f32; 2]);
                put(&mut parameters, draw.parameters[5], [1.0f32; 4]);
                put(&mut parameters, draw.parameters[6], [0.5f32; 2]);
                put(&mut parameters, self.blend, self.opaque);
                draw.function.call(canvas, &mut parameters);
            }

            let mut parameters = Parameters::new();
            put(&mut parameters, self.end.parameters[0], context);
            put(&mut parameters, self.end.parameters[1], draw_context);
            self.end.function.call(self.rendering, &mut parameters);
            if canvas.is_null() {
                return Err("no canvas to draw on".into());
            }

            let export = &self.export;
            let mut parameters = Parameters::new();
            put(&mut parameters, export.parameters[0], context);
            put(&mut parameters, export.parameters[1], target);
            put(
                &mut parameters,
                export.parameters[2],
                folder.as_ptr().read(),
            );
            put(&mut parameters, export.parameters[3], name.as_ptr().read());
            export.function.call(self.rendering, &mut parameters);
        }
        Ok(())
    }
}

/// `value` (0 to 1) as a byte.
fn to_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}
