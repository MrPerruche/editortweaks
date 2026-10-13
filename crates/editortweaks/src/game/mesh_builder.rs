//! Static meshes made at run time from our own triangles: a `UStaticMeshDescription` filled through
//! reflection, its normals computed by the engine's own mesh operations, then built into a new
//! `UStaticMesh` (`BuildFromStaticMeshDescriptions`). Found in game 1.11.7 (build 25831956); see
//! `docs/re/collision-shapes.md`.
//!
//! A static mesh rather than a procedural mesh: in UE 4.27, meshes drawn the dynamic way (procedural
//! meshes) get no custom primitive data, which brick materials read their color, pattern and
//! texture placement from.

use brickrust::ue::coreuobject::{EObjectFlags, UObjectBase};
use brickrust::ue::fname::FName;
use brickrust::ue::tarray::TArray;
use brickrust::ue::uclass::UClass;

use crate::framework::sig::FunctionSig;
use crate::game::editor_object::Vector;
use crate::game::reflection::{self, Function, Getter, Parameters, put, take};

/// The end of `UStaticMeshDescription::CreateCube` (reflected; game RVA 0x225DFF0), which computes
/// the cube's normals and tangents:
///
/// ```text
/// FStaticMeshOperations::ComputePolygonTangentsAndNormals(this->MeshDescription, 0.0f);  // 0x225CFC0
/// FStaticMeshOperations::ComputeTangentsAndNormals(this->MeshDescription, Normals | Tangents);  // 0x225D330
/// ```
///
/// `MeshDescription` (the `FMeshDescription` inside the object, +0x28) is read from the code too.
pub const COMPUTE_NORMALS: FunctionSig = FunctionSig {
    name: "UStaticMeshDescription::CreateCube (normals)",
    pattern: "0f 57 c9 49 8d 4f ?? e8 ?? ?? ?? ?? ba 03 00 00 00 49 8d 4f ?? e8 ?? ?? ?? ??",
};

/// Offsets in [`COMPUTE_NORMALS`]: the two calls, and the mesh description's offset in each.
const POLYGON_NORMALS_CALL: usize = 7;
const NORMALS_CALL: usize = 21;
const DESCRIPTION_OFFSETS: [usize; 2] = [6, 20];

/// `FStaticMeshOperations::ComputePolygonTangentsAndNormals(FMeshDescription&, float
/// ComparisonThreshold)`: each polygon's normal (registering the polygon attributes it needs).
type PolygonNormalsFn = unsafe extern "C" fn(description: *mut u8, threshold: f32);

/// `FStaticMeshOperations::ComputeTangentsAndNormals(FMeshDescription&, EComputeNTBsFlags)`: each
/// vertex instance's normal and tangent, averaged over the polygons sharing its vertex.
type NormalsFn = unsafe extern "C" fn(description: *mut u8, flags: u32);

/// `EComputeNTBsFlags::Normals | Tangents`, as `CreateCube` asks.
const NORMALS_AND_TANGENTS: u32 = 3;

/// Mesh description element IDs (`FVertexID`, `FPolygonGroupID`, ...): an `int32`, -1 if none.
type ElementId = i32;

/// Triangles for a mesh, in its own space.
#[derive(Clone, Debug, Default)]
pub struct Triangles {
    pub positions: Vec<Vector>,
    pub uvs: Vec<[f32; 2]>,
    /// Three indices into `positions` per triangle, its front facing the side it's clockwise from
    /// (UE's winding). Triangles sharing a vertex are shaded smoothly across it: a sharp edge needs
    /// a vertex per side.
    pub corners: Vec<[u32; 3]>,
}

/// A reflected function and where its parameters go.
struct Call<const N: usize> {
    function: Function,
    parameters: [usize; N],
}

impl<const N: usize> Call<N> {
    /// `name` of `class`, with its parameters `(name, kind, size)`.
    fn find(
        class: *const UClass,
        name: &'static str,
        parameters: [(&'static str, &'static str, i32); N],
    ) -> Result<Self, String> {
        let function = Function::find(class, name)?;
        let mut offsets = [0; N];
        for (offset, (parameter, kind, size)) in offsets.iter_mut().zip(parameters) {
            *offset = function.parameter(parameter, kind, size)?;
        }
        Ok(Self {
            function,
            parameters: offsets,
        })
    }
}

/// Builds static meshes (after engine init).
pub struct MeshBuilder {
    /// `StaticMesh`, and its default object (to call its static functions on).
    mesh_class: *const UClass,
    mesh_defaults: *mut UObjectBase,
    /// `static UStaticMeshDescription* UStaticMesh::CreateStaticMeshDescription(UObject* Outer)`.
    create_description: Call<2>,
    /// `FName UStaticMesh::AddMaterial(UMaterialInterface* Material)`: a new slot.
    add_material: Call<2>,
    /// `UStaticMesh::BuildFromStaticMeshDescriptions(const TArray<UStaticMeshDescription*>&,
    /// bool bBuildSimpleCollision)`.
    build: Call<1>,
    /// `FVertexID UMeshDescriptionBase::CreateVertex()`.
    create_vertex: Getter<ElementId>,
    /// `SetVertexPosition(FVertexID VertexID, const FVector& Position)`.
    set_position: Call<2>,
    /// `FVertexInstanceID CreateVertexInstance(FVertexID VertexID)`.
    create_instance: Call<2>,
    /// `UStaticMeshDescription::SetVertexInstanceUV(FVertexInstanceID, FVector2D UV, int32
    /// UVIndex)`.
    set_uv: Call<2>,
    /// `FPolygonGroupID CreatePolygonGroup()`.
    create_group: Getter<ElementId>,
    /// `UStaticMeshDescription::SetPolygonGroupMaterialSlotName(FPolygonGroupID, const FName&)`.
    set_slot: Call<2>,
    /// `FPolygonID CreatePolygon(FPolygonGroupID, TArray<FVertexInstanceID>& VertexInstanceIDs,
    /// TArray<FEdgeID>& NewEdgeIDs)`: the edges it made are written to the last.
    create_polygon: Call<4>,
    polygon_normals: PolygonNormalsFn,
    normals: NormalsFn,
    /// Offset of the `FMeshDescription` in a `UStaticMeshDescription`.
    description: usize,
}

// SAFETY: reflection records and code living as long as the game; only used on the game thread.
unsafe impl Send for MeshBuilder {}
unsafe impl Sync for MeshBuilder {}

impl MeshBuilder {
    pub fn find() -> Result<Self, String> {
        let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
        let mesh_class = class("StaticMesh")?;
        let base = class("MeshDescriptionBase")?;
        let description_class = class("StaticMeshDescription")?;
        // SAFETY: a live class.
        let mesh_defaults = unsafe { reflection::default_object(mesh_class) }
            .ok_or("StaticMesh has no default object")?;

        let found = COMPUTE_NORMALS.locate()?;
        // SAFETY: the offsets lie within the matched pattern (its length covers both calls).
        let (polygon_normals, normals, offsets) = unsafe {
            (
                found.call_target(POLYGON_NORMALS_CALL),
                found.call_target(NORMALS_CALL),
                DESCRIPTION_OFFSETS.map(|at| usize::from(found.address.add(at).read())),
            )
        };
        let (Some(polygon_normals), Some(normals)) = (polygon_normals, normals) else {
            return Err(format!("{}: calls not found", COMPUTE_NORMALS.name));
        };
        if offsets[0] != offsets[1] {
            return Err(format!(
                "{}: mesh description at {:#x} and {:#x}",
                COMPUTE_NORMALS.name, offsets[0], offsets[1]
            ));
        }

        let id = |name| (name, "StructProperty", 4);
        Ok(Self {
            mesh_class,
            mesh_defaults,
            create_description: Call::find(
                mesh_class,
                "CreateStaticMeshDescription",
                [
                    ("Outer", "ObjectProperty", 8),
                    ("ReturnValue", "ObjectProperty", 8),
                ],
            )?,
            add_material: Call::find(
                mesh_class,
                "AddMaterial",
                [
                    ("Material", "ObjectProperty", 8),
                    ("ReturnValue", "NameProperty", size_of::<FName>() as i32),
                ],
            )?,
            build: Call::find(
                mesh_class,
                "BuildFromStaticMeshDescriptions",
                [("StaticMeshDescriptions", "ArrayProperty", 16)],
            )?,
            create_vertex: Getter::find(base, "CreateVertex", "StructProperty")?,
            set_position: Call::find(
                base,
                "SetVertexPosition",
                [id("VertexID"), ("Position", "StructProperty", 12)],
            )?,
            create_instance: Call::find(
                base,
                "CreateVertexInstance",
                [id("VertexID"), id("ReturnValue")],
            )?,
            set_uv: Call::find(
                description_class,
                "SetVertexInstanceUV",
                [id("VertexInstanceID"), ("UV", "StructProperty", 8)],
            )?,
            create_group: Getter::find(base, "CreatePolygonGroup", "StructProperty")?,
            set_slot: Call::find(
                description_class,
                "SetPolygonGroupMaterialSlotName",
                [
                    id("PolygonGroupID"),
                    ("SlotName", "NameProperty", size_of::<FName>() as i32),
                ],
            )?,
            create_polygon: Call::find(
                base,
                "CreatePolygon",
                [
                    id("PolygonGroupID"),
                    ("VertexInstanceIDs", "ArrayProperty", 16),
                    ("NewEdgeIDs", "ArrayProperty", 16),
                    id("ReturnValue"),
                ],
            )?,
            // SAFETY: the starts of the two functions `CreateCube` calls with these arguments.
            polygon_normals: unsafe {
                core::mem::transmute::<*const u8, PolygonNormalsFn>(polygon_normals)
            },
            // SAFETY: as above.
            normals: unsafe { core::mem::transmute::<*const u8, NormalsFn>(normals) },
            description: offsets[0],
        })
    }

    /// A new static mesh of `triangles` (one section, `material` in its slot), kept while
    /// something uses it; `None` if the engine refused a step.
    ///
    /// # Safety
    /// `outer` must be a live object and `material` null or a live material. Game thread only.
    pub unsafe fn build(
        &self,
        outer: *mut UObjectBase,
        triangles: &Triangles,
        material: *mut UObjectBase,
    ) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; each call gets its parameters at their reflected offsets.
        unsafe {
            let mesh = reflection::new_object(
                self.mesh_class,
                outer,
                FName::default(),
                EObjectFlags::RF_Transient,
            );
            if mesh.is_null() {
                return None;
            }
            let description = self.describe(mesh, triangles, material)?;
            let normals_of = description.cast::<u8>().add(self.description);
            (self.polygon_normals)(normals_of, 0.0);
            (self.normals)(normals_of, NORMALS_AND_TANGENTS);

            let call = &self.build;
            let mut parameters = Parameters::new();
            let descriptions = [description];
            put(&mut parameters, call.parameters[0], borrow(&descriptions));
            call.function.call(mesh, &mut parameters);
            Some(mesh)
        }
    }

    /// A new mesh description of `triangles` inside `mesh`, its polygon group using a new slot of
    /// `mesh` with `material`.
    ///
    /// # Safety
    /// As for [`MeshBuilder::build`], `mesh` a live static mesh.
    unsafe fn describe(
        &self,
        mesh: *mut UObjectBase,
        triangles: &Triangles,
        material: *mut UObjectBase,
    ) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; each call gets its parameters at their reflected offsets.
        unsafe {
            let call = &self.create_description;
            let mut parameters = Parameters::new();
            put(&mut parameters, call.parameters[0], mesh);
            call.function.call(self.mesh_defaults, &mut parameters);
            let description = take::<*mut UObjectBase>(&parameters, call.parameters[1]);
            if description.is_null() {
                return None;
            }

            let group = valid(self.create_group.call(description))?;
            let call = &self.add_material;
            let mut parameters = Parameters::new();
            put(&mut parameters, call.parameters[0], material);
            call.function.call(mesh, &mut parameters);
            let slot = take::<FName>(&parameters, call.parameters[1]);
            let call = &self.set_slot;
            let mut parameters = Parameters::new();
            put(&mut parameters, call.parameters[0], group);
            put(&mut parameters, call.parameters[1], slot);
            call.function.call(description, &mut parameters);

            // One vertex instance per vertex: triangles sharing a vertex share its normal.
            let mut instances = Vec::with_capacity(triangles.positions.len());
            for (&position, &uv) in triangles.positions.iter().zip(&triangles.uvs) {
                let vertex = valid(self.create_vertex.call(description))?;
                let call = &self.set_position;
                let mut parameters = Parameters::new();
                put(&mut parameters, call.parameters[0], vertex);
                put(&mut parameters, call.parameters[1], position);
                call.function.call(description, &mut parameters);

                let call = &self.create_instance;
                let mut parameters = Parameters::new();
                put(&mut parameters, call.parameters[0], vertex);
                call.function.call(description, &mut parameters);
                let instance = valid(take::<ElementId>(&parameters, call.parameters[1]))?;
                instances.push(instance);

                let call = &self.set_uv;
                let mut parameters = Parameters::new();
                put(&mut parameters, call.parameters[0], instance);
                put(&mut parameters, call.parameters[1], uv);
                // `UVIndex` stays 0.
                call.function.call(description, &mut parameters);
            }

            for corners in &triangles.corners {
                let corners = corners
                    .iter()
                    .map(|&corner| instances.get(corner as usize).copied())
                    .collect::<Option<Vec<ElementId>>>()?;
                let call = &self.create_polygon;
                let mut parameters = Parameters::new();
                put(&mut parameters, call.parameters[0], group);
                // Only read by the function.
                put(&mut parameters, call.parameters[1], borrow(&corners));
                call.function.call(description, &mut parameters);
                // The engine allocated this one.
                let mut edges = take::<TArray<ElementId>>(&parameters, call.parameters[2]);
                if !edges.data.is_null() {
                    edges.free();
                }
                valid(take::<ElementId>(&parameters, call.parameters[3]))?;
            }
            Some(description)
        }
    }
}

/// `id` unless it's the engine's invalid ID (-1).
fn valid(id: ElementId) -> Option<ElementId> {
    (id >= 0).then_some(id)
}

/// A `TArray` pointing into `values`, for a `const TArray&` parameter: the callee must only read it.
fn borrow<T>(values: &[T]) -> TArray<T> {
    TArray {
        data: values.as_ptr().cast_mut(),
        num: values.len() as i32,
        max: values.len() as i32,
    }
}
