//! The triangles of bricks' models. The shipping game keeps a static mesh's render triangles on the
//! GPU only (no brick mesh allows CPU access), but bricks' meshes also have complex collision: a
//! PhysX triangle mesh cooked from the same triangles (welded), which the game keeps for precise
//! traces. Found through BRMK 1.11.6 and the game's PhysX DLLs; see `docs/re/brick-models.md`.
//!
//! brick -> `UBrickEditorObject::GetRootComponent` (its `UStaticMeshComponent`) -> `StaticMesh`
//! -> `BodySetup` (a brick's body setup is its mesh's, `UBrick::GetBodySetup`) -> `TriMeshes`
//! (`TArray<physx::PxTriangleMesh*>`, not reflected) -> PhysX's virtual functions.

use core::ffi::{CStr, c_char, c_void};

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::tarray::TArray;
use brickrust::ue::uclass::UStruct;

use crate::framework::sig::Module;
use crate::game::reflection::{self, Getter};
use crate::math::Vec3;

/// The PhysX module with the triangle meshes' code (`Gu::TriangleMesh`).
const PHYSX_COMMON: &str = "PhysX3Common_x64.dll";

/// Virtual functions of `physx::PxTriangleMesh` (PhysX 3.4), as offsets in its vtable: after
/// `PxBase`'s `release`, `getConcreteTypeName`, `isReleasable`, destructor and `isKindOf`.
mod slot {
    pub const TYPE_NAME: usize = 0x08;
    pub const VERTEX_COUNT: usize = 0x28;
    pub const VERTICES: usize = 0x30;
    pub const TRIANGLE_COUNT: usize = 0x48;
    pub const TRIANGLES: usize = 0x50;
    /// `getTriangleMeshFlags`: returns its one-byte flags through a hidden pointer (MSVC member
    /// functions returning a class).
    pub const FLAGS: usize = 0x58;
}

/// `PxTriangleMeshFlag::e16_BIT_INDICES`: triangles use 16-bit vertex indices (else 32-bit).
const SIXTEEN_BIT_INDICES: u8 = 1 << 1;

/// What `getConcreteTypeName` returns for triangle meshes (by their midphase structure).
const TYPE_NAMES: [&str; 2] = ["PxBVH33TriangleMesh", "PxBVH34TriangleMesh"];

/// Sanity limits: brick meshes have far fewer.
const MAX_MESHES: i32 = 64;
const MAX_COUNT: u32 = 4_000_000;

type CountFn = unsafe extern "C" fn(mesh: *const c_void) -> u32;
type PointerFn = unsafe extern "C" fn(mesh: *const c_void) -> *const c_void;
type NameFn = unsafe extern "C" fn(mesh: *const c_void) -> *const c_char;
type FlagsFn = unsafe extern "C" fn(mesh: *const c_void, flags: *mut u8) -> *mut u8;

/// A mesh's triangles, in its own space (unscaled).
pub struct Triangles {
    pub vertices: Vec<[f32; 3]>,
    /// Three vertex indices each.
    pub triangles: Vec<[u32; 3]>,
}

/// Reads bricks' meshes and their triangles (after engine init).
pub struct BrickModels {
    /// `UPrimitiveComponent* UBrickEditorObject::GetRootComponent()`.
    root: Getter<*mut UObjectBase>,
    /// `StaticMeshComponent`.
    mesh_component: *const UStruct,
    /// Offsets of `UStaticMeshComponent::StaticMesh` and `UStaticMesh::BodySetup`.
    static_mesh: usize,
    body_setup: usize,
    /// Offset of `TriMeshes`: the last field of `UBodySetupCore`, right before `UBodySetup`'s
    /// first, the reflected `AggGeom`.
    tri_meshes: usize,
    physx: Module,
}

// SAFETY: reflection records and module headers living as long as the game; only used on the game
// thread.
unsafe impl Send for BrickModels {}
unsafe impl Sync for BrickModels {}

impl BrickModels {
    pub fn find() -> Result<Self, String> {
        let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
        let mesh_component = class("StaticMeshComponent")?;
        let body_setup = class("BodySetup")?;
        // `UBodySetupCore` ends with `TriMeshes` (a `TArray`, 0x10 bytes) after its reflected
        // fields, and `UBodySetup` starts with `AggGeom`: `AggGeom` must start where
        // `UBodySetupCore` ends, with room for the array after its last reflected field.
        let tri_meshes = {
            let core = class("BodySetupCore")?.cast::<UStruct>();
            // SAFETY: a class found through reflection; its property records live as long as it.
            let (core_size, fields_end) = unsafe {
                let fields_end = reflection::properties(core)
                    .map(|property| {
                        let size = (*property).element_size * (*property).array_dim;
                        (*property).offset_internal as usize + usize::try_from(size).unwrap_or(0)
                    })
                    .max()
                    .unwrap_or(0);
                (reflection::struct_size(core), fields_end)
            };
            // SAFETY: as above.
            let geometry = unsafe {
                let property = reflection::find_property(body_setup.cast(), "AggGeom");
                if property.is_null() || !reflection::is_kind(property, "StructProperty") {
                    return Err("BodySetup: field 'AggGeom' not found".into());
                }
                (*property).offset_internal as usize
            };
            let offset = geometry.checked_sub(size_of::<TArray<*mut c_void>>());
            match offset {
                Some(offset) if geometry == core_size && offset >= fields_end => offset,
                _ => {
                    return Err(format!(
                        "BodySetup's triangle meshes not where expected (AggGeom at \
                         {geometry:#x}, BodySetupCore is {core_size:#x} bytes with fields up \
                         to {fields_end:#x})"
                    ));
                }
            }
        };
        Ok(Self {
            root: Getter::find(
                class("BrickEditorObject")?,
                "GetRootComponent",
                "ObjectProperty",
            )?,
            mesh_component: mesh_component.cast(),
            static_mesh: reflection::field_offset(
                mesh_component,
                "StaticMesh",
                "ObjectProperty",
                8,
            )?,
            body_setup: reflection::field_offset(
                class("StaticMesh")?,
                "BodySetup",
                "ObjectProperty",
                8,
            )?,
            tri_meshes,
            physx: Module::find(PHYSX_COMMON).ok_or(format!("{PHYSX_COMMON} not loaded"))?,
        })
    }

    /// The static mesh `brick` shows (its root component's), if any.
    ///
    /// # Safety
    /// `brick` must be a live editor object. Game thread only.
    pub unsafe fn mesh_of(&self, brick: *mut UObjectBase) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; the root component is a live object if not null.
        unsafe { self.mesh_of_component(self.root.call(brick)) }
    }

    /// The static mesh `component` (a brick's root component, or null) shows, if any.
    ///
    /// # Safety
    /// `component` must be null or a live object. Game thread only.
    pub unsafe fn mesh_of_component(
        &self,
        component: *mut UObjectBase,
    ) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; the offset comes from reflection (checked to be a static mesh
        // component's).
        unsafe {
            if component.is_null()
                || !reflection::with_parents(reflection::class_of(component.cast()))
                    .any(|class| class == self.mesh_component)
            {
                return None;
            }
            let mesh = read::<*mut UObjectBase>(component.cast(), self.static_mesh);
            (!mesh.is_null()).then_some(mesh)
        }
    }

    /// The triangles of `mesh`'s complex collision, or why there are none.
    ///
    /// # Safety
    /// `mesh` must be a live static mesh. Game thread only (PhysX meshes are made and released
    /// there).
    pub unsafe fn triangles(&self, mesh: *mut UObjectBase) -> Result<Triangles, String> {
        // SAFETY: per the contract; the offset comes from reflection (checked above), and the body
        // setup is live if not null.
        unsafe {
            let body_setup = read::<*mut UObjectBase>(mesh.cast(), self.body_setup);
            if body_setup.is_null() {
                return Err("no body setup".into());
            }
            self.triangles_of_body_setup(body_setup)
        }
    }

    /// The triangles of `body_setup`'s complex collision, or why there are none. Bricks whose mesh
    /// is made at run time (spinners) have a body setup of their own rather than their mesh's
    /// (`Collision::body_setup_of`).
    ///
    /// # Safety
    /// `body_setup` must be a live body setup. Game thread only (PhysX meshes are made and
    /// released there).
    pub unsafe fn triangles_of_body_setup(
        &self,
        body_setup: *mut UObjectBase,
    ) -> Result<Triangles, String> {
        // SAFETY: per the contract; the offset comes from reflection (checked in `find`).
        let meshes = unsafe { read::<TArray<*const c_void>>(body_setup.cast(), self.tri_meshes) };
        if meshes.num <= 0 {
            return Err("no complex collision".into());
        }
        if meshes.num > MAX_MESHES || meshes.num > meshes.max || meshes.data.is_null() {
            return Err(format!("unexpected triangle meshes ({})", meshes.num));
        }
        let mut triangles = Triangles {
            vertices: Vec::new(),
            triangles: Vec::new(),
        };
        for index in 0..meshes.num as usize {
            // SAFETY: the array holds `num` triangle mesh pointers.
            let physx_mesh = unsafe { meshes.data.add(index).read() };
            // SAFETY: an element of the body setup's live triangle meshes.
            unsafe { self.append(physx_mesh, &mut triangles)? };
        }
        Ok(triangles)
    }

    /// Adds the triangles of a PhysX triangle mesh to `triangles`.
    ///
    /// # Safety
    /// `mesh` must be null or a live `PxTriangleMesh`. Game thread only.
    unsafe fn append(&self, mesh: *const c_void, triangles: &mut Triangles) -> Result<(), String> {
        if mesh.is_null() {
            return Err("null triangle mesh".into());
        }
        // SAFETY: per the contract; every function is checked to be PhysX code before it's called,
        // and the type name to be a triangle mesh's before the others are; arrays are read within
        // the counts the mesh reports.
        unsafe {
            let vtable = mesh.cast::<*const *const u8>().read();
            let function = |offset: usize| {
                let pointer = vtable.byte_add(offset).read();
                self.physx
                    .is_code(pointer)
                    .then_some(pointer)
                    .ok_or("triangle mesh functions not found")
            };
            let name = core::mem::transmute::<*const u8, NameFn>(function(slot::TYPE_NAME)?)(mesh);
            let name = CStr::from_ptr(name).to_str().unwrap_or("");
            if !TYPE_NAMES.contains(&name) {
                return Err(format!("not a triangle mesh: {name}"));
            }
            let count = |offset| -> Result<u32, String> {
                let count = core::mem::transmute::<*const u8, CountFn>(function(offset)?)(mesh);
                (count <= MAX_COUNT)
                    .then_some(count)
                    .ok_or(format!("unexpected count {count}"))
            };
            let pointer = |offset| -> Result<*const c_void, String> {
                let pointer = core::mem::transmute::<*const u8, PointerFn>(function(offset)?)(mesh);
                (!pointer.is_null())
                    .then_some(pointer)
                    .ok_or("null triangle data".into())
            };
            let vertex_count = count(slot::VERTEX_COUNT)?;
            let triangle_count = count(slot::TRIANGLE_COUNT)? as usize;
            let vertices = pointer(slot::VERTICES)?.cast::<[f32; 3]>();
            let indices = pointer(slot::TRIANGLES)?;
            let mut flags = 0u8;
            core::mem::transmute::<*const u8, FlagsFn>(function(slot::FLAGS)?)(mesh, &mut flags);

            let first = triangles.vertices.len() as u32;
            let index_count = triangle_count * 3;
            let read_index = |at: usize| -> u32 {
                if flags & SIXTEEN_BIT_INDICES != 0 {
                    u32::from(indices.cast::<u16>().add(at).read())
                } else {
                    indices.cast::<u32>().add(at).read()
                }
            };
            let mut added = Vec::with_capacity(triangle_count);
            for triangle in 0..triangle_count {
                let corners: [u32; 3] =
                    core::array::from_fn(|corner| read_index(triangle * 3 + corner));
                if corners.iter().any(|&corner| corner >= vertex_count) {
                    return Err(format!(
                        "vertex index out of range ({} indices, {vertex_count} vertices)",
                        index_count
                    ));
                }
                added.push(corners.map(|corner| first + corner));
            }
            triangles
                .vertices
                .extend_from_slice(core::slice::from_raw_parts(vertices, vertex_count as usize));
            triangles.triangles.extend(added);
        }
        Ok(())
    }
}

/// How far a brick's box may be from its mesh's box scaled (per coordinate, as a share of the box's
/// size) for the mesh to be taken as the brick's shape.
const FIT_TOLERANCE: f64 = 1e-4;

/// How a brick scales a mesh whose triangles span `mesh` (smallest and largest corner, in its own
/// space) to show its box `exact` (the same, in the brick), turned inside out along the axes it's
/// `mirrored` along: the scale along each axis, around the origin, as the game scales a brick's
/// mesh (see `docs/re/collision-shapes.md`). `None` unless the box is the mesh's box scaled
/// around the origin (e.g. a brick adding other meshes to its box) or is flattened to nothing.
pub fn scale_to_fit(mesh: [Vec3; 2], mirrored: [bool; 3], exact: [Vec3; 2]) -> Option<Vec3> {
    let mut scale = [1.0; 3];
    for axis in 0..3 {
        let size = mesh[1][axis] - mesh[0][axis];
        let wanted = exact[1][axis] - exact[0][axis];
        let tolerance = FIT_TOLERANCE * wanted.abs().max(1.0);
        if size <= 1e-6 {
            // Flat along this axis: only a flat box fits.
            if wanted > tolerance {
                return None;
            }
            continue;
        }
        if wanted <= 1e-9 {
            // Flattened to nothing: points can't be found back on it.
            return None;
        }
        let sign = if mirrored[axis] { -1.0 } else { 1.0 };
        scale[axis] = sign * wanted / size;
        let [low, high] = [mesh[0][axis], mesh[1][axis]].map(|side| side * scale[axis]);
        if (low.min(high) - exact[0][axis]).abs() > tolerance
            || (low.max(high) - exact[1][axis]).abs() > tolerance
        {
            return None;
        }
    }
    Some(scale)
}

/// The `T` at `offset` bytes into `base`.
///
/// # Safety
/// `base + offset` must hold a valid `T`.
unsafe fn read<T>(base: *const u8, offset: usize) -> T {
    // SAFETY: per the contract.
    unsafe { base.add(offset).cast::<T>().read_unaligned() }
}
