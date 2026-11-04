//! The game's chat, for the mod's own messages to this player: what
//! `ABrickPlayerController::ClientReceiveChatMessage` shows when a server sends a message. It's a
//! reflected client RPC: called on the local controller it runs right here, and nothing goes over
//! the network, so no one else sees these messages. Found through BRMK 1.11.6's exports and
//! Blueprints, then in game 1.11.7 (build 25831956); see `docs/re/chat.md`.
//!
//! A message has no free text of its own but for players' chat, which comes in one colour: the game
//! makes the text from its type, and takes the colour from the type too. So a message of ours has
//! the type of the colour wanted, our text in its `TextOption` and [`OURS`] in its `IntOption`, and
//! [`detour`] answers `FBrickChatMessage::GetText` with that text.
//!
//! The game hides the chat in single-player games: [`Chat::say`] fails there.

use std::sync::OnceLock;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UStruct;

use crate::framework::hook;
use crate::framework::sig::StringRefSig;
use crate::framework::timings;
use crate::game::kismet::{self, Texts};
use crate::game::net_mode::{self, NetMode, Role};
use crate::game::reflection::{self, Function, Parameters, put, take};
use crate::game::text::Text;
use crate::game::world;

/// `FText FBrickChatMessage::GetText() const` (BRMK 0x2AF300, game 1.11.7 RVA 0xE24B50), which the
/// message widget shows: a switch on the type, each case formatting a string table entry. This key
/// is the case of `EChatMessageType::Join`.
pub const MESSAGE_TEXT: StringRefSig = StringRefSig {
    name: "FBrickChatMessage::GetText",
    text: "Info_ChatMessage_PlayerJoined_Format",
};

/// [`MESSAGE_TEXT`]'s signature: returns `out`, the new `FText`.
type MessageTextFn = unsafe extern "C" fn(message: *const u8, out: *mut Text) -> *mut Text;

/// In a message's `IntOption`: it's ours. The game uses that field only for players' messages (the
/// chat context, and [`detour`] skips those) and vehicle spawns (a brick count, never this high).
const OURS: i32 = 0x4554_4348;

/// How a message of ours looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// The chat's usual colour.
    Plain,
    /// Green.
    Good,
    /// Red.
    Bad,
}

/// Where a message keeps what [`detour`] reads (reflected).
#[derive(Clone, Copy)]
struct Layout {
    kind: usize,
    number: usize,
    text: usize,
    /// `EChatMessageType::Message`: players' messages.
    players: u8,
}

static LAYOUT: OnceLock<Layout> = OnceLock::new();
static ORIGINAL: OnceLock<MessageTextFn> = OnceLock::new();

/// What sending ourselves a message takes (after engine init).
pub struct Chat {
    /// `UGameplayStatics`' default object and `GetPlayerController`, with its parameters' offsets.
    statics: *mut UObjectBase,
    get_controller: Function,
    world: usize,
    index: usize,
    controller: usize,
    /// `ABrickPlayerController`.
    controller_class: *const UStruct,
    /// `ClientReceiveChatMessage`, and where its message goes.
    receive: Function,
    message: usize,
    layout: Layout,
    /// The type giving each [`Tone`] (see `docs/re/chat.md`): `Join`, `Unban`, `JoinBanned`.
    plain: u8,
    good: u8,
    bad: u8,
    texts: Texts,
    net: NetMode,
}

// SAFETY: reflection records and a default object living as long as the game; only used on the game
// thread.
unsafe impl Send for Chat {}
unsafe impl Sync for Chat {}

fn message_type(entry: &str) -> Result<u8, String> {
    let value = reflection::enum_value("EChatMessageType", &format!("EChatMessageType::{entry}"))?;
    u8::try_from(value).map_err(|_| format!("EChatMessageType::{entry} is {value}"))
}

/// Finds the chat's parts and hooks what makes a message's text. After engine init, once.
pub fn find() -> Result<Chat, String> {
    let class =
        |name: &'static str| reflection::find_class(name).ok_or(format!("class {name} not found"));
    let statics_class = class("GameplayStatics")?;
    let statics = reflection::find_object("Default__GameplayStatics", "GameplayStatics")
        .ok_or("GameplayStatics' default object not found")?;
    let get_controller = Function::find(statics_class, "GetPlayerController")?;
    let controller_class = class("BrickPlayerController")?;
    let receive = Function::find(controller_class, "ClientReceiveChatMessage")?;
    let message_struct = reflection::find_object("BrickChatMessage", "ScriptStruct")
        .ok_or("struct BrickChatMessage not found")?
        .cast::<UStruct>()
        .cast_const();
    // SAFETY: a live struct.
    let size = unsafe { reflection::struct_size(message_struct) } as i32;
    let field =
        |name, kind, size| reflection::struct_field_offset(message_struct, name, kind, size);
    let layout = Layout {
        kind: field("Type", "EnumProperty", 1)?,
        number: field("IntOption", "IntProperty", 4)?,
        text: field("TextOption", "TextProperty", size_of::<Text>() as i32)?,
        players: message_type("Message")?,
    };
    let chat = Chat {
        statics,
        world: get_controller.parameter("WorldContextObject", "ObjectProperty", 8)?,
        index: get_controller.parameter("PlayerIndex", "IntProperty", 4)?,
        controller: get_controller.parameter("ReturnValue", "ObjectProperty", 8)?,
        get_controller,
        controller_class: controller_class.cast(),
        message: receive.only_parameter(&["StructProperty"], size)?,
        receive,
        layout,
        plain: message_type("Join")?,
        good: message_type("Unban")?,
        bad: message_type("JoinBanned")?,
        texts: kismet::texts()?,
        net: net_mode::find()?,
    };
    let _ = LAYOUT.set(layout);
    let target = MESSAGE_TEXT.resolve()?;
    // SAFETY: `target` is the start of FBrickChatMessage::GetText, and the detour has its signature.
    unsafe {
        hook::install_typed(
            MESSAGE_TEXT.name,
            target,
            detour as MessageTextFn,
            &ORIGINAL,
        )?;
    }
    Ok(chat)
}

/// Our messages' text is what they carry; any other message's is the game's to make.
unsafe extern "C" fn detour(message: *const u8, out: *mut Text) -> *mut Text {
    let _span = timings::span("game::chat::detour");
    // SAFETY: the game's message, read at reflected offsets; `out` is where the game wants a new
    // text, so nothing there is overwritten, and a copy is how the game hands out a text.
    unsafe {
        if let Some(layout) = LAYOUT.get()
            && message.add(layout.kind).read() != layout.players
            && message.add(layout.number).cast::<i32>().read_unaligned() == OURS
        {
            out.write(Text::copy_of(message.add(layout.text).cast()));
            return out;
        }
    }
    let original = *ORIGINAL.get().expect("hook recorded before use");
    // SAFETY: the original function with its own arguments.
    unsafe { timings::game(MESSAGE_TEXT.name, || original(message, out)) }
}

impl Chat {
    /// Shows `text` in this player's chat only. Fails if there's no chat to show it: a single-player
    /// game, or no player yet.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn say(&self, tone: Tone, text: &str) -> Result<(), String> {
        // SAFETY: on the game thread. Each value goes to its reflected offset in a zeroed message,
        // which is what an empty one is to the copy the RPC's thunk makes: null ids and empty
        // strings (`docs/re/chat.md`). The controller is checked to have the function. The text
        // goes in as ours: the game copies the message and ProcessEvent destroys nothing.
        unsafe {
            let world = world::current();
            if world.is_null() {
                return Err("no world".into());
            }
            if self.net.role(world) == Role::Standalone {
                return Err("a single-player game has no chat".into());
            }
            let mut parameters = Parameters::new();
            put(&mut parameters, self.world, world);
            put(&mut parameters, self.index, 0i32);
            self.get_controller.call(self.statics, &mut parameters);
            let controller: *mut UObjectBase = take(&parameters, self.controller);
            if controller.is_null() || !reflection::is_a(controller, self.controller_class) {
                return Err("no player controller with a chat".into());
            }
            let kind = match tone {
                Tone::Plain => self.plain,
                Tone::Good => self.good,
                Tone::Bad => self.bad,
            };
            let layout = &self.layout;
            let mut parameters = Parameters::new();
            put(&mut parameters, self.message + layout.kind, kind);
            put(&mut parameters, self.message + layout.number, OURS);
            let at = self.message + layout.text;
            put(&mut parameters, at, self.texts.text(text));
            self.receive.call(controller, &mut parameters);
            take::<Text>(&parameters, at).release();
        }
        Ok(())
    }
}
