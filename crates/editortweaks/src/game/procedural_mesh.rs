//! Meshes given as whole arrays in one call: `UProceduralMeshComponent` (the engine's
//! ProceduralMeshComponent plugin, compiled into the game) and its reflected
//! `CreateMeshSection_LinearColor`. Found by name in game 1.11.7; see `docs/re/collision-shapes.md`.
//!
//! Much cheaper to make than a static mesh built at run time (`game::mesh_builder`, a few reflected
//! calls per vertex), but drawn the dynamic way, without custom primitive data: only for materials
//! that don't read it (not the bricks' own).

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UClass;

use crate::game::mesh_builder::borrow;
use crate::game::reflection::{self, Function, Parameters, put};

/// Where the parameters of `CreateMeshSection_LinearColor` go.
struct CreateSection {
    function: Function,
    section: usize,
    vertices: usize,
    triangles: usize,
    normals: usize,
    uvs: usize,
    collision: usize,
}

/// Fills procedural mesh components (after engine init).
pub struct ProceduralMeshes {
    /// `ProceduralMeshComponent`.
    class: *const UClass,
    /// `UProceduralMeshComponent::CreateMeshSection_LinearColor(int32 SectionIndex, const
    /// TArray<FVector>& Vertices, const TArray<int32>& Triangles, const TArray<FVector>& Normals,
    /// const TArray<FVector2D>& UV0, (UV1, UV2, UV3,) const TArray<FLinearColor>& VertexColors,
    /// const TArray<FProcMeshTangent>& Tangents, bool bCreateCollision)`. The parameters not set
    /// stay empty arrays: no vertex colors, the engine's default tangents.
    create_section: CreateSection,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for ProceduralMeshes {}
unsafe impl Sync for ProceduralMeshes {}

impl ProceduralMeshes {
    pub fn find() -> Result<Self, String> {
        let class = reflection::find_class("ProceduralMeshComponent")
            .ok_or("class ProceduralMeshComponent not found")?;
        let function = Function::find(class, "CreateMeshSection_LinearColor")?;
        let array = |name| function.parameter(name, "ArrayProperty", 16);
        // Checked so a changed signature disables the feature rather than passing garbage.
        array("VertexColors")?;
        array("Tangents")?;
        let create_section = CreateSection {
            section: function.parameter("SectionIndex", "IntProperty", 4)?,
            vertices: array("Vertices")?,
            triangles: array("Triangles")?,
            normals: array("Normals")?,
            uvs: array("UV0")?,
            collision: function.parameter("bCreateCollision", "BoolProperty", 1)?,
            function,
        };
        Ok(Self {
            class,
            create_section,
        })
    }

    /// The component class, to make components with (`MeshComponents::create_of`).
    pub fn class(&self) -> *const UClass {
        self.class
    }

    /// Replaces `component`'s first section with these triangles (three indices into `positions`
    /// each, the front facing the side they're clockwise from), without collision. `normals` and
    /// `uvs` have one entry per position.
    ///
    /// # Safety
    /// `component` must be a live procedural mesh component. Game thread only.
    pub unsafe fn set_section(
        &self,
        component: *mut UObjectBase,
        positions: &[[f32; 3]],
        corners: &[[u32; 3]],
        normals: &[[f32; 3]],
        uvs: &[[f32; 2]],
    ) {
        let triangles: Vec<i32> = corners
            .iter()
            .flatten()
            .map(|&corner| i32::try_from(corner).unwrap_or(0))
            .collect();
        let call = &self.create_section;
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; each parameter goes at its reflected offset. The arrays are
        // `const TArray&` parameters, only read during the call, pointing into slices that outlive
        // it.
        unsafe {
            put(&mut parameters, call.section, 0i32);
            put(&mut parameters, call.vertices, borrow(positions));
            put(&mut parameters, call.triangles, borrow(&triangles));
            put(&mut parameters, call.normals, borrow(normals));
            put(&mut parameters, call.uvs, borrow(uvs));
            put(&mut parameters, call.collision, false);
            call.function.call(component, &mut parameters);
        }
    }
}
