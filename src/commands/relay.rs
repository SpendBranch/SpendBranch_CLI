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
    net::{Ipv4Addr, TcpListener, TcpStream},
    sync::{atomic::AtomicBool, mpsc},
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
    let port = port.unwrap_or(m::RELAY_DEFAULT_PORT);
    // Only this machine: the address is fixed, never taken from the command line.
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
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
    let mut dropped = 0u64;
    let mut served = Ok(());
    let (listener_ref, stop_ref, forward_ref) = (&listener, &stop, &forward);
    std::thread::scope(|scope| -> Result<()> {
        let worker =
            scope.spawn(move || relay_server::serve(listener_ref, stop_ref, forward_ref, &events));
        loop {
            match seen.recv_timeout(Duration::from_millis(100)) {
                Ok(Event::Forwarded) => {}
                Ok(Event::Dropped(why)) => {
                    dropped += 1;
                    if dropped == 1 || matches!(dropped % 50, 0) {
                        writeln!(ctx.out, "{}", m::relay_dropped(dropped, &why))?;
                        ctx.out.flush()?;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if ctx.system.stopped() {
                        stop.store(true, std::sync::atomic::Ordering::Relaxed);
                        // Ends the wait for the next connection.
                        let _ = TcpStream::connect((Ipv4Addr::LOCALHOST, port));
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        served = worker
            .join()
            .unwrap_or_else(|_| Err(std::io::Error::other("relay thread failed")));
        Ok(())
    })?;
    match served {
        Ok(()) => Ok(0),
        Err(e) => say(ctx, &format!("The relay stopped: {e}"), 1),
    }
}
