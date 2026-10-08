/// Outcome of [`Feature::on_load`]. An `Err` disables the feature for this session and its
/// message is logged, so the reason shows up in `brickworks.txt`.
pub type LoadResult = Result<(), String>;

/// One self-contained piece of functionality (a tweak, a tool, ...).
///
/// Each feature lives in its own module under `features/` and is registered once in
/// [`crate::features::ALL`]. All callbacks run on the game thread, in registration order.
///
/// A feature that can't find what it needs (e.g. a byte signature after a game update) must
/// return an error from [`Feature::on_load`] rather than crash, so the rest of the mod keeps
/// working.
pub trait Feature: Sync {
    /// Short, stable identifier used in logs.
    fn id(&self) -> &'static str;

    /// Called once from `mod_init`, before the engine starts. Resolve and validate everything the
    /// feature depends on and install its hooks here.
    fn on_load(&self) -> LoadResult {
        Ok(())
    }

    /// Called once after `UEngine::Init`. Reflection, `GObjects` and game statics work from here.
    fn on_engine_init(&self) {}

    /// Called after every map load (main menu, editor, game maps). `GWorld` is valid here.
    fn on_map_load(&self) {}
}
