//! The original localhost NukeServerSocket protocol; never launches or retries Nuke.
use crate::{
    incoming::{IncomingItem, nuke_sequence_path},
    jobs::{Checkpoint, JobControl},
};
use std::{
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpStream},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Submission {
    Confirmed,
    NotSubmitted,
    Uncertain,
}
#[derive(Clone, Debug)]
pub struct NukeReply {
    pub submission: Submission,
    pub message: String,
}
const SUCCESS: &[u8] = b"Read node created successfully";

pub fn command(item: &IncomingItem) -> Result<Vec<u8>, String> {
    let (path, range) = match item {
        IncomingItem::Single(single) => (single.file.path.clone(), None),
        IncomingItem::Sequence(sequence) => {
            let first = sequence
                .members
                .first()
                .ok_or("The sequence is empty.")?
                .frame;
            let last = sequence
                .members
                .last()
                .ok_or("The sequence is empty.")?
                .frame;
            (nuke_sequence_path(sequence)?, Some((first, last)))
        }
    };
    let path = path
        .to_str()
        .ok_or("Nuke cannot receive this non-UTF-8 path.")?;
    if path
        .chars()
        .any(|c| c.is_control() || matches!(c, '[' | ']' | '$' | '%'))
    {
        return Err("The source path contains a Nuke expression character; it cannot be sent as a literal filename.".into());
    }
    if matches!(item, IncomingItem::Single(_)) && path.contains('#') {
        return Err(
            "A literal # in this filename would be interpreted as a Nuke frame token.".into(),
        );
    }
    if let IncomingItem::Sequence(sequence) = item {
        if path.bytes().filter(|b| *b == b'#').count() != sequence.padding {
            return Err("The source path contains a literal # outside its frame token.".into());
        }
    }
    #[cfg(windows)]
    let path = path.replace('\\', "/");
    let literal = serde_json::to_string(&path).map_err(|e| e.to_string())?;
    let mut code = format!(
        "import nuke\nread_node = nuke.createNode('Read')\nread_node['file'].setValue({literal})\n"
    );
    if let Some((first, last)) = range {
        code.push_str(&format!(
            "read_node['first'].setValue({first})\nread_node['last'].setValue({last})\n"
        ));
    }
    code.push_str("read_node.setXYpos(int(read_node.xpos()), int(read_node.ypos()) + 100)\nviewer = nuke.activeViewer()\nif viewer:\n    viewer.node().setInput(0, read_node)\nprint('Read node created successfully')");
    let bytes = serde_json::to_vec(&serde_json::json!({"text":code,"formatText":"0"}))
        .map_err(|e| e.to_string())?;
    if bytes.len() > 64 * 1024 {
        return Err("The Nuke request is too large.".into());
    }
    Ok(bytes)
}

pub fn send(item: &IncomingItem, control: &JobControl) -> NukeReply {
    let reply = |submission, message: String| NukeReply {
        submission,
        message,
    };
    let payload = match command(item) {
        Ok(p) => p,
        Err(e) => return reply(Submission::NotSubmitted, e),
    };
    if control.checkpoint() == Checkpoint::Cancelled {
        return reply(
            Submission::NotSubmitted,
            "Send cancelled before submission.".into(),
        );
    }
    let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 49512);
    let mut socket = match TcpStream::connect_timeout(&address.into(), Duration::from_secs(1)) {
        Ok(s) => s,
        Err(e) => {
            return reply(
                Submission::NotSubmitted,
                format!(
                    "Could not connect to Nuke at 127.0.0.1:49512. Enable NukeServerSocket in the running Nuke session. {e}"
                ),
            );
        }
    };
    if let Err(e) = socket
        .set_read_timeout(Some(Duration::from_millis(100)))
        .and_then(|_| socket.set_write_timeout(Some(Duration::from_millis(100))))
    {
        return reply(
            Submission::NotSubmitted,
            format!("Could not set the Nuke connection deadline: {e}"),
        );
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut sent = 0;
    let mut response = Vec::new();
    while sent < payload.len() {
        if control.is_cancelled() || Instant::now() >= deadline {
            break;
        }
        match socket.write(&payload[sent..]) {
            Ok(0) => break,
            Ok(n) => sent += n,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::Interrupted
                ) => {}
            Err(_) => break,
        }
    }
    if sent == 0 {
        return reply(
            Submission::NotSubmitted,
            "No request bytes were sent to Nuke.".into(),
        );
    }
    if sent == payload.len() {
        let mut buffer = [0u8; 4096];
        while Instant::now() < deadline && !control.is_cancelled() {
            match socket.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    if response.len() + n > 64 * 1024 {
                        break;
                    }
                    response.extend_from_slice(&buffer[..n]);
                    if response.windows(SUCCESS.len()).any(|w| w == SUCCESS) {
                        return reply(
                            Submission::Confirmed,
                            "Nuke confirmed that the Read node was created.".into(),
                        );
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => break,
            }
        }
    }
    reply(Submission::Uncertain,"Nuke did not confirm the result after submission. Check Nuke before sending again; this request will not be retried automatically.".into())
}
