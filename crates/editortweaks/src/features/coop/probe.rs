//! The Steam check of milestone 2 (`docs/re/coop/protocol.md`): in the editor, logs our SteamID
//! and sends a coop hello to ourselves over Steam's messages. Answered in game 1.11.7: Steam
//! delivers messages to self, in under a second (`testing.md`, section 4). Kept, as it shows at
//! each start whether Steam's messages work.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use ::coop::{Event, PeerInfo, Session};

use super::log::coop_log;
use super::steam_link::SteamTransport;
use crate::framework::timings;

/// How long the probe waits for its hello to itself.
const SELF_SEND_WAIT: Duration = Duration::from_secs(15);

struct Probe {
    session: Session,
    transport: SteamTransport,
    own_id: u64,
    sent_at: Instant,
    answered: bool,
}

/// The probe starts on the editor's first tick, stops for good if Steam is missing, and is done
/// once answered.
enum State {
    NotStarted,
    Running(Box<Probe>),
    Done,
}

thread_local! {
    // The game thread's: the editor ticks there.
    static STATE: RefCell<State> = const { RefCell::new(State::NotStarted) };
}

fn start(info: PeerInfo) -> State {
    let _span = timings::span("coop::probe::start");
    // Relay only, before anything is sent: a peer must never learn our IP address. Without that
    // guarantee coop stays off.
    let mut transport = match SteamTransport::new(false) {
        Ok(transport) => transport,
        Err(reason) => {
            coop_log!("Steam unavailable: {reason}");
            return State::Done;
        }
    };
    coop_log!("Steam: direct connections disabled, relay only");
    let own_id = transport.steam.own_id();
    coop_log!(
        "Steam user {own_id} ({}); sending a hello to ourselves",
        transport.steam.own_name()
    );
    transport.expect(vec![own_id]);
    let mut session = Session::new(info);
    session.hello(own_id);
    State::Running(Box::new(Probe {
        session,
        transport,
        own_id,
        sent_at: Instant::now(),
        answered: false,
    }))
}

/// Runs the probe on (each) editor tick, until it's done.
pub fn tick(info: impl FnOnce() -> PeerInfo) {
    STATE.with_borrow_mut(|state| {
        if matches!(state, State::NotStarted) {
            *state = start(info());
        }
        let State::Running(probe) = state else { return };
        ::coop::transport::pump(&mut probe.session, &mut probe.transport);
        for event in probe.session.take_events() {
            match event {
                Event::Hello { peer, info } if peer == probe.own_id && !probe.answered => {
                    probe.answered = true;
                    coop_log!(
                        "Steam self-send works ({:.1} s; mod {}, game {})",
                        probe.sent_at.elapsed().as_secs_f32(),
                        info.mod_version,
                        info.game_version
                    );
                    probe.transport.log_connection(probe.own_id);
                }
                Event::BadMessage { peer, error } => {
                    coop_log!("unreadable Steam message from {peer}: {error}")
                }
                _ => {}
            }
        }
        if !probe.answered && probe.sent_at.elapsed() > SELF_SEND_WAIT {
            probe.answered = true;
            coop_log!(
                "no hello from ourselves after {} s: Steam's messages don't arrive",
                SELF_SEND_WAIT.as_secs()
            );
            probe.transport.log_connection(probe.own_id);
        }
        // Our hello's reply (a second hello) comes shortly after the first.
        if probe.answered && probe.sent_at.elapsed() > SELF_SEND_WAIT {
            *state = State::Done;
        }
    });
}
