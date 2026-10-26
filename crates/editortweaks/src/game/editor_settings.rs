//! The vehicle editor's settings in `UBrickUserSettings`, which the editor's side panel shows with
//! nothing selected ("Editor settings", see `docs/re/settings-menu.md`): automatic mirroring and its
//! axis, and the editor's world (`EditorWorldSetupParams`: time of day, sun, atmosphere). All
//! through reflection: the settings object from the reflected `GetUserSettings()`, the fields by
//! name.
//!
//! Only the values: writing one doesn't tell the game. The panel does that after a row of it
//! changes (an event naming the row's property, see `features::compact_panel`).

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UStruct;

use crate::game::reflection::{self, BoolField, Getter};

/// A float of the editor's world (`FWorldSetupParams`).
#[derive(Clone, Copy, Debug)]
pub enum World {
    /// Hours, 0 to 24.
    TimeOfDay,
    /// The sun's height above the horizon, 0° to 90° (the game's `SunAngle`).
    SunAngle,
    /// The sun's direction, -180° to 180° (`SunYaw`).
    SunYaw,
    /// 0 to 1, shown as a percentage (`AtmosphereDensity`).
    AtmosphereDensity,
}

pub struct EditorSettings {
    /// `UBrickUserSettings`' default object, to call its static functions on.
    defaults: *mut UObjectBase,
    /// `static UBrickUserSettings* UBrickUserSettings::GetUserSettings()`.
    settings: Getter<*mut UObjectBase>,
    /// `bEditorMirrorAxisEnabled`.
    mirror_enabled: BoolField,
    /// Offset of `EditorMirrorAxis` (`EAxis`: X 1, Y 2, Z 3).
    mirror_axis: usize,
    /// Offset of `EditorWorldSetupParams` (an `FWorldSetupParams`).
    world: usize,
    /// Offsets of the floats of [`World`] in it, in its order.
    world_fields: [usize; 4],
}

// SAFETY: reflection records and an object living as long as the game; only used on the game
// thread.
unsafe impl Send for EditorSettings {}
unsafe impl Sync for EditorSettings {}

/// Finds the settings' fields. After engine init.
pub fn find() -> Result<EditorSettings, String> {
    let class =
        reflection::find_class("BrickUserSettings").ok_or("class BrickUserSettings not found")?;
    // SAFETY: a live class from reflection.
    let settings: *const UStruct = unsafe { &raw const (*class).ustruct };
    // SAFETY: a live class; the property record, if found, lives as long as it.
    let (world_offset, world_struct) = unsafe {
        let world = reflection::find_property(settings, "EditorWorldSetupParams");
        if world.is_null() {
            return Err("field 'EditorWorldSetupParams' not found".into());
        }
        (
            (*world).offset_internal as usize,
            reflection::struct_type(world),
        )
    };
    if world_struct.is_null() {
        return Err("field 'EditorWorldSetupParams' isn't a struct".into());
    }
    let float = |name| reflection::struct_field_offset(world_struct, name, "FloatProperty", 4);
    Ok(EditorSettings {
        defaults: reflection::find_object("Default__BrickUserSettings", "BrickUserSettings")
            .ok_or("BrickUserSettings' default object not found")?,
        settings: Getter::find(class, "GetUserSettings", "ObjectProperty")?,
        mirror_enabled: reflection::bool_field(settings, "bEditorMirrorAxisEnabled")?,
        mirror_axis: reflection::field_offset(class, "EditorMirrorAxis", "ByteProperty", 1)?,
        world: world_offset,
        world_fields: [
            float("TimeOfDay")?,
            float("SunAngle")?,
            float("SunYaw")?,
            float("AtmosphereDensity")?,
        ],
    })
}

impl EditorSettings {
    /// The settings object, if there is one.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn object(&self) -> Option<*mut u8> {
        // SAFETY: per the contract; a static function, called on the default object.
        let settings = unsafe { self.settings.call(self.defaults) };
        (!settings.is_null()).then_some(settings.cast())
    }

    /// The mirror axis (X 1, Y 2, Z 3) if automatic mirroring is on, else `None`.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn mirror(&self) -> Option<u8> {
        // SAFETY: per the contract; the settings are a live UBrickUserSettings, and the fields'
        // places come from reflection.
        unsafe {
            let settings = self.object()?;
            if !self.mirror_enabled.get(settings) {
                return None;
            }
            Some(settings.add(self.mirror_axis).read().clamp(1, 3))
        }
    }

    /// Turns automatic mirroring off (`None`) or on along `axis` (X 1, Y 2, Z 3). Off keeps the
    /// axis, as the game's own row does.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn set_mirror(&self, axis: Option<u8>) {
        // SAFETY: as for `mirror`.
        unsafe {
            let Some(settings) = self.object() else {
                return;
            };
            self.mirror_enabled.set(settings, axis.is_some());
            if let Some(axis) = axis {
                settings.add(self.mirror_axis).write(axis.clamp(1, 3));
            }
        }
    }

    /// A float of the editor's world.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn world(&self, value: World) -> f32 {
        // SAFETY: per the contract; the struct's float at its reflected offset.
        unsafe {
            self.object()
                .map_or(0.0, |settings| self.world_float(settings, value).read())
        }
    }

    /// Sets a float of the editor's world.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn set_world(&self, value: World, to: f32) {
        // SAFETY: as for `world`.
        unsafe {
            if let Some(settings) = self.object() {
                self.world_float(settings, value).write(to);
            }
        }
    }

    /// # Safety
    /// `settings` must be the live settings object.
    unsafe fn world_float(&self, settings: *mut u8, value: World) -> *mut f32 {
        // SAFETY: per the contract; the struct lies within the settings, the float within it.
        unsafe {
            settings
                .add(self.world + self.world_fields[value as usize])
                .cast::<f32>()
        }
    }
}
