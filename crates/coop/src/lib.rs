//! Coop building: several players editing one vehicle (`docs/re/coop/`). The wire protocol and
//! the session logic, with no game code, so `cargo test -p coop` runs simulated members against
//! each other. The mod turns editor changes into [`Op`]s and [`Event`]s back into editor changes.

pub mod archive;
pub mod audit;
pub mod framing;
pub mod ids;
pub mod limits;
pub mod match_watch;
pub mod message;
pub mod ops;
pub mod order;
pub mod presence;
pub mod references;
pub mod session;
pub mod tcp;
pub mod transport;
pub mod undo;
pub mod vehicle;
pub mod wire;

pub use audit::{Audit, Difference, Digest};
pub use ids::{Check, CoopId, Slot};
pub use message::{Member, Message, PeerId, PeerInfo, RefuseReason};
pub use ops::{GroupKind, Groups, Op, Transform, Value};
pub use session::{
    EndReason, Event, HostSettings, Outgoing, Session, SessionError, SnapshotRequest,
};
pub use transport::Transport;
pub use undo::{Change, StepBack};
