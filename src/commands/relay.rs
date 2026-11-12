//! `relay`: listens on this machine for the agent's OTLP logs, filters them and sends them on,
//! signed with the device key. Keeps nothing: no file, no log, no buffer.
use super::{
    Ctx, Result,
    account::{self, Loaded},
    say,
};
use crate::cli::{VERSION, messages as m};
use crate::infra::{
    http::{Client, Signer},
    relay_server::{self, Event},
};
use crate::relay;
use serde_json::Value;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

const OTLP_LOGS: &str = "/api/ingest/otlp/v1/logs";

pub(super) fn run(ctx: &mut Ctx, port: Option<u16>) -> Result<i32> {
    let account = account::open(ctx)?;
    let connection = match account::load(&account)? {
        Loaded::NotConnected => return say(ctx, m::NOT_CONNECTED, 1),
        Loaded::Broken(text) => return say(ctx, &text, 1),
        Loaded::Ready(c) => c,
    };
    let port = port.unwrap_or(relay::DEFAULT_PORT);
    let listener = match relay_server::bind(port) {
        Ok(l) => l,
        Err(e) => return say(ctx, &m::relay_port_busy(port, &e.kind().to_string()), 1),
    };
    let port = listener.local_addr().map_or(port, |a| a.port());
    say(ctx, &m::relay_listening(port, &connection.server.base()), 0)?;
    ctx.out.flush()?;

    let (server, device, key) = (&connection.server, &connection.device, &connection.key);
    // Each batch is sent with a fresh signature; a client is cheap to make and not shared.
    let forward = |payload: &Value| -> std::result::Result<u64, String> {
        let signer = Signer { device, key };
        Client::new(server.clone(), VERSION)
            .post::<_, Value>(OTLP_LOGS, payload, Some(&signer))
            .map(|answer| relay::refused_by_server(&answer))
            .map_err(|f| m::failure_text(&f, account::now().millis()))
    };
    let (events, seen) = mpsc::channel();
    let stop = AtomicBool::new(false);
    let (listener_ref, stop_ref, forward_ref) = (&listener, &stop, &forward);
    let served = std::thread::scope(|scope| {
        let worker =
            scope.spawn(move || relay_server::serve(listener_ref, stop_ref, forward_ref, &events));
        let shown = show_events(ctx, &seen, &stop, port);
        if shown.is_err() {
            stop.store(true, Ordering::Relaxed); // the screen is gone: do not serve on
            relay_server::wake(port);
        }
        let served = worker
            .join()
            .unwrap_or_else(|_| Err(std::io::Error::other("relay thread failed")));
        shown.map(|()| served)
    })?;
    match served {
        Ok(()) => Ok(0),
        Err(e) => say(ctx, &m::relay_stopped(&e.to_string()), 1),
    }
}

/// Tells the screen about lost batches until the server ends; asks it to end when the system
/// says so.
fn show_events(
    ctx: &mut Ctx,
    seen: &mpsc::Receiver<Event>,
    stop: &AtomicBool,
    port: u16,
) -> Result<()> {
    let mut dropped = 0u64;
    loop {
        match seen.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::Forwarded) => {}
            Ok(Event::Dropped(why)) => {
                dropped += 1;
                if m::report_drop(dropped) {
                    writeln!(ctx.out, "{}", m::relay_dropped(dropped, &why))?;
                    ctx.out.flush()?;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if ctx.system.stopped() {
                    stop.store(true, Ordering::Relaxed);
                    relay_server::wake(port);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}
