//! A session over real localhost sockets, as two game instances on one PC run it.

use std::time::{Duration, Instant};

use coop::tcp::TcpTransport;
use coop::transport::pump;
use coop::{Event, HostSettings, Op, PeerInfo, Session};

const HOST: u64 = 1;
const MEMBER: u64 = 2;

fn info() -> PeerInfo {
    PeerInfo {
        protocol: (coop::message::PROTOCOL, coop::message::PROTOCOL),
        mod_version: "test".into(),
        game_version: "1.11.7".into(),
        hosting: false,
    }
}

/// Pumps both until `done` says so (or two seconds pass), collecting the events.
fn run(
    a: (&mut Session, &mut TcpTransport),
    b: (&mut Session, &mut TcpTransport),
    mut done: impl FnMut(&[Event], &[Event]) -> bool,
) -> (Vec<Event>, Vec<Event>) {
    let (mut events_a, mut events_b) = (Vec::new(), Vec::new());
    let start = Instant::now();
    while !done(&events_a, &events_b) {
        assert!(start.elapsed() < Duration::from_secs(2), "timed out");
        pump(a.0, a.1);
        pump(b.0, b.1);
        events_a.extend(a.0.take_events());
        events_b.extend(b.0.take_events());
        std::thread::sleep(Duration::from_millis(1));
    }
    (events_a, events_b)
}

#[test]
fn join_load_and_edit_over_tcp() {
    let mut host_link = TcpTransport::listen(HOST, 0).unwrap();
    let port = host_link.port().unwrap();
    let mut member_link =
        TcpTransport::connect(MEMBER, port, HOST, Duration::from_secs(1)).unwrap();

    let mut host = Session::new(info());
    let ids = host
        .host(
            7,
            "host".into(),
            HostSettings {
                code: "abc".into(),
                max_members: 3,
            },
            3,
        )
        .unwrap();
    let mut member = Session::new(info());
    member.join(HOST, "abc".into(), "member".into()).unwrap();

    // The host is asked for a snapshot, answers, and the member loads it.
    let vehicle = vec![0x5a; 300_000];
    let mut answered = false;
    let start = Instant::now();
    let mut loaded = None;
    while loaded.is_none() {
        assert!(start.elapsed() < Duration::from_secs(2), "timed out");
        pump(&mut host, &mut host_link);
        pump(&mut member, &mut member_link);
        for event in host.take_events() {
            if let Event::SnapshotWanted(request) = event {
                host.send_snapshot(request, ids.clone(), vehicle.clone())
                    .unwrap();
                answered = true;
            }
        }
        for event in member.take_events() {
            if let Event::Load { ids, vehicle } = event {
                loaded = Some((ids, vehicle));
            }
        }
    }
    assert!(answered);
    assert_eq!(loaded, Some((ids.clone(), vehicle)));

    // The member creates a brick; the host gets it, and so does the member's own confirmation.
    let new = member.new_ids(1).unwrap();
    let create = Op::Create {
        ids: new.clone(),
        archive: vec![1, 2, 3],
    };
    member.edit(vec![create.clone()]).unwrap();
    let (host_events, _) = run(
        (&mut host, &mut host_link),
        (&mut member, &mut member_link),
        |host, _| host.iter().any(|e| matches!(e, Event::Apply { .. })),
    );
    let applied: Vec<_> = host_events
        .iter()
        .filter_map(|e| match e {
            Event::Apply { ops, .. } => Some(ops.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(applied, [vec![create]]);

    // The host's edits reach the member.
    let delete = Op::Delete { id: ids[0] };
    host.edit(vec![delete.clone()]).unwrap();
    let (_, member_events) = run(
        (&mut host, &mut host_link),
        (&mut member, &mut member_link),
        |_, member| member.iter().any(|e| matches!(e, Event::Apply { .. })),
    );
    assert!(member_events.contains(&Event::Apply {
        author: coop::ids::SUBHOST_SLOT,
        ops: vec![delete],
    }));
    assert!(!member_events.contains(&Event::Drifted));
}
