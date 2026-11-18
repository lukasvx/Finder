//! UI ↔ daemon protocol: length-prefixed binary frames (u32 LE length, then payload).

use std::io::{self, Read, Write};

/// A connection to the daemon.
#[cfg(unix)]
pub type Stream = std::os::unix::net::UnixStream;
#[cfg(windows)]
pub type Stream = std::fs::File;

/// Protocol version; a daemon that answers `Version` with another number is replaced.
/// (v0.4 daemons don't know the request at all and just hang up.)
pub const VERSION: u32 = 1;

fn connect_raw() -> io::Result<Stream> {
    #[cfg(unix)]
    return Stream::connect(crate::paths::socket_path());
    #[cfg(windows)]
    return crate::pipe::connect();
}

/// Connect to the running daemon, if it speaks this protocol version. A daemon from another
/// version (an update replaced the files, but the old one still runs) is stopped, so the caller
/// can start its own; that case returns `ErrorKind::ConnectionRefused` like "none running".
pub fn connect() -> io::Result<Stream> {
    let mut c = connect_raw()?;
    match exchange(&mut c, &Request::Version) {
        Ok(Response::Version(v)) if v == VERSION => return Ok(c),
        Ok(Response::Version(_)) => {
            let _ = exchange(&mut c, &Request::Quit);
        }
        // Too old to know `Version`.
        _ => crate::process::stop_peer(&c),
    }
    drop(c);
    // Wait until it is gone (its socket or pipe no longer accepts connections).
    let start = std::time::Instant::now();
    while connect_raw().is_ok() && start.elapsed() < std::time::Duration::from_secs(3) {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Err(io::Error::new(io::ErrorKind::ConnectionRefused, "stopped a finderd from another version"))
}

/// Send `req` and wait for the reply.
pub fn exchange(c: &mut Stream, req: &Request) -> io::Result<Response> {
    req.send(c)?;
    Response::recv(c)
}

const MAX_FRAME: u32 = 256 << 20;

/// 1 was the search with full-path, hidden and exclude options (up to v0.4).
const REQ_SEARCH: u8 = 3;
const REQ_OPENED: u8 = 2;
const REQ_QUIT: u8 = 4;
const REQ_VERSION: u8 = 5;
const RESP_RESULTS: u8 = 1;
const RESP_OK: u8 = 2;
const RESP_ERROR: u8 = 3;
const RESP_VERSION: u8 = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    pub text: String,
    pub case_sensitive: bool,
    /// Maximum number of hits to return (the total count is always reported).
    pub limit: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Search(Query),
    /// The user opened this path; used for ranking.
    Opened(Vec<u8>),
    /// Exit the daemon (`finderd --uninstall`, or replaced by another version).
    Quit,
    /// Ask for the protocol version (`Response::Version`).
    Version,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub is_dir: bool,
    pub path: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Response {
    Results { total: u64, indexing: bool, hits: Vec<Hit> },
    Ok,
    Error(String),
    Version(u32),
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

#[derive(Default)]
struct Enc(Vec<u8>);

impl Enc {
    fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.u32(v.len() as u32);
        self.0.extend_from_slice(v);
        self
    }
}

struct Dec<'a>(&'a [u8]);

impl<'a> Dec<'a> {
    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        if self.0.len() < n {
            return Err(invalid("truncated message"));
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }
    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn bool(&mut self) -> io::Result<bool> {
        Ok(self.u8()? != 0)
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn bytes(&mut self) -> io::Result<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    fn string(&mut self) -> io::Result<String> {
        String::from_utf8(self.bytes()?.to_vec()).map_err(|_| invalid("invalid UTF-8"))
    }
    fn end(&self) -> io::Result<()> {
        if self.0.is_empty() { Ok(()) } else { Err(invalid("trailing bytes")) }
    }
}

fn write_frame(w: &mut impl Write, payload: &[u8]) -> io::Result<()> {
    let len =
        u32::try_from(payload.len()).ok().filter(|&n| n <= MAX_FRAME).ok_or_else(|| invalid("message too large"))?;
    let mut buf = Vec::with_capacity(4 + payload.len());
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(payload);
    w.write_all(&buf)?;
    w.flush()
}

/// Read one frame. A clean close before the frame starts yields `UnexpectedEof`.
fn read_frame(r: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len);
    if len > MAX_FRAME {
        return Err(invalid("message too large"));
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

impl Request {
    pub fn encode(&self) -> Vec<u8> {
        let mut e = Enc::default();
        match self {
            Request::Search(q) => {
                e.u8(REQ_SEARCH).bytes(q.text.as_bytes()).u8(q.case_sensitive as u8).u32(q.limit);
            }
            Request::Opened(path) => {
                e.u8(REQ_OPENED).bytes(path);
            }
            Request::Quit => {
                e.u8(REQ_QUIT);
            }
            Request::Version => {
                e.u8(REQ_VERSION);
            }
        }
        e.0
    }

    pub fn decode(buf: &[u8]) -> io::Result<Request> {
        let mut d = Dec(buf);
        let req = match d.u8()? {
            REQ_SEARCH => {
                let (text, case_sensitive, limit) = (d.string()?, d.bool()?, d.u32()?);
                Request::Search(Query { text, case_sensitive, limit })
            }
            REQ_OPENED => Request::Opened(d.bytes()?.to_vec()),
            REQ_QUIT => Request::Quit,
            REQ_VERSION => Request::Version,
            _ => return Err(invalid("unknown request (finder and finderd from different versions?)")),
        };
        d.end()?;
        Ok(req)
    }

    pub fn send(&self, w: &mut impl Write) -> io::Result<()> {
        write_frame(w, &self.encode())
    }

    pub fn recv(r: &mut impl Read) -> io::Result<Request> {
        Request::decode(&read_frame(r)?)
    }
}

impl Response {
    pub fn encode(&self) -> Vec<u8> {
        let mut e = Enc::default();
        match self {
            Response::Results { total, indexing, hits } => {
                e.u8(RESP_RESULTS).u64(*total).u8(*indexing as u8).u32(hits.len() as u32);
                for h in hits {
                    e.u8(h.is_dir as u8).bytes(&h.path);
                }
            }
            Response::Ok => {
                e.u8(RESP_OK);
            }
            Response::Version(v) => {
                e.u8(RESP_VERSION).u32(*v);
            }
            Response::Error(msg) => {
                e.u8(RESP_ERROR).bytes(msg.as_bytes());
            }
        }
        e.0
    }

    pub fn decode(buf: &[u8]) -> io::Result<Response> {
        let mut d = Dec(buf);
        let resp = match d.u8()? {
            RESP_RESULTS => {
                let total = d.u64()?;
                let indexing = d.bool()?;
                let n = d.u32()? as usize;
                let mut hits = Vec::with_capacity(n.min(1 << 16));
                for _ in 0..n {
                    hits.push(Hit { is_dir: d.bool()?, path: d.bytes()?.to_vec() });
                }
                Response::Results { total, indexing, hits }
            }
            RESP_OK => Response::Ok,
            RESP_ERROR => Response::Error(d.string()?),
            RESP_VERSION => Response::Version(d.u32()?),
            _ => return Err(invalid("unknown response")),
        };
        d.end()?;
        Ok(resp)
    }

    pub fn send(&self, w: &mut impl Write) -> io::Result<()> {
        write_frame(w, &self.encode())
    }

    pub fn recv(r: &mut impl Read) -> io::Result<Response> {
        Response::decode(&read_frame(r)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let reqs = [
            Request::Search(Query { text: "héllo".into(), case_sensitive: true, limit: 500 }),
            Request::Opened(b"/home/x".to_vec()),
            Request::Quit,
            Request::Version,
        ];
        for r in reqs {
            let mut buf = Vec::new();
            r.send(&mut buf).unwrap();
            assert_eq!(Request::recv(&mut buf.as_slice()).unwrap(), r);
        }
        let resps = [
            Response::Results {
                total: 7,
                indexing: true,
                hits: vec![Hit { is_dir: true, path: b"/a".to_vec() }, Hit { is_dir: false, path: b"/a/b".to_vec() }],
            },
            Response::Ok,
            Response::Error("boom".into()),
            Response::Version(VERSION),
        ];
        for r in resps {
            let mut buf = Vec::new();
            r.send(&mut buf).unwrap();
            assert_eq!(Response::recv(&mut buf.as_slice()).unwrap(), r);
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(Request::decode(&[9]).is_err());
        assert!(Request::decode(&[REQ_OPENED, 5, 0, 0, 0, b'a']).is_err());
        assert!(Response::decode(&[RESP_OK, 0]).is_err());
    }
}
