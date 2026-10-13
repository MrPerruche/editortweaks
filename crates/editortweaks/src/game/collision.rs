//! Bricks' collision shapes: the simple collision of their body setup (`UBodySetup::AggGeom`),
//! which PhysX simulates contacts with, read through reflection, and the scale the game gives it.
//! Found through BRMK 1.11.6, then in game 1.11.7 (build 25831956); see
//! `docs/re/collision-shapes.md`.
//!
//! brick -> `UBrickEditorObject::GetRootComponent` (its `UBrickStaticMeshComponent`) ->
//! `StaticMesh` -> `BodySetup` (a spinner: its own `SpinnerBodySetup`) -> `AggGeom` (boxes, spheres,
//! capsules, convex hulls, in the mesh's space).

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fproperty::FProperty;
use brickrust::ue::tarray::TArray;
use brickrust::ue::uclass::UStruct;

use crate::game::editor_object::{LocalBounds, Vector};
use crate::game::reflection::{self, Getter};
use crate::math::{self, Quat};

/// The smallest scale the game gives a body setup, per axis (`UBrick::UpdateBodySetupScale3D`
/// clamps to it).
const MIN_SCALE: f32 = 0.01;

/// Sanity limits: brick meshes have far fewer.
const MAX_ELEMENTS: i32 = 4096;
const MAX_VERTICES: i32 = 65_536;

/// A box (`FKBoxElem`).
#[derive(Clone, Copy, Debug)]
pub struct BoxShape {
    pub center: Vector,
    /// An `FRotator`: pitch, yaw, roll in degrees.
    pub rotation: Vector,
    /// Full lengths along its own axes.
    pub size: Vector,
}

/// A sphere (`FKSphereElem`).
#[derive(Clone, Copy, Debug)]
pub struct SphereShape {
    pub center: Vector,
    pub radius: f32,
}

/// A capsule (`FKSphylElem`): a cylinder along its own Z axis, `length` long (without the caps),
/// with a half sphere at each end.
#[derive(Clone, Copy, Debug)]
pub struct CapsuleShape {
    pub center: Vector,
    pub rotation: Vector,
    pub radius: f32,
    pub length: f32,
}

/// A body setup's simple collision, unscaled, in its mesh's space.
#[derive(Clone, Debug, Default)]
pub struct Shapes {
    pub boxes: Vec<BoxShape>,
    pub spheres: Vec<SphereShape>,
    pub capsules: Vec<CapsuleShape>,
    /// Convex hulls (`FKConvexElem`): their points, the element's transform already applied. The
    /// hull's faces aren't kept (PhysX makes them when cooking).
    pub hulls: Vec<Vec<Vector>>,
    /// Tapered capsules: PhysX doesn't support them, so they don't collide.
    pub ignored: usize,
}

impl Shapes {
    pub fn is_empty(&self) -> bool {
        self.boxes.is_empty()
            && self.spheres.is_empty()
            && self.capsules.is_empty()
            && self.hulls.is_empty()
    }
}

/// An array of elements in `FKAggregateGeom`: where it is and how far apart its elements are.
struct Elements {
    offset: usize,
    stride: usize,
}

impl Elements {
    /// The array field `name` of `FKAggregateGeom`, and the struct of its elements.
    fn find(
        geometry: *const UStruct,
        name: &'static str,
    ) -> Result<(Self, *const UStruct), String> {
        let property = reflection::struct_field(geometry, name, "ArrayProperty", 16)
            .map_err(|reason| format!("KAggregateGeom: {reason}"))?;
        // SAFETY: a live ArrayProperty record; its inner property and struct live as long as it.
        unsafe {
            let element = reflection::struct_type(reflection::array_inner(property));
            if element.is_null() {
                return Err(format!("KAggregateGeom: {name} doesn't hold structs"));
            }
            let found = Self {
                offset: (*property).offset_internal as usize,
                stride: reflection::struct_size(element),
            };
            Ok((found, element))
        }
    }

    /// The addresses of the elements in the `FKAggregateGeom` at `geometry`.
    ///
    /// # Safety
    /// `geometry` must point to a live `FKAggregateGeom`.
    unsafe fn of(&self, geometry: *const u8) -> Result<Vec<*const u8>, String> {
        // SAFETY: per the contract; the array holds `num` elements `stride` bytes apart.
        unsafe {
            let array = geometry.add(self.offset).cast::<TArray<u8>>().read();
            if array.num == 0 {
                return Ok(Vec::new());
            }
            if !(0..=MAX_ELEMENTS).contains(&array.num) || array.data.is_null() {
                return Err(format!("unexpected collision elements ({})", array.num));
            }
            Ok((0..array.num as usize)
                .map(|index| array.data.add(index * self.stride).cast_const())
                .collect())
        }
    }
}

/// Offsets of the fields read from each kind of element.
struct BoxFields {
    elements: Elements,
    center: usize,
    rotation: usize,
    x: usize,
    y: usize,
    z: usize,
}

struct SphereFields {
    elements: Elements,
    center: usize,
    radius: usize,
}

struct CapsuleFields {
    elements: Elements,
    center: usize,
    rotation: usize,
    radius: usize,
    length: usize,
}

struct HullFields {
    elements: Elements,
    /// `TArray<FVector> VertexData`.
    vertices: usize,
    /// `FTransform Transform`.
    transform: usize,
}

/// Reads bricks' collision shapes (after engine init).
pub struct Collision {
    /// `UPrimitiveComponent* UBrickEditorObject::GetRootComponent()`.
    root: Getter<*mut UObjectBase>,
    /// `StaticMeshComponent`.
    mesh_component: *const UStruct,
    /// Offsets of `UStaticMeshComponent::StaticMesh`, `UStaticMesh::BodySetup` and
    /// `UBodySetup::AggGeom`.
    static_mesh: usize,
    body_setup: usize,
    geometry: usize,
    /// `SpinnerBrick` and the offset of its `SpinnerBodySetup`, made for each spinner's shape.
    spinner: Option<(*const UStruct, usize)>,
    /// `FBoxSphereBounds UStaticMesh::GetBounds() const`: origin, box extent, sphere radius.
    mesh_bounds: Getter<[f32; 7]>,
    local_bounds: LocalBounds,
    /// The brick classes whose body setup is scaled (`GetBodySetupScale3D` overridden):
    /// `ScalableBrick` (its size) and `WheelBrick` (its radius and width).
    scaled: Vec<*const UStruct>,
    boxes: BoxFields,
    spheres: SphereFields,
    capsules: CapsuleFields,
    hulls: HullFields,
    /// `TaperedCapsuleElems`, if this engine has them.
    tapered: Option<Elements>,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for Collision {}
unsafe impl Sync for Collision {}

impl Collision {
    pub fn find() -> Result<Self, String> {
        let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
        let mesh_component = class("StaticMeshComponent")?;
        let static_mesh = class("StaticMesh")?;
        let body_setup = class("BodySetup")?;
        let geometry = struct_property(body_setup.cast(), "AggGeom")
            .map_err(|reason| format!("BodySetup: {reason}"))?;
        // SAFETY: a live StructProperty record.
        let (geometry_offset, geometry_struct) = unsafe {
            (
                (*geometry).offset_internal as usize,
                reflection::struct_type(geometry),
            )
        };
        if geometry_struct.is_null() {
            return Err("BodySetup: AggGeom isn't a struct".into());
        }
        let field = |ustruct: *const UStruct, name, kind, size| {
            reflection::struct_field_offset(ustruct, name, kind, size)
        };
        let (elements, ustruct) = Elements::find(geometry_struct, "BoxElems")?;
        let boxes = BoxFields {
            elements,
            center: field(ustruct, "Center", "StructProperty", 12)?,
            rotation: field(ustruct, "Rotation", "StructProperty", 12)?,
            x: field(ustruct, "X", "FloatProperty", 4)?,
            y: field(ustruct, "Y", "FloatProperty", 4)?,
            z: field(ustruct, "Z", "FloatProperty", 4)?,
        };
        let (elements, ustruct) = Elements::find(geometry_struct, "SphereElems")?;
        let spheres = SphereFields {
            elements,
            center: field(ustruct, "Center", "StructProperty", 12)?,
            radius: field(ustruct, "Radius", "FloatProperty", 4)?,
        };
        let (elements, ustruct) = Elements::find(geometry_struct, "SphylElems")?;
        let capsules = CapsuleFields {
            elements,
            center: field(ustruct, "Center", "StructProperty", 12)?,
            rotation: field(ustruct, "Rotation", "StructProperty", 12)?,
            radius: field(ustruct, "Radius", "FloatProperty", 4)?,
            length: field(ustruct, "Length", "FloatProperty", 4)?,
        };
        let (elements, ustruct) = Elements::find(geometry_struct, "ConvexElems")?;
        let hulls = HullFields {
            elements,
            vertices: field(ustruct, "VertexData", "ArrayProperty", 16)?,
            transform: field(ustruct, "Transform", "StructProperty", 0x30)?,
        };
        let spinner = reflection::find_class("SpinnerBrick").and_then(|spinner| {
            reflection::field_offset(spinner, "SpinnerBodySetup", "ObjectProperty", 8)
                .ok()
                .map(|offset| (spinner.cast::<UStruct>(), offset))
        });
        let scaled = ["ScalableBrick", "WheelBrick"]
            .into_iter()
            .map(|name| class(name).map(|found| found.cast::<UStruct>()))
            .collect::<Result<_, _>>()?;
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
            body_setup: reflection::field_offset(static_mesh, "BodySetup", "ObjectProperty", 8)?,
            geometry: geometry_offset,
            spinner,
            mesh_bounds: Getter::find(static_mesh, "GetBounds", "StructProperty")?,
            local_bounds: LocalBounds::find()?,
            scaled,
            boxes,
            spheres,
            capsules,
            hulls,
            tapered: Elements::find(geometry_struct, "TaperedCapsuleElems")
                .ok()
                .map(|(elements, _)| elements),
        })
    }

    /// The component showing `brick` (a static mesh component), if any.
    ///
    /// # Safety
    /// `brick` must be a live brick. Game thread only.
    pub unsafe fn component_of(&self, brick: *mut UObjectBase) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; the root component is a live object if not null.
        unsafe {
            let component = self.root.call(brick);
            (!component.is_null() && is_a(component, &[self.mesh_component])).then_some(component)
        }
    }

    /// The body setup `brick` collides with (`UBrick::GetBodySetup`: its mesh's, a spinner's own),
    /// shown by `component` (see [`Collision::component_of`]), and its static mesh.
    ///
    /// # Safety
    /// `brick` must be a live brick and `component` its static mesh component. Game thread only.
    pub unsafe fn body_setup_of(
        &self,
        brick: *mut UObjectBase,
        component: *mut UObjectBase,
    ) -> (Option<*mut UObjectBase>, Option<*mut UObjectBase>) {
        // SAFETY: per the contract; offsets come from reflection, checked against the classes.
        unsafe {
            let mesh = read::<*mut UObjectBase>(component.cast(), self.static_mesh);
            let mesh = (!mesh.is_null()).then_some(mesh);
            if let Some((spinner, offset)) = self.spinner
                && is_a(brick, &[spinner])
            {
                let body_setup = read::<*mut UObjectBase>(brick.cast(), offset);
                return ((!body_setup.is_null()).then_some(body_setup), mesh);
            }
            let body_setup = mesh
                .map(|mesh| read::<*mut UObjectBase>(mesh.cast(), self.body_setup))
                .filter(|body_setup| !body_setup.is_null());
            (body_setup, mesh)
        }
    }

    /// The brick's local bounds (min, max) and its static mesh's bounds (origin, extent), for the
    /// log.
    ///
    /// # Safety
    /// `brick` must be a live brick and `mesh` the static mesh it shows. Game thread only.
    pub unsafe fn bounds_report(&self, brick: *mut UObjectBase, mesh: *mut UObjectBase) -> String {
        // SAFETY: per the contract.
        unsafe {
            let [ox, oy, oz, ex, ey, ez, _] = self.mesh_bounds.call(mesh);
            format!(
                "local bounds {:?}, mesh bounds origin {:?} extent {:?}",
                self.local_bounds.of(brick),
                [ox, oy, oz],
                [ex, ey, ez]
            )
        }
    }

    /// The scale the game gives `brick`'s body setup (`UBrick::GetBodySetupScale3D`, clamped like
    /// `UpdateBodySetupScale3D` does): 1 except for scalable bricks and wheels.
    ///
    /// It isn't reflected, but the brick's reflected local bounds are its mesh's bounds times it
    /// (`UBrick::GetBrickEditorObjectLocalBounds`), so it's their ratio. Taken from the bounds'
    /// largest corner: thrusters (scalable) move their smallest Z down for the nozzle.
    ///
    /// # Safety
    /// `brick` must be a live brick in the editor and `mesh` the static mesh it shows. Game thread
    /// only.
    pub unsafe fn scale_of(&self, brick: *mut UObjectBase, mesh: *mut UObjectBase) -> Vector {
        // SAFETY: per the contract.
        unsafe {
            if !is_a(brick, &self.scaled) {
                return [1.0; 3];
            }
            let Some([local_min, local_max]) = self.local_bounds.of(brick) else {
                return [1.0; 3];
            };
            let [ox, oy, oz, ex, ey, ez, _] = self.mesh_bounds.call(mesh);
            let (origin, extent) = ([ox, oy, oz], [ex, ey, ez]);
            core::array::from_fn(|axis| {
                let (mesh_max, mesh_min) =
                    (origin[axis] + extent[axis], origin[axis] - extent[axis]);
                // The corner farther from the mesh's origin gives the more precise ratio; the max
                // unless it's close to 0.
                let ratio = if mesh_max.abs() > extent[axis] * 0.01 {
                    local_max[axis] / mesh_max
                } else if mesh_min.abs() > extent[axis] * 0.01 {
                    local_min[axis] / mesh_min
                } else {
                    1.0
                };
                if ratio.is_finite() {
                    ratio.max(MIN_SCALE)
                } else {
                    1.0
                }
            })
        }
    }

    /// The simple collision of `body_setup`, unscaled.
    ///
    /// # Safety
    /// `body_setup` must be a live `UBodySetup`. Game thread only.
    pub unsafe fn shapes(&self, body_setup: *mut UObjectBase) -> Result<Shapes, String> {
        let mut shapes = Shapes::default();
        // SAFETY: per the contract; every offset comes from reflection, and arrays are read within
        // their counts.
        unsafe {
            let geometry = body_setup.cast::<u8>().add(self.geometry).cast_const();
            let fields = &self.boxes;
            for element in fields.elements.of(geometry)? {
                shapes.boxes.push(BoxShape {
                    center: read(element, fields.center),
                    rotation: read(element, fields.rotation),
                    size: [
                        read(element, fields.x),
                        read(element, fields.y),
                        read(element, fields.z),
                    ],
                });
            }
            let fields = &self.spheres;
            for element in fields.elements.of(geometry)? {
                shapes.spheres.push(SphereShape {
                    center: read(element, fields.center),
                    radius: read(element, fields.radius),
                });
            }
            let fields = &self.capsules;
            for element in fields.elements.of(geometry)? {
                shapes.capsules.push(CapsuleShape {
                    center: read(element, fields.center),
                    rotation: read(element, fields.rotation),
                    radius: read(element, fields.radius),
                    length: read(element, fields.length),
                });
            }
            let fields = &self.hulls;
            for element in fields.elements.of(geometry)? {
                let vertices = read::<TArray<Vector>>(element, fields.vertices);
                if !(0..=MAX_VERTICES).contains(&vertices.num)
                    || (vertices.num > 0 && vertices.data.is_null())
                {
                    return Err(format!("unexpected convex hull ({} points)", vertices.num));
                }
                let transform = read::<Transform>(element, fields.transform);
                let points = core::slice::from_raw_parts(vertices.data, vertices.num as usize);
                shapes
                    .hulls
                    .push(points.iter().map(|&point| transform.apply(point)).collect());
            }
            if let Some(tapered) = &self.tapered {
                shapes.ignored = tapered.of(geometry)?.len();
            }
        }
        Ok(shapes)
    }
}

/// The `StructProperty` called `name` of `ustruct`, whatever its size (`FKAggregateGeom`'s differs
/// between builds).
fn struct_property(
    ustruct: *const UStruct,
    name: &'static str,
) -> Result<*const FProperty, String> {
    // SAFETY: `ustruct` comes from reflection.
    unsafe {
        let property = reflection::find_property(ustruct, name);
        if property.is_null() || !reflection::is_kind(property, "StructProperty") {
            return Err(format!("field '{name}' not found"));
        }
        Ok(property)
    }
}

/// An `FTransform` (UE 4.27 with SIMD): a rotation quaternion (x, y, z, w), then the translation
/// and the 3D scale, each padded to four floats.
#[repr(C)]
#[derive(Clone, Copy)]
struct Transform {
    rotation: [f32; 4],
    translation: [f32; 4],
    scale: [f32; 4],
}

impl Transform {
    /// `FTransform::TransformPosition`: scaled, then rotated, then moved.
    fn apply(&self, point: Vector) -> Vector {
        let scaled = core::array::from_fn(|axis| f64::from(point[axis] * self.scale[axis]));
        let [x, y, z, _] = self.translation;
        let moved = math::add(
            Quat::widen(self.rotation).rotate(scaled),
            math::widen([x, y, z]),
        );
        math::narrow(moved)
    }
}

/// Whether `object`'s class is one of `classes` or derives from one.
///
/// # Safety
/// `object` must be a live object.
unsafe fn is_a(object: *const UObjectBase, classes: &[*const UStruct]) -> bool {
    // SAFETY: per the contract.
    unsafe { reflection::with_parents(reflection::class_of(object.cast())) }
        .any(|class| classes.contains(&class))
}

/// The `T` at `offset` bytes into `base`.
///
/// # Safety
/// `base + offset` must hold a valid `T`.
unsafe fn read<T>(base: *const u8, offset: usize) -> T {
    // SAFETY: per the contract.
    unsafe { base.add(offset).cast::<T>().read_unaligned() }
}
