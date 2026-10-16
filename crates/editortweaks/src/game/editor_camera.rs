//! The vehicle editor's camera: where it is (`SetCameraRelativeLocation`), how the editor makes its
//! view (`ABrickEditor::CalcCamera`: its place, its zoom and the gizmo's size), its zoom parameters
//! and its gizmo. Found through BRMK 1.11.6's exports, then in game 1.11.7 (build 25831956); see
//! `docs/re/editor-camera.md`.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UClass;

use crate::framework::sig::FunctionSig;
use crate::game::editor_object::Vector;
use crate::game::{reflection, view_target};

/// `void ABrickEditor::CalcCamera(float DeltaTime, FMinimalViewInfo& OutResult)` (BRMK 0x8C1E0,
/// game RVA 0xCD0AB0): the editor's view, every frame it's the view target. After the camera
/// manager reset `OutResult` (its `FOV` the player's):
///
/// ```text
/// GetCameraViewPoint(OutResult.Location, OutResult.Rotation);   // the actor's place + the camera's
/// UpdateZoom(DeltaTime);      // the zoom ratio glides to the one the zoom keys set
/// OutResult.FOV *= Lerp(ZoomRange.Max, ZoomRange.Min, powf(ZoomRatio, Exponent));
/// ...                         // a post-process material
/// FVector2D FOV = CameraManager->GetEffectiveFOV(OutResult.FOV);   // the LAST frame's, from its
///                                                                   // projection matrix
/// Gizmo->SetRelativeScale3D(Distance(OutResult.Location, Gizmo) * tanf(FOV.Y / 2) * GizmoFactor);
/// ```
///
/// `GizmoFactor` is a float of the editor's static info, not reflected (game +0x60, BRMK +0x68):
/// this pattern is only read for its offset.
pub const CALC_CAMERA: FunctionSig = FunctionSig {
    name: "ABrickEditor::CalcCamera",
    // The gizmo's size: tanf, times the distance, times the static info's factor (its offset at
    // +21), into the scale's three axes, then SetRelativeScale3D.
    pattern: "e8 ?? ?? ?? ?? 48 8d 54 24 ?? 48 8b cb f3 0f 59 f0 f3 0f 59 75 ?? f3 0f 11 74 24 ?? f3 0f 11 74 24 ?? f3 0f 11 74 24 ?? e8",
};

/// `void ABrickEditor::SetCameraRelativeLocation(const FVector&)` (BRMK 0xE6680, game RVA
/// 0xD06630): moves the editor's camera, relative to the editor (its view is the editor's place
/// plus this). Keeps it inside the editor's bounds (`ConstraintLocationToEditorBounds`, 100 cm
/// padding), stores it (game +0x3F0, BRMK +0x4C8; `GetCameraRelativeLocation` reads it there),
/// then fades the bounds' box in by the camera's distance to it.
pub const SET_CAMERA_LOCATION: FunctionSig = FunctionSig {
    name: "ABrickEditor::SetCameraRelativeLocation",
    // After the bounds constraint: the location's three floats loaded, then stored in the editor
    // (Y's offset at +42, X's at +53).
    pattern: "48 8b f0 e8 ?? ?? ?? ?? 66 0f 6f 1d ?? ?? ?? ?? f3 0f 10 0b f3 0f 10 43 04 f3 0f 10 53 08 f3 0f 10 35 ?? ?? ?? ?? f3 0f 11 87 ?? ?? 00 00 0f 54 c3 f3 0f 11 8f ?? ?? 00 00",
};

/// `void ABrickEditor::GetCameraViewPoint(FVector& Location, FRotator& Rotation) const` (BRMK
/// 0xA9FE0, game 0xCE0840; slot 11 of the editor's `IViewTargetInterface`, `this` is the
/// interface): the view's place, which `CalcCamera` takes. It is
/// `RootComponent->ComponentToWorld.Translation + CameraRelativeLocation` (no rotation: the game
/// never turns the editor, see `features/editor_frame`) and the camera's rotation (`this + 0x10`,
/// in the world's space). The pattern has the root
/// component's offset (+7), the relative location's x, y and z offsets in the interface (+69, +61,
/// +49) and the rotation's offset (+91, one byte).
pub const GET_CAMERA_VIEW_POINT: FunctionSig = FunctionSig {
    name: "ABrickEditor::GetCameraViewPoint",
    pattern: "48 83 ec 38 48 8b 81 ?? ?? ?? ?? 48 85 c0 74 08 48 05 ?? ?? ?? ?? eb 07 48 8d 05 ?? ?? ?? ?? 0f 10 50 10 0f 28 ca 0f 28 c2 0f c6 c2 aa f3 0f 58 81 ?? ?? ?? ?? 0f c6 ca 55 f3 0f 58 89 ?? ?? ?? ?? f3 0f 58 91 ?? ?? ?? ?? f3 0f 11 42 08 f3 0f 11 4a 04 f3 0f 11 12 f2 0f 10 41 10 8b 41 18 f2 41 0f 11 00",
};

/// `void ABrickEditor::UpdateViewRotation(FRotator& Rotation, float DeltaTime)` (BRMK 0xFD400, game
/// 0xD13C50; slot 8 of the same interface): the base one stores the view's rotation (`this +
/// 0x10`, the pattern holds the offset), then, while the camera pivots, the camera's relative
/// location turns around the middle of the selected bricks by the rotation's change: the
/// location and the bricks' bounds are in the editor's space, the rotation (and the pitch axis
/// made from it) in the world's.
pub const UPDATE_VIEW_ROTATION: FunctionSig = FunctionSig {
    name: "ABrickEditor::UpdateViewRotation",
    pattern: "40 55 56 57 48 8d 6c 24 ?? 48 81 ec ?? ?? 00 00 f2 0f 10 41 10 48 8b f9 8b 71 18 f2 0f 11 45 ?? e8 ?? ?? ?? ?? f6 87 ?? ?? 00 00 08 0f 84 ?? ?? 00 00",
};

/// `void ABrickEditor::AddCameraMovementInput(EAxis::Type Axis, float Value)` (BRMK 0x85A40, game
/// 0xCCD690; its one caller is `UEditorInputComponent::HandleMovementAxis`, the fly keys): adds
/// `Axis` of the camera, in the editor's space, times `Value` to the input `ABrickEditor::Tick`
/// moves the camera's relative location by. The camera's rotation in the editor's space is
/// worked out as `GetCameraRelativeRotation` (BRMK 0xA9E70, otherwise inlined) does:
/// `rotation.Quaternion() * RootComponent->ComponentToWorld.Rotation.Inverse()`, the camera's
/// rotation (in the world) times the editor's inverse. Right only while the editor isn't turned
/// (the right order is the other one, see `features/editor_frame`). The pattern runs from the
/// function's start to that product (the camera's quaternion broadcast, the root's swizzled: camera
/// first), and holds the rotation's offset in the editor (+76) and its yaw and roll's (+87).
pub const ADD_CAMERA_MOVEMENT_INPUT: FunctionSig = FunctionSig {
    name: "ABrickEditor::AddCameraMovementInput",
    pattern: "48 89 5c 24 10 57 48 81 ec 80 00 00 00 0f 29 74 24 70 0f 57 c0 0f 28 f2 8b fa 0f 2e f0 48 8b d9 0f 84 ?? ?? ?? ?? 83 fa 03 75 ?? f2 0f 10 05 ?? ?? ?? ?? 8b 05 ?? ?? ?? ?? f2 0f 11 44 24 ?? 89 44 24 ?? e9 ?? ?? ?? ?? f2 0f 10 81 ?? ?? ?? ?? 48 8d 54 24 ?? 8b 81 ?? ?? ?? ?? 48 8d 4c 24 ?? f2 0f 11 44 24 ?? 89 44 24 ?? e8 ?? ?? ?? ?? 48 8b 8b ?? ?? ?? ?? 48 85 c9 74 09 48 81 c1 ?? ?? ?? ?? eb 07 48 8d 0d ?? ?? ?? ?? 0f 10 10 0f 10 19 0f 28 e2 0f 59 1d ?? ?? ?? ?? 0f 28 ca 0f c6 e2 00 0f c6 ca 55 0f 28 c3 0f c6 c3 1b 0f 59 e0",
};

/// `void UBrickEditorMoveMode::TickMode(float DeltaTime)` (BRMK 0xF19A0, game 0xD0C8F0), where it
/// works out the camera's rotation in the editor's space the same way as
/// [`ADD_CAMERA_MOVEMENT_INPUT`] (camera times the editor's inverse; game +0x103): the mouse and
/// the keys move bricks along that rotation's axes. It's the only read of the camera's rotation in
/// `TickMode`. The pattern holds the rotation's offset in the editor (+4) and its yaw and roll's
/// (+10).
pub const MOVE_MODE_CAMERA: FunctionSig = FunctionSig {
    name: "UBrickEditorMoveMode::TickMode camera rotation",
    pattern: "f2 0f 10 83 ?? ?? ?? ?? 8b 83 ?? ?? ?? ?? f2 0f 11 44 24 ?? 89 44 24 ?? e8 ?? ?? ?? ?? 48 8b 8b ?? ?? ?? ?? 48 85 c9 74 09 48 81 c1 ?? ?? ?? ?? eb 07 48 8d 0d ?? ?? ?? ?? 0f 10 10 0f 10 41 10 44 0f 28 e2 0f 10 49 20 0f 10 19 0f 59 1d ?? ?? ?? ?? 0f b6 47 ?? 8b 5f ?? 44 8b bf ?? ?? ?? ?? 0f 29 85 ?? ?? ?? ?? 44 0f c6 e2 00 0f 28 c3 0f c6 c3 1b 44 0f 59 e0",
};

type SetCameraLocationFn = unsafe extern "C" fn(editor: *mut UObjectBase, location: *const Vector);

/// What the editor's code tells: [`CALC_CAMERA`]'s gizmo factor and [`SET_CAMERA_LOCATION`].
#[derive(Clone, Copy)]
pub struct EditorCode {
    /// Offset of the gizmo's size factor in the editor's static info.
    gizmo_factor: usize,
    set_camera_location: SetCameraLocationFn,
    /// Offset of the camera's relative location in the editor.
    camera_location: usize,
}

/// Finds [`EditorCode`]. Before engine init too.
pub fn code() -> Result<EditorCode, String> {
    let calc = CALC_CAMERA.locate()?;
    let set = SET_CAMERA_LOCATION.locate()?;
    // SAFETY: displacements inside the matched patterns (see their comments).
    let (gizmo_factor, x, y) = unsafe {
        (
            usize::from(calc.address.add(21).read()),
            set.address.add(53).cast::<u32>().read_unaligned() as usize,
            set.address.add(42).cast::<u32>().read_unaligned() as usize,
        )
    };
    if y != x + 4 {
        return Err(format!(
            "{}: unexpected offsets 0x{x:X}, 0x{y:X}",
            SET_CAMERA_LOCATION.name
        ));
    }
    Ok(EditorCode {
        gizmo_factor,
        // SAFETY: the start of SetCameraRelativeLocation, which has this signature.
        set_camera_location: unsafe {
            core::mem::transmute::<*const u8, SetCameraLocationFn>(set.function)
        },
        camera_location: x,
    })
}

/// The editor's camera parts (after engine init).
pub struct EditorCamera {
    /// `BrickEditor`.
    class: *const UClass,
    code: EditorCode,
    /// Offset of `ABrickEditor::StaticInfoClass`, whose default object is the editor's static info.
    static_info_class: usize,
    /// Offset of `InterpSpeed` in the static info (in its `ZoomParams`).
    zoom_glide: usize,
    /// Offset of `ABrickEditor::GizmoMeshComponent`.
    gizmo: usize,
    /// Offset of `ABrickEditor::PlayerController`.
    player_controller: usize,
    /// Offset of `AActor::InputComponent`, the editor's `UEditorInputComponent`.
    input: usize,
    /// Offset of the editor's `IViewTargetInterface`, if found.
    view_target: Option<usize>,
}

// SAFETY: reflection records and game code living as long as the game; only used on the game
// thread.
unsafe impl Send for EditorCamera {}
unsafe impl Sync for EditorCamera {}

/// Finds the editor's camera parts. After engine init.
pub fn find(code: EditorCode) -> Result<EditorCamera, String> {
    let class = reflection::find_class("BrickEditor").ok_or("class BrickEditor not found")?;
    let info = reflection::find_class("BrickEditorStaticInfo")
        .ok_or("class BrickEditorStaticInfo not found")?;
    let zoom = reflection::field(info, "ZoomParams", "StructProperty", 0x18)?;
    // SAFETY: a live StructProperty record, so its struct is live and linked.
    let (params, zoom) = unsafe {
        (
            reflection::struct_type(zoom),
            (*zoom).offset_internal as usize,
        )
    };
    let glide = reflection::struct_field_offset(params, "InterpSpeed", "FloatProperty", 4)
        .map_err(|reason| format!("ZoomParams: {reason}"))?;
    let field = |name, kind, size| reflection::field_offset(class, name, kind, size);
    Ok(EditorCamera {
        class,
        code,
        static_info_class: field("StaticInfoClass", "ClassProperty", 8)?,
        zoom_glide: zoom + glide,
        gizmo: field("GizmoMeshComponent", "ObjectProperty", 8)?,
        player_controller: field("PlayerController", "ObjectProperty", 8)?,
        input: field("InputComponent", "ObjectProperty", 8)?,
        // SAFETY: a live class.
        view_target: unsafe { reflection::interface_offset(class.cast(), "ViewTargetInterface") },
    })
}

impl EditorCamera {
    /// Whether `object` is an editor.
    ///
    /// # Safety
    /// `object` must be a live object.
    pub unsafe fn is_editor(&self, object: *const UObjectBase) -> bool {
        // SAFETY: per the contract.
        unsafe { reflection::with_parents(reflection::class_of(object.cast())) }
            .any(|class| class == self.class.cast())
    }

    /// The editor's static info: the default object of its `StaticInfoClass`, what
    /// `ABrickEditor::GetStaticInfo` returns.
    ///
    /// # Safety
    /// `editor` must be a live `ABrickEditor`. Game thread only.
    unsafe fn static_info(&self, editor: *const UObjectBase) -> Option<*const u8> {
        // SAFETY: per the contract; the offset comes from reflection, and a class property holds a
        // live class or null.
        unsafe {
            let class = read::<*const UClass>(editor.cast(), self.static_info_class);
            if class.is_null() {
                return None;
            }
            reflection::default_object(class).map(|info| info.cast_const().cast())
        }
    }

    /// How fast the editor's zoom glides to where its steps take it (`InterpSpeed` of its zoom
    /// params, `FInterpTo`'s speed; 0: at once).
    ///
    /// # Safety
    /// As for [`Self::static_info`].
    pub unsafe fn zoom_glide_speed(&self, editor: *const UObjectBase) -> Option<f32> {
        // SAFETY: per the contract; the offset comes from reflection.
        unsafe { Some(read(self.static_info(editor)?, self.zoom_glide)) }
    }

    /// Where the camera's relative location is in the editor, from [`SET_CAMERA_LOCATION`].
    pub fn camera_location(&self) -> usize {
        self.code.camera_location
    }

    /// Where the editor's `IViewTargetInterface` is in it, if found.
    pub fn view_target_offset(&self) -> Option<usize> {
        self.view_target
    }

    /// The address of the editor's `IViewTargetInterface` (what the camera functions are called
    /// on), if found.
    ///
    /// # Safety
    /// `editor` must be a live editor.
    pub unsafe fn view_target_interface(&self, editor: *const UObjectBase) -> Option<usize> {
        // SAFETY: per the contract.
        Some(unsafe { editor.cast::<u8>().add(self.view_target?) } as usize)
    }

    /// The editor's zoom ratio: 0 (unzoomed) to 1 (see `view_target::zoom_ratio`).
    ///
    /// # Safety
    /// As for [`Self::static_info`].
    pub unsafe fn zoom_ratio(&self, editor: *mut UObjectBase) -> Option<f32> {
        // SAFETY: per the contract; the interface's offset in the editor comes from its class.
        unsafe { view_target::zoom_ratio(editor.cast::<u8>().add(self.view_target?).cast()) }
    }

    /// How big the gizmo is for the height of the view at its place: its scale over half that
    /// height (see [`CALC_CAMERA`]).
    ///
    /// # Safety
    /// As for [`Self::static_info`].
    pub unsafe fn gizmo_factor(&self, editor: *const UObjectBase) -> Option<f32> {
        // SAFETY: per the contract; the offset comes from the game's own code.
        let factor: f32 = unsafe { read(self.static_info(editor)?, self.code.gizmo_factor) };
        (factor.is_finite() && factor > 0.0).then_some(factor)
    }

    /// The editor's gizmo (a static mesh component), if it has one.
    ///
    /// # Safety
    /// As for [`Self::static_info`].
    pub unsafe fn gizmo(&self, editor: *const UObjectBase) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; the offset comes from reflection.
        let gizmo: *mut UObjectBase = unsafe { read(editor.cast(), self.gizmo) };
        (!gizmo.is_null()).then_some(gizmo)
    }

    /// The editor's player controller, if it has one.
    ///
    /// # Safety
    /// As for [`Self::static_info`].
    pub unsafe fn player_controller(&self, editor: *const UObjectBase) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; the offset comes from reflection.
        let controller: *mut UObjectBase = unsafe { read(editor.cast(), self.player_controller) };
        (!controller.is_null()).then_some(controller)
    }

    /// The editor's input component, if it has one.
    ///
    /// # Safety
    /// As for [`Self::static_info`].
    pub unsafe fn input(&self, editor: *const UObjectBase) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; the offset comes from reflection.
        let input: *mut UObjectBase = unsafe { read(editor.cast(), self.input) };
        (!input.is_null()).then_some(input)
    }

    /// Moves the editor's camera by `offset` cm, as far as the game lets it (it keeps the camera
    /// inside the editor's bounds): returns how far it moved.
    ///
    /// # Safety
    /// As for [`Self::static_info`].
    pub unsafe fn move_camera(&self, editor: *mut UObjectBase, offset: Vector) -> Vector {
        // SAFETY: per the contract; the offset and function come from the game's own code.
        unsafe {
            let before: Vector = read(editor.cast(), self.code.camera_location);
            let wanted = [0, 1, 2].map(|axis| before[axis] + offset[axis]);
            (self.code.set_camera_location)(editor, &wanted);
            let after: Vector = read(editor.cast(), self.code.camera_location);
            [0, 1, 2].map(|axis| after[axis] - before[axis])
        }
    }
}

/// The `T` at `offset` in `base`.
///
/// # Safety
/// `base` must point to something holding a valid `T` at `offset`.
unsafe fn read<T: Copy>(base: *const u8, offset: usize) -> T {
    // SAFETY: per the contract.
    unsafe { base.add(offset).cast::<T>().read_unaligned() }
}
