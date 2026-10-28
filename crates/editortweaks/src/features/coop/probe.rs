//! The Steam check of milestone 2 (`docs/re/coop/protocol.md`): in the editor, logs our SteamID
//! and sends a coop hello to ourselves over Steam's messages. Answered in game 1.11.7: Steam
//! delivers messages to self, in under a second (`testing.md`, section 4). Kept, as it shows at
//! each start whether Steam's messages work.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use ::coop::{Event, PeerInfo, Session, transport::Transport};

use super::log::coop_log;
use crate::game::steam::Steam;

/// Our channel of Steam's messages ("ET"). The game itself talks over SteamSockets, not messages.
const CHANNEL: i32 = 0x4554;
/// How long the probe waits for its hello to itself.
const SELF_SEND_WAIT: Duration = Duration::from_secs(15);

/// Steam's messages as a coop transport. Only the peers the session expects get their sessions
/// accepted, so strangers can't open one.
struct SteamTransport {
    steam: Steam,
    expected: Vec<u64>,
}

impl Transport for SteamTransport {
    fn send(&mut self, to: u64, bytes: &[u8], reliable: bool) {
        if !self.steam.send(to, CHANNEL, bytes, reliable) {
            coop_log!("Steam refused a message to {to}");
        }
    }

    fn receive(&mut self) -> Vec<(u64, Vec<u8>)> {
        for &peer in &self.expected {
            self.steam.accept(peer);
        }
        // Steam only delivers from sessions we accepted or opened, i.e. expected peers; checking
        // the sender anyway keeps that from depending on Steam's behaviour.
        let mut messages = self.steam.receive(CHANNEL);
        messages.retain(|(sender, _)| self.expected.contains(sender));
        messages
    }
}

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

/// Logs how Steam reaches `peer`, to see in game that the traffic is relayed.
fn log_connection(steam: Steam, peer: u64) {
    match steam.peer_connection(peer) {
        Some(connection) => coop_log!(
            "Steam connection to {peer}: state {}, relayed {}, relay {}, flags {:#x}",
            connection.state,
            connection.relayed,
            connection.relay_pop.as_deref().unwrap_or("none"),
            connection.flags
        ),
        None => coop_log!("Steam has no connection to {peer}"),
    }
}

fn start(info: PeerInfo) -> State {
    let steam = match Steam::get() {
        Ok(steam) => steam,
        Err(reason) => {
            coop_log!("Steam unavailable: {reason}");
            return State::Done;
        }
    };
    // Before anything is sent: a peer must never learn our IP address, so Steam may only use
    // Valve's relay. Without that guarantee coop stays off.
    if steam.force_relay() {
        coop_log!("Steam: direct connections disabled, relay only");
    } else {
        coop_log!("Steam refused relay-only mode: coop stays off, so no address can leak");
        return State::Done;
    }
    let own_id = steam.own_id();
    coop_log!(
        "Steam user {own_id} ({}); sending a hello to ourselves",
        steam.own_name()
    );
    let mut session = Session::new(info);
    session.hello(own_id);
    State::Running(Box::new(Probe {
        session,
        transport: SteamTransport {
            steam,
            expected: vec![own_id],
        },
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
                    log_connection(probe.transport.steam, probe.own_id);
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
            log_connection(probe.transport.steam, probe.own_id);
        }
        // Our hello's reply (a second hello) comes shortly after the first.
        if probe.answered && probe.sent_at.elapsed() > SELF_SEND_WAIT {
            *state = State::Done;
        }
    });
}
