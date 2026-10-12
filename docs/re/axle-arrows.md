# Axle arrows and suspension

How the editor draws a selected axle's arrows, and where its suspension lets the wheels go. Found
in BRMK 1.11.6 (`BrickRigsModKitSteam-BrickRigs.dll` exports, reflection data), confirmed in game
1.11.7 (builds 25831956 and 25633865). Used by `features/suspension_arrows` through
`game/axle.rs` (2026-10-11). See also `actuator-arrows.md`.

## Properties

`UAxleBrick` (reflected; BRMK offsets, the game's are 8 lower where checked: `bDriven` +0x154,
`SteeringAngle` +0x15C): `SuspensionLength` +0x150, `SuspensionStiffness` +0x154,
`SuspensionDamping` +0x158, `bDriven` +0x15C, `bInvertDrive` +0x15D (full `bool`s, compared as
bytes), `BrakeStrength` +0x160, `SteeringAngle` +0x164, `SteeringSpeed` +0x168.
`UAxleBrickStaticInfo`: `SuspensionStiffness`, `SuspensionDamping`. The feature reads them by name.

## Drawing

`virtual void UAxleBrick::UpdateEditorVisualization()` (export BRMK 0xF7D40; game 0xD10180 in
25831956, 0xD10150 in 25633865; split over unwind entries, the arrows in a later one) calls
`UBrick::UpdateEditorVisualization`, then, if the editor params want arrows and the static info
has an arrow class, for each connector of type 2 or 5 (wheel connectors; connectors from virtual
`GetBrickConnectors`, BRMK vtable +0x490, game +0x3D0, elements 0x34 / 0x30 bytes):

```text
T = GetConnectorRelativeTransform(Connector)      // virtual, BRMK +0x4A0, game +0x3E0
if (|SteeringAngle| > SMALL_NUMBER)
    SetArrow(T.Location, MakeFromZX(Up, T.Rotation * Z).Rotator(), angle 2 * SteeringAngle,
             radius 60, Positive)
if (bDriven)
    SetArrow(T.Location + T.Rotation * ((0, 0, 30) * T.Scale), MakeFromZX(bInvertDrive ? -Right
             : Right, Up).Rotator(), angle 270, radius 30, Negative)
remove the arrows past the last one set
```

Colors as the game's numbers show them (5 and 6 in the game, 4 and 5 in BRMK; looked up by name).

`SetArrow` is a lambda (BRMK 0x79EC0, game 0xCC9D00 in 25831956): `(captures*, const FVector&
Location, const FRotator& Rotation, float Angle (xmm3), float Size, uint8 ColorStyle)`. Its
captures are laid out like the actuators' lambda's (editor params, arrow counter, arrow class,
brick: `[+0]`, `[+8]`, `[+0x10]`, `[+0x18]`), and its code is the same except that it never sets
`ArrowType` or `LinearArrowLength` on an arrow it reuses (a new one gets the class defaults): it
assumes circular arrows.

Signature: the steering arrow's doubled angle, color, radius and the call to the lambda
(`f3 0f 58 db c6 44 24 28 ?? 48 8d 54 24 ?? f3 ?? 0f 11 ?? 24 20 48 8d 4d ?? e8`); the lambda is
the call's target (`SigMatch::call_target`, 25 bytes in).

## Suspension

Each wheel connection makes a `UPhysicsConstraintComponent` (BRMK 0x4290B0) placed by the
connection's virtual +0x358 (BRMK 0x42FAA0 for `UWheelConnection`, vtable BRMK 0x863A88):
location = the axle's connector's relative location (`GetConnectorRelativeTransform(Index)`),
rotation = `MakeFromXZ(Connector Z, Connector X)`: the constraint's X axis (twist, the wheel's
spin) is the connector's Z, its Z axis the connector's X. Then (BRMK 0x4293D0):

```text
Half = GetSuspensionLength() * 0.5
if (Half > 0) {
    SetLinearZLimit(Limited, Half)                // ±Half along the constraint's Z
    SetLinearPositionDrive(false, false, true); SetLinearVelocityDrive(false, false, true)
    SetLinearDriveParams(SuspensionStiffness, SuspensionDamping, 0)
    SetLinearPositionTarget((0, 0, Half))         // the spring pushes to one end
}
```

So a wheel moves `SuspensionLength` in all, and the spring holds it at one end until loaded. From
the user (2026-10-11): where the wheel is built is the top of its travel, it never goes above the
axle; so the travel is `SuspensionLength` down from there (the constraint's frames must be offset by
`Half`; not traced). The feature draws it along the axle's -Z axis, assuming the wheel connectors'
X axis is the axle's vertical (the suspension moves wheels up and down; the steering arrow, around
the axle's Z axis, uses the connector's Z as its X axis, so that one is horizontal).

## The feature

`features/suspension_arrows` hooks the function and its lambda. For an axle with a suspension, the
function runs with `SteeringAngle` at 1 if it was 0 (put back after, same bits), so the lambda is
called for every wheel connector; the steering call (by its color, `Positive`) is at the connector:
there the feature adds a straight arrow (`Linear`, `Positive`, length `SuspensionLength`, a half
turn about X: pointing down) through the actuators' lambda, and leaves out the stand-in steering
arrow (first version, 2026-10-11: two arrows of half the length, up and down, assuming the travel
centered on the built place: wrong, the built place is the top). Every arrow of the axle lambda goes through the actuators' lambda too
(type `Circular`, the rotator turned into a quaternion), so a reused straight arrow becomes
circular again.
