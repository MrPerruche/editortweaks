# Actuator limit arrows

How the editor draws a selected actuator's limits as arrows. Found in BRMK 1.11.6
(`BrickRigsModKitSteam-BrickRigs.dll` exports, reflection data), confirmed in game 1.11.7 (build
25831956; both signatures also match 25633865). Used by `features/actuator_arrows` through
`game/actuator.rs` (2026-10-11).

## The limits

`UActuatorBrick::MaxLimit` and `MinLimit` (reflected floats; `__PPO__` exports: BRMK +0x1CC, +0x1D0;
game +0x1BC, +0x1C0). The game's sliders: `MaxLimit` -20 to 0, `MinLimit` 0 to 20 (logged by
`property_limits`). From the user: `MaxLimit` is the "max extension", `MinLimit` the "max
contraction".

## Drawing

`virtual void UActuatorBrick::UpdateEditorVisualization()` (export BRMK 0xF73F0; game 0xD0F970,
split over several unwind entries, the arrows in the third) calls `UBrick::UpdateEditorVisualization`,
then, for each entry of an array from a virtual (game vtable +0x3D0; one entry for the actuators
tried), draws arrows through a lambda (BRMK 0x797B0, game 0xCC9750; all four calls in the game are
from this function). The static info is `StaticInfoClass`'s default object, an
`UActuatorBrickStaticInfo`:

| Field | Reflected on | BRMK | Game |
|---|---|---|---|
| `ActuationAxis` (`EAxis`) | `ActuatorBrickBaseStaticInfo` | +0x1A0 | +0x190 |
| `LinearActuationOffset` | same | +0x1A4 | +0x194 |
| `LinearActuationRange` | same | +0x1A8 | +0x198 |
| `bIsAngularActuator` | `ActuatorBrickStaticInfo` | +0x1B0 | +0x1A0 |

```text
Axis = unit X, Y or Z from ActuationAxis (1, 2, else)
if (bIsAngularActuator) { if (MinLimit + Threshold < MaxLimit) two circular arrows ... }
else {
    if (MinLimit > Threshold)
        SetArrow(Axis * Offset, MakeFromZ(Axis).ToQuat(), angle 0, size MinLimit, Linear, Negative)
    if (-MaxLimit > Threshold)
        SetArrow(Axis * (Offset + Range), MakeFromZ(-Axis).ToQuat(), angle 0, size -MaxLimit, Linear, Positive)
}
remove the arrows past the last one set
```

So a limit on the other side of zero draws nothing. The color is the last argument,
`EBrickUIColorStyle` (BRMK: Default 0, Selected, Foreground, Highlight, Positive 4, Negative 5,
Neutral, Custom; the game passes 5 and 6, so one more entry comes before them there). Looked up by
name at run time.

## The lambda (`SetArrow`)

`(captures*, const FVector& Location, const FQuat& Rotation, float Angle (xmm3), float Size,
uint8 ArrowType, uint8 ColorStyle)`; the captures hold the editor params, the arrow counter, the
arrow class and the actuator. It takes the next arrow from a `TArray` of shared handles in the
actuator's editor params (BRMK +0x30, game +0x28; not reflected) or creates one, calls
`SetRelativeLocationAndRotation(Location, Rotation)`, then sets the reflected fields of
`UBrickEditorArrowComponent` (BRMK +0x648..+0x658, game +0x568..+0x578), updating its mesh or
material when one changed: `ArrowType` (`EBrickEditorArrowType` Linear 0, Circular 1),
`LinearArrowLength` = Size, `CircularArrowAngle` = Angle, `CircularArrowRadius` = Size, `ColorStyle`.

Signatures: the lambda by the arrow type's compare, store and `UpdateMesh` call
(`0f b6 8d ?? ?? 00 00 3a 88 ?? ?? 00 00 74 ?? 88 88 ?? ?? 00 00 48 8b c8 e8`); the function by the
second linear arrow's test and start (`xorps xmm10, xmm9; comiss xmm10, xmm11; addss xmm2,
[rbx + Offset]; jbe`). The function's code is split into chained unwind entries, so
`sig::direct_calls` (main entry only) can't find its calls to the lambda.

## The feature

`features/actuator_arrows` hooks both: for a linear actuator with `MaxLimit > 0` or `MinLimit < 0`,
the function runs with the limits at `-|MaxLimit|` and `|MinLimit|`, then they're put back (same
bits). During that call, a linear arrow of a limit that crossed zero (by its color) is turned
around on its base: a half turn about its own X axis (its Z axis reverses; the game's
`MakeFromZ(-Axis)` differs by a turn about the arrow's axis). Each limit's base stays where the game
puts it (first version, 2026-10-11: also moved `Range` along to where the game starts arrows pointing
that way, which put it at the other end of the brick: misleading, dropped). Both limits on one side
draw two arrows from different bases.
