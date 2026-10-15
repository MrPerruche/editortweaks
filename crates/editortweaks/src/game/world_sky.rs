//! The world's sky (`AWorldSetupActor`, BRMK 1.11.6; see `docs/re/editor-camera.md`): a sphere mesh
//! (`SkyMeshComponent`: `SM_SkySphere`, scaled by `SkyMeshScale`, 500 in the C++ constructor) with
//! an unlit sky material (`bIsSky`; the editor's: `EditorSkyMaterial`). The actor moves it to each
//! view's place before the view is drawn (`OnBeginPostProcessSettings`, bound to the world's
//! delegate), and its `BoundsScale` is 10 000 000, so it's never culled: the view's far plane must
//! reach it.

use brickrust::ue::coreuobject::{EObjectFlags, GObjects, UObjectBase};
use brickrust::ue::uclass::UClass;

use crate::game::reflection::{self, Getter};

pub struct WorldSky {
    /// `WorldSetupActor`.
    class: *const UClass,
    /// Offset of `AWorldSetupActor::SkyMeshComponent`.
    sky: usize,
    /// Offset of `UStaticMeshComponent::StaticMesh`.
    mesh: usize,
    /// `FBoxSphereBounds UStaticMesh::GetBounds() const`: origin, box extent, sphere radius.
    bounds: Getter<[f32; 7]>,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for WorldSky {}
unsafe impl Sync for WorldSky {}

/// Finds the sky's fields. After engine init.
pub fn find() -> Result<WorldSky, String> {
    let class =
        reflection::find_class("WorldSetupActor").ok_or("class WorldSetupActor not found")?;
    let component = reflection::find_class("StaticMeshComponent")
        .ok_or("class StaticMeshComponent not found")?;
    let mesh = reflection::find_class("StaticMesh").ok_or("class StaticMesh not found")?;
    Ok(WorldSky {
        class,
        sky: reflection::field_offset(class, "SkyMeshComponent", "ObjectProperty", 8)?,
        mesh: reflection::field_offset(component, "StaticMesh", "ObjectProperty", 8)?,
        bounds: Getter::find(mesh, "GetBounds", "StructProperty")?,
    })
}

impl WorldSky {
    /// The sky sphere (a static mesh component) of a live world setup actor. Walks every live
    /// object, so keep the result. Game thread only.
    pub fn find_sky(&self) -> Option<*mut UObjectBase> {
        // SAFETY: on the game thread, GObjects slots hold live objects or null, and every object
        // has a valid class; a world setup actor holds its sky component or null.
        unsafe {
            GObjects().iter().find_map(|object| {
                if object.is_null()
                    || (*object)
                        .object_flags
                        .contains(EObjectFlags::RF_ClassDefaultObject)
                    || !reflection::with_parents(reflection::class_of(object.cast()))
                        .any(|class| class == self.class.cast())
                {
                    return None;
                }
                let sky = object
                    .cast::<u8>()
                    .add(self.sky)
                    .cast::<*mut UObjectBase>()
                    .read();
                (!sky.is_null()).then_some(sky)
            })
        }
    }

    /// How far the sky's mesh reaches from its origin, unscaled (cm).
    ///
    /// # Safety
    /// `sky` must be a live sky component (from [`Self::find_sky`]). Game thread only.
    pub unsafe fn mesh_radius(&self, sky: *const UObjectBase) -> Option<f32> {
        // SAFETY: per the contract; the offset comes from reflection, and the function returns an
        // FBoxSphereBounds.
        unsafe {
            let mesh = sky
                .cast::<u8>()
                .add(self.mesh)
                .cast::<*mut UObjectBase>()
                .read();
            if mesh.is_null() {
                return None;
            }
            let [x, y, z, extent_x, extent_y, extent_z, _] = self.bounds.call(mesh);
            let radius = (x * x + y * y + z * z).sqrt() + extent_x.max(extent_y).max(extent_z);
            (radius.is_finite() && radius > 0.0).then_some(radius)
        }
    }
}
