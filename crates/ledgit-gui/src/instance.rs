//! One Ledgit at a time.
//!
//! Two copies of the app on one budget would each hold their own staging
//! area and undo history over the same file, and whichever wrote last would
//! win. So the first copy to start listens on a loopback port picked from
//! the user's name; a second copy finds the port taken, hands its budget
//! path to the first - which opens it and comes to the front - and exits.
//! That is also what makes double-clicking a `.ledgit` file in Explorer
//! open it in the window you already have.
//!
//! A socket rather than a lock file because it has to carry the path
//! across, and it frees itself however the first copy ends. Loopback only:
//! nothing off this machine can reach it, and binding it asks nothing of
//! the firewall. If something that is not Ledgit holds the port, the app
//! runs unguarded rather than refusing to start.

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

/// First word of every message, so a stranger on the port is not mistaken
/// for a running Ledgit. Bump the number if the message ever changes shape.
const HELLO: &str = "LEDGIT-1";

/// What a later launch asked the running app to do.
#[derive(Debug, PartialEq, Eq)]
pub struct Request {
    /// A budget to open, or `None` just to come to the front.
    pub path: Option<PathBuf>,
}

pub enum Claim {
    /// This is the only Ledgit; later launches report to the inbox.
    Primary(Inbox),
    /// A Ledgit was already running and has taken over. Exit.
    Forwarded,
    /// The port is held by something that is not Ledgit. Run anyway.
    Unguarded,
}

/// Where requests from later launches arrive.
pub struct Inbox {
    rx: mpsc::Receiver<Request>,
    ctx: Arc<Mutex<Option<egui::Context>>>,
}

impl Inbox {
    /// Wake this context whenever a request arrives, so it is acted on at
    /// once rather than on the next mouse move.
    pub fn attach(&self, ctx: &egui::Context) {
        if let Ok(mut slot) = self.ctx.lock() {
            *slot = Some(ctx.clone());
        }
    }

    /// Every request since the last call.
    pub fn take(&self) -> Vec<Request> {
        self.rx.try_iter().collect()
    }
}

/// A port for this user, so two people signed in to one PC each get their
/// own Ledgit. Stable across runs: FNV-1a over the name, into the dynamic
/// range.
pub fn port_for(user: &str) -> u16 {
    let mut h: u32 = 0x811c_9dc5;
    for b in user.to_lowercase().bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    49_152 + (h % 16_000) as u16
}

/// Become the running Ledgit, or hand `path` to the one that already is.
pub fn claim(port: u16, user: &str, path: Option<&Path>) -> Claim {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    match TcpListener::bind(addr) {
        Ok(listener) => Claim::Primary(serve(listener, user.to_string())),
        Err(_) => match forward(addr, user, path) {
            Ok(()) => Claim::Forwarded,
            Err(_) => Claim::Unguarded,
        },
    }
}

fn serve(listener: TcpListener, user: String) -> Inbox {
    let (tx, rx) = mpsc::channel();
    let ctx: Arc<Mutex<Option<egui::Context>>> = Arc::new(Mutex::new(None));
    let wake = Arc::clone(&ctx);
    std::thread::Builder::new()
        .name("ledgit-instance".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let Some(request) = answer(stream, &user) else { continue };
                if tx.send(request).is_err() {
                    return; // the app has gone
                }
                if let Some(ctx) = wake.lock().ok().and_then(|c| c.clone()) {
                    ctx.request_repaint();
                }
            }
        })
        .expect("spawn the instance listener");
    Inbox { rx, ctx }
}

/// Read one request and say whether it was taken. Anything malformed, slow
/// or from another user is turned away.
fn answer(stream: TcpStream, user: &str) -> Option<Request> {
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.trim_end_matches(['\r', '\n']).splitn(3, '\t');
    let ok = parts.next() == Some(HELLO) && parts.next() == Some(user);
    let path = parts.next().unwrap_or("");
    let mut stream = stream;
    let _ = stream.write_all(if ok { b"OK\n" } else { b"NO\n" });
    ok.then(|| Request { path: (!path.is_empty()).then(|| PathBuf::from(path)) })
}

fn forward(addr: SocketAddr, user: &str, path: Option<&Path>) -> std::io::Result<()> {
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(1))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    // The running copy has its own working directory; send a path that
    // does not depend on ours.
    let path = match path {
        Some(p) => std::path::absolute(p)?.to_string_lossy().into_owned(),
        None => String::new(),
    };
    stream.write_all(format!("{HELLO}\t{user}\t{path}\n").as_bytes())?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    if reply.trim() == "OK" {
        Ok(())
    } else {
        Err(std::io::Error::other("not a Ledgit, or not this user's"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A port nothing is using right now.
    fn free_port() -> u16 {
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap().port()
    }

    fn wait_for(inbox: &Inbox) -> Vec<Request> {
        for _ in 0..100 {
            let got = inbox.take();
            if !got.is_empty() {
                return got;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Vec::new()
    }

    #[test]
    fn a_second_launch_hands_its_budget_to_the_first() {
        let port = free_port();
        let Claim::Primary(inbox) = claim(port, "aaron", None) else {
            panic!("the first launch runs");
        };
        let file = Path::new("budgets/home.ledgit");
        assert!(matches!(claim(port, "aaron", Some(file)), Claim::Forwarded));
        let got = wait_for(&inbox);
        assert_eq!(got.len(), 1);
        let path = got[0].path.as_ref().expect("a path was sent");
        assert!(path.is_absolute(), "sent as an absolute path: {path:?}");
        assert!(path.ends_with("budgets/home.ledgit"));

        // Launched with no file: just come to the front.
        assert!(matches!(claim(port, "aaron", None), Claim::Forwarded));
        assert_eq!(wait_for(&inbox), vec![Request { path: None }]);
    }

    #[test]
    fn a_stranger_on_the_port_is_not_a_ledgit() {
        let port = free_port();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).unwrap();
        std::thread::spawn(move || {
            for mut s in listener.incoming().flatten() {
                let _ = s.write_all(b"HTTP/1.0 400 Bad Request\r\n\r\n");
            }
        });
        assert!(matches!(claim(port, "aaron", None), Claim::Unguarded));
    }

    #[test]
    fn another_users_ledgit_does_not_take_the_request() {
        let port = free_port();
        let Claim::Primary(inbox) = claim(port, "someone-else", None) else { panic!() };
        assert!(matches!(claim(port, "aaron", None), Claim::Unguarded));
        std::thread::sleep(Duration::from_millis(50));
        assert!(inbox.take().is_empty());
    }

    #[test]
    fn each_user_gets_a_stable_port_in_the_dynamic_range() {
        assert_eq!(port_for("Aaron"), port_for("aaron"));
        assert_ne!(port_for("aaron"), port_for("guest"));
        assert!(port_for("aaron") >= 49_152);
    }
}
