//! Vehicle thumbnails on images of ours, asked for the way the browser's rows ask for theirs.
//!
//! `UUGCItemWidget::InitializeEntry_Implementation` (BRMK 0x3CC8C0) gets its row's thumbnail path
//! from `FUGCFileInfo::GetThumbnailPath` (our name; not reflected, so [`THUMBNAIL_PATH`]), puts it in
//! an `FBrickThumbnailRequest` (reflected: `Path`, `bIsURL`, a size as an `FIntPoint`,
//! `RoundedEdgeRadius`, `DropShadowOffset`, the rest left at the constructor's defaults) and calls
//! `UBrickImage::SetBrushFromThumbnail(Request, false)` (reflected), which loads it in the
//! background. We do the same for any image and any vehicle row. See `docs/vehicle-menu.md`,
//! "Folder thumbnails".

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::{UClass, UStruct};

use crate::framework::sig::FunctionSig;
use crate::game::reflection::{self, BoolField, Parameters};
use crate::game::slider_text::FString;
use crate::game::ugc_item::{InitializeStructFn, struct_initializer};

/// `FString FUGCFileInfo::GetThumbnailPath(bool& bIsURL) const` (BRMK 0x326020, game 1.11.8 RVA
/// 0xE6C410): by the info's origin byte (+8), the local file of a saved vehicle, a Workshop item's
/// preview URL, or nothing. Its start, past `push rbx; sub rsp, 0x20; movzx r9d, byte [rcx+8]`.
pub const THUMBNAIL_PATH: FunctionSig = FunctionSig {
    name: "FUGCFileInfo::GetThumbnailPath",
    pattern: "0f b6 49 08 48 8b da 41 83 e9 01 74 ?? 41 83 e9 01 74 ?? 41 83 e9 01 74 ?? 41 83 e9 01 74 ?? 41 83 f9 01 74",
};

/// `info`, the hidden return value's address and the out flag (the game's `bool`, a byte).
type ThumbnailPathFn =
    unsafe extern "C" fn(info: *const u8, out: *mut FString, is_url: *mut u8) -> *mut FString;

/// Where a vehicle's thumbnail is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thumbnail {
    pub path: String,
    pub is_url: bool,
}

/// How a thumbnail is drawn: its size in pixels and the row widget's look.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub size: [i32; 2],
    pub rounded_edge_radius: f32,
    pub shadow_offset: [f32; 2],
}

/// The reflected parts of a thumbnail request (after engine init).
pub struct Thumbnails {
    path_of: ThumbnailPathFn,
    /// `UBrickImage::SetBrushFromThumbnail`, and its request parameter's offset (the other one, a
    /// bool, stays false as the game passes it).
    set_brush: *const brickrust::ue::uclass::UFunction,
    request_at: usize,
    request: *const UStruct,
    initialize: InitializeStructFn,
    path: usize,
    is_url: BoolField,
    size: usize,
    radius: usize,
    shadow: usize,
}

// SAFETY: reflection records and game code living as long as the game; only used on the game
// thread.
unsafe impl Send for Thumbnails {}
unsafe impl Sync for Thumbnails {}

impl Thumbnails {
    pub fn find() -> Result<Self, String> {
        let brick_image: *const UClass =
            reflection::find_class("BrickImage").ok_or("class BrickImage not found")?;
        let set_brush = reflection::find_function(brick_image, "SetBrushFromThumbnail")?;
        // SAFETY: a live function record.
        if !unsafe { Parameters::fit(set_brush) } {
            return Err("SetBrushFromThumbnail: too many parameters".into());
        }
        // SAFETY: a live function record; its parameter records live as long as it.
        let (request_at, request) = unsafe {
            reflection::properties(set_brush.cast())
                .find(|&parameter| reflection::is_kind(parameter, "StructProperty"))
                .map(|parameter| {
                    (
                        (*parameter).offset_internal as usize,
                        reflection::struct_type(parameter),
                    )
                })
                .filter(|(_, request)| !request.is_null())
                .ok_or("SetBrushFromThumbnail has no request parameter")?
        };
        let field = |name, kind, size| reflection::struct_field_offset(request, name, kind, size);
        let found = || -> Result<Self, String> {
            let string = size_of::<FString>() as i32;
            let found = THUMBNAIL_PATH.locate()?;
            Ok(Self {
                // SAFETY: the function the pattern is in has this signature (see the constant).
                path_of: unsafe {
                    core::mem::transmute::<*const u8, ThumbnailPathFn>(found.function)
                },
                set_brush,
                request_at,
                request,
                initialize: struct_initializer(request)?,
                path: field("Path", "StrProperty", string)?,
                is_url: reflection::bool_field(request, "bIsURL")?,
                size: size_field(request)?,
                radius: field("RoundedEdgeRadius", "FloatProperty", 4)?,
                shadow: field("DropShadowOffset", "StructProperty", 8)?,
            })
        };
        found().map_err(|reason| format!("thumbnail request: {reason}"))
    }

    /// The thumbnail of a vehicle row (`FUGCQueryResult`, which starts with its `FUGCFileInfo`);
    /// `None` if it has none.
    ///
    /// # Safety
    /// `row` must be a live row. Game thread only.
    pub unsafe fn of_row(&self, row: *const u8) -> Option<Thumbnail> {
        let mut out = FString::allocated("");
        let mut is_url = 0u8;
        // SAFETY: per the contract; the result is ours to free.
        let path = unsafe {
            (self.path_of)(row, &mut out, &mut is_url);
            let path = out.to_string();
            out.free();
            path
        };
        (!path.is_empty()).then_some(Thumbnail {
            path,
            is_url: is_url != 0,
        })
    }

    /// Asks `image` (a `UBrickImage`) to show `thumbnail`, loaded in the background.
    ///
    /// # Safety
    /// `image` must be a live `UBrickImage`. Game thread only.
    pub unsafe fn show(&self, image: *mut UObjectBase, thumbnail: &Thumbnail, style: Style) {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract. The request is constructed as the game constructs it, at its
        // reflected offset in the call's parameters; its fields written at their reflected
        // offsets. The function copies the request; the path is freed after (the rest holds
        // nothing allocated: default soft paths and plain values).
        unsafe {
            let request = parameters.as_mut_ptr().add(self.request_at);
            (self.initialize)(self.request, request, 1);
            let path = request.add(self.path).cast::<FString>();
            path.replace(FString::allocated(&thumbnail.path)).free();
            self.is_url.set(request, thumbnail.is_url);
            request
                .add(self.size)
                .cast::<[i32; 2]>()
                .write_unaligned(style.size);
            request
                .add(self.radius)
                .cast::<f32>()
                .write_unaligned(style.rounded_edge_radius);
            request
                .add(self.shadow)
                .cast::<[f32; 2]>()
                .write_unaligned(style.shadow_offset);
            reflection::call(image, self.set_brush, parameters.as_mut_ptr());
            path.read().free();
        }
    }
}

/// The request's size: its one `IntPoint` field (its name isn't in BRMK's strings).
fn size_field(request: *const UStruct) -> Result<usize, String> {
    // SAFETY: a live struct record; its property records live as long as it.
    unsafe {
        let mut sizes = reflection::properties(request).filter(|&field| {
            reflection::is_kind(field, "StructProperty") && (*field).element_size == 8 && {
                let inner = reflection::struct_type(field);
                !inner.is_null() && reflection::name_is(reflection::struct_name(inner), "IntPoint")
            }
        });
        match (sizes.next(), sizes.next()) {
            (Some(field), None) => Ok((*field).offset_internal as usize),
            _ => Err("no single IntPoint field for its size".into()),
        }
    }
}
