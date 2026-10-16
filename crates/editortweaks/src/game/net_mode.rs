//! This machine's part in the game: alone, hosting or joined. `UKismetSystemLibrary::IsStandalone`
//! and `IsServer(const UObject* WorldContextObject)`, called through reflection on the library's
//! default object, like the engine's other Blueprint helpers (see `game::kismet`).

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UClass;

use crate::game::reflection::{self, Function, Parameters};

/// This machine's part in the game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// A single-player game: no server, no clients.
    Standalone,
    /// The server of a game others join (a listen server).
    Host,
    /// Joined someone else's game.
    Client,
}

pub struct NetMode {
    /// The library's default object, which static functions are called on.
    defaults: *mut UObjectBase,
    standalone: Query,
    server: Query,
}

/// A `static bool F(const UObject* WorldContextObject)`.
struct Query {
    function: Function,
    context: usize,
    result: usize,
}

// SAFETY: reflection records and a default object living as long as the game; only used on the
// game thread.
unsafe impl Send for NetMode {}
unsafe impl Sync for NetMode {}

/// Finds the functions. After engine init.
pub fn find() -> Result<NetMode, String> {
    const LIBRARY: &str = "KismetSystemLibrary";
    let class = reflection::find_class(LIBRARY).ok_or(format!("class {LIBRARY} not found"))?;
    Ok(NetMode {
        defaults: reflection::find_object("Default__KismetSystemLibrary", LIBRARY)
            .ok_or(format!("{LIBRARY}'s default object not found"))?,
        standalone: Query::find(class, "IsStandalone")?,
        server: Query::find(class, "IsServer")?,
    })
}

impl Query {
    fn find(class: *const UClass, name: &'static str) -> Result<Self, String> {
        let function = Function::find(class, name)?;
        Ok(Self {
            context: function.parameter("WorldContextObject", "ObjectProperty", 8)?,
            result: function.parameter("ReturnValue", "BoolProperty", 1)?,
            function,
        })
    }

    /// # Safety
    /// `defaults` must be the library's default object, `world` a live world. Game thread only.
    unsafe fn ask(&self, defaults: *mut UObjectBase, world: *mut UObjectBase) -> bool {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; both offsets come from the function's parameters.
        unsafe {
            reflection::put(&mut parameters, self.context, world);
            self.function.call(defaults, &mut parameters);
            reflection::take::<u8>(&parameters, self.result) != 0
        }
    }
}

impl NetMode {
    /// This machine's part in the game of `world`.
    ///
    /// # Safety
    /// `world` must be a live world. Game thread only.
    pub unsafe fn role(&self, world: *mut UObjectBase) -> Role {
        // SAFETY: per the contract.
        unsafe {
            if self.standalone.ask(self.defaults, world) {
                Role::Standalone
            } else if self.server.ask(self.defaults, world) {
                Role::Host
            } else {
                Role::Client
            }
        }
    }
}
