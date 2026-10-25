//! Black-box contracts from lab-browser.md, section 8.
#[expect(dead_code, reason = "shared process reader and JSON vocabulary expose helpers for other stories")]
#[path = "../src/json.rs"]
mod json;
#[expect(dead_code, reason = "the shared reader exposes answer helpers for capture stories")]
mod support;

use json::Value;
use skein_scratch::Scratch;
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    process::{Command, Stdio},
    time::Duration,
};
use support::{Child, Reader, field, integer, text};

struct Hand {
    scratch: Scratch,
    child: Child,
    reader: Reader,
    address: SocketAddr,
    origin: String,
    _server: Child,
}
impl Hand {
    fn start(html: &str) -> Self {
        Self::with_peers(html, &[])
    }
    fn with_peers(html: &str, peers: &[String]) -> Self {
        let scratch = Scratch::new("qa-hand");
        for dir in ["profile", "out", "www"] {
            fs::create_dir(scratch.path().join(dir)).expect("fixture directory");
        }
        fs::write(scratch.path().join("www/index.html"), html).expect("static fixture");
        fs::write(scratch.path().join("www/favicon.ico"), []).expect("favicon");
        let mut server = Child(
            Command::new("python3")
                .args(["-u", "-m", "http.server", "0", "--bind", "127.0.0.1", "--directory"])
                .arg(scratch.path().join("www"))
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("standard HTTP server"),
        );
        let mut line = String::new();
        BufReader::new(server.0.stdout.take().expect("server stdout")).read_line(&mut line).expect("server ready");
        let port = line.split(" port ").nth(1).expect("server port").split_whitespace().next().expect("port");
        let origin = format!("http://127.0.0.1:{port}");
        let stderr = scratch.path().join("stderr");
        let mut child = Child(
            Command::new(skein_scratch::paths::binary("skein-lab-browser"))
                .arg("hand")
                .arg("--chromium")
                .arg(support::chromium())
                .arg("--fonts")
                .arg(support::fonts())
                .arg("--scratch")
                .arg(scratch.path().join("profile"))
                .arg("--out")
                .arg(scratch.path().join("out"))
                .arg("--origin")
                .arg(&origin)
                .args(peers.iter().flat_map(|peer| ["--peer", peer.as_str()]))
                .args(["--size", "320x240", "--listen", "127.0.0.1:0"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(File::create(&stderr).expect("stderr"))
                .spawn()
                .expect("hand process"),
        );
        let mut reader = Reader::new(child.0.stdout.take().expect("hand stdout"), stderr);
        let ready = String::from_utf8(reader.line()).expect("ready UTF-8");
        let address: SocketAddr =
            ready.strip_prefix("listening ").expect("8.1 ready line").parse().expect("bound address");
        assert!(address.ip().is_loopback() && address.port() != 0);
        Self { scratch, child, reader, address, origin, _server: server }
    }
    fn connect(&self) -> TcpStream {
        let stream = TcpStream::connect(self.address).expect("connect hand");
        stream.set_read_timeout(Some(Duration::from_secs(28))).expect("read deadline");
        stream.set_write_timeout(Some(Duration::from_secs(6))).expect("write deadline");
        stream
    }
    fn call(&self, line: &str) -> Value {
        let mut stream = self.connect();
        writeln!(stream, "{line}").expect("send call");
        answer(stream)
    }
    fn go(&self) {
        let result = self.call(r#"{"call":"go","path":"/index.html"}"#);
        assert_eq!(text(field(&result, "went")), format!("{}/index.html", self.origin));
    }
    fn stop(&mut self, signal: &str) {
        assert!(
            Command::new("kill")
                .args([signal, &self.child.0.id().to_string()])
                .status()
                .expect("signal hand")
                .success()
        );
        let deadline = self.reader.now().saturating_add(skein_lib::Duration::from_secs(10));
        loop {
            if let Some(status) = self.child.0.try_wait().expect("wait hand") {
                assert!(status.success(), "8.1: {status}; {}", self.reader.stderr());
                return;
            }
            assert!(self.reader.now() < deadline, "8.1 shutdown deadline; {}", self.reader.stderr());
            self.reader.pause();
        }
    }
}
fn answer(stream: TcpStream) -> Value {
    let mut reader = BufReader::new(stream);
    let mut line = Vec::new();
    reader.read_until(b'\n', &mut line).expect("answer before deadline");
    assert_eq!(line.pop(), Some(b'\n'), "8.2 answer newline");
    let result = json::parse(&line).expect("answer JSON");
    assert_eq!(json::encode(&result), line, "canonical answer");
    let mut rest = Vec::new();
    reader.read_to_end(&mut rest).expect("8.2 closes after one answer");
    assert!(rest.is_empty(), "8.2 one answer a connection");
    result
}
fn refused(answer: &Value, why: &str) {
    assert_eq!(text(field(answer, "refused")), why, "{answer:?}");
}

#[test]
fn ready_blank_address_and_both_shutdown_signals() {
    for signal in ["-TERM", "-INT"] {
        let mut hand = Hand::start("<h1>Fixture</h1>");
        assert_eq!(text(field(&hand.call(r#"{"call":"address"}"#), "address")), "about:blank");
        hand.stop(signal);
    }
}

#[test]
fn one_answer_even_when_two_lines_are_sent() {
    let mut hand = Hand::start("<h1>Fixture</h1>");
    let mut stream = hand.connect();
    stream.write_all(b"{\"call\":\"address\"}\n{\"call\":\"shot\"}\n").expect("two lines");
    assert_eq!(text(field(&answer(stream), "address")), "about:blank");
    assert!(!hand.scratch.path().join("out/shots/0001.png").exists());
    hand.stop("-TERM");
}

#[test]
fn incomplete_connection_expires_and_preserves_accept_order() {
    let mut hand = Hand::start("<h1>Fixture</h1>");
    let mut first = hand.connect();
    first.write_all(b"{\"call\":").expect("partial line");
    let start = hand.reader.now();
    let mut second = hand.connect();
    second.write_all(b"{\"call\":\"address\"}\n").expect("queued call");
    assert_eq!(text(field(&answer(second), "address")), "about:blank");
    assert!(
        hand.reader.now() >= start.saturating_add(skein_lib::Duration::from_secs(9)),
        "8.2 accepted order bypassed"
    );
    assert!(hand.reader.now() < start.saturating_add(skein_lib::Duration::from_secs(13)), "8.2 ten second bound");
    let mut bytes = Vec::new();
    first.read_to_end(&mut bytes).expect("expired connection closed");
    assert!(bytes.is_empty(), "8.2 incomplete line unanswered");
    hand.stop("-TERM");
}

#[test]
fn line_limit_has_an_answer_at_boundary_and_none_past_it() {
    let mut hand = Hand::start("<h1>Fixture</h1>");
    for size in [128 * 1024, 128 * 1024 + 1] {
        let mut stream = hand.connect();
        let prefix = r#"{"call":"unknown","padding":""#;
        let line = format!("{prefix}{}\"}}\n", "x".repeat(size - prefix.len() - 2));
        assert_eq!(line.len(), size + 1, "JSON bytes plus LF delimiter");
        stream.write_all(line.as_bytes()).expect("bounded line");
        if size <= 128 * 1024 {
            refused(&answer(stream), "call");
        } else {
            let mut bytes = Vec::new();
            match stream.read_to_end(&mut bytes) {
                Ok(_) => {}
                Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::ConnectionReset),
            }
            assert!(bytes.is_empty(), "8.2 oversized line unanswered");
        }
    }
    assert_eq!(text(field(&hand.call(r#"{"call":"address"}"#), "address")), "about:blank");
    hand.stop("-TERM");
}

#[test]
fn capture_calls_schema_roles_and_paths_are_refused() {
    let mut hand = Hand::start("<h1>Fixture</h1>");
    for call in ["serve", "open", "tree", "select", "style", "close", "evaluate"] {
        refused(&hand.call(&format!(r#"{{"call":"{call}"}}"#)), "call");
    }
    for line in [
        r#"{"call":"address","extra":true}"#,
        r#"{"call":"key","key":"Delete","shift":false}"#,
        r#"{"call":"press","in":[],"match":"exact","name":"Fixture","role":"invented"}"#,
        r#"{"call":"wait","gone":false,"in":[],"match":"exact","name":"Fixture","role":"heading","seconds":0}"#,
        r#"{"call":"wait","gone":false,"in":[],"match":"exact","name":"Fixture","role":"heading","seconds":301}"#,
    ] {
        refused(&hand.call(line), "call");
    }
    for path in ["//127.0.0.1/", "http://127.0.0.1/", "/a b", "/a\nb", "/café"] {
        let request = json::object([("call", json::string("go")), ("path", json::string(path))]);
        refused(&hand.call(std::str::from_utf8(&json::encode(&request)).expect("JSON")), "origin");
    }
    assert_eq!(text(field(&hand.call(r#"{"call":"address"}"#), "address")), "about:blank");
    hand.stop("-TERM");
}

#[test]
fn replacement_typing_scopes_waits_choices_and_history() {
    let mut hand = Hand::start(
        r#"<main aria-label="Draft"><h1>Fixture</h1><input aria-label="Title" value="old words"><button disabled>Save</button><input type="checkbox" aria-label="Consent"><select aria-label="Theme"><option>Light</option><option>Dark</option></select><a href="/index.html?second">Next</a></main>"#,
    );
    hand.go();
    let typed = hand.call(r#"{"call":"type","in":[{"match":"exact","name":"Draft","role":"main"}],"match":"exact","name":"Title","role":"text field","text":"new"}"#);
    assert_eq!(text(field(field(&typed, "typed"), "value")), "new");
    refused(&hand.call(r#"{"call":"press","in":[],"match":"exact","name":"Title","role":"text field"}"#), "takes");
    refused(&hand.call(r#"{"call":"press","in":[],"match":"exact","name":"Save","role":"button"}"#), "disabled");
    let missed = hand
        .call(r#"{"call":"wait","gone":false,"in":[],"match":"exact","name":"Absent","role":"button","seconds":1}"#);
    assert!(matches!(field(&missed, "missed"), Value::Array(parts) if parts.len() <= 8));
    let gone =
        hand.call(r#"{"call":"wait","gone":true,"in":[],"match":"exact","name":"Absent","role":"button","seconds":1}"#);
    assert_eq!(field(&gone, "gone"), &Value::Bool(true));
    let keyed = hand.call(r#"{"call":"key","key":"Tab","shift":false}"#);
    assert_eq!(text(field(&keyed, "keyed")), "Tab");
    let checked = hand.call(r#"{"call":"press","in":[],"match":"exact","name":"Consent","role":"check"}"#);
    let _ = field(&checked, "pressed");
    let outline = hand.call(r#"{"call":"outline","in":[]}"#);
    assert!(
        text(field(&outline, "outline")).lines().any(|line| line.contains("Consent") && line.contains("[checked]")),
        "8.3 check toggles: {outline:?}"
    );
    let chosen = hand.call(r#"{"call":"choose","in":[],"match":"exact","name":"Theme","option":{"match":"exact","name":"Dark"},"role":"select"}"#);
    let _ = field(&chosen, "chose");
    let met = hand
        .call(r#"{"call":"wait","gone":false,"in":[],"match":"exact","name":"Fixture","role":"heading","seconds":1}"#);
    assert_eq!(text(field(field(&met, "met"), "name")), "Fixture");
    let outline = hand.call(r#"{"call":"outline","in":[]}"#);
    assert!(text(field(&outline, "outline")).contains("heading 1 \"Fixture\""));
    let next = hand.call(r#"{"call":"go","path":"/index.html?second"}"#);
    assert!(text(field(&next, "went")).ends_with("?second"));
    for (to, suffix) in [("back", "/index.html"), ("forward", "?second"), ("reload", "?second")] {
        let result = hand.call(&format!(r#"{{"call":"history","to":"{to}"}}"#));
        assert!(text(field(&result, "went")).ends_with(suffix), "8.3 {result:?}");
    }
    hand.stop("-TERM");
}

#[test]
fn another_loopback_port_is_never_reached_by_fetch_frame_or_link() {
    let forbidden = TcpListener::bind("127.0.0.1:0").expect("canary listener");
    forbidden.set_nonblocking(true).expect("nonblocking canary");
    let address = forbidden.local_addr().expect("canary address");
    let mut hand = Hand::start(&format!(
        r#"<h1>Fixture</h1><img src="http://{address}/image"><iframe src="http://{address}/frame"></iframe><a href="http://{address}/link">Outside</a><script>fetch('http://{address}/fetch').catch(()=>{{}})</script>"#
    ));
    hand.go();
    let pressed = hand.call(r#"{"call":"press","in":[],"match":"exact","name":"Outside","role":"link"}"#);
    assert!(matches!(field(&pressed, "pressed"), Value::Object(_)));
    let trouble = hand.call(r#"{"call":"trouble"}"#);
    let Value::Array(entries) = field(&trouble, "trouble") else { panic!("{trouble:?}") };
    assert!(
        entries.iter().any(|entry| text(field(entry, "kind")) == "network"
            && text(field(entry, "text")).contains(&address.port().to_string())),
        "8.5: {trouble:?}"
    );
    assert_eq!(forbidden.accept().expect_err("8.5 another port never reached").kind(), std::io::ErrorKind::WouldBlock);
    hand.stop("-TERM");
}

#[test]
fn a_named_peer_is_reached_and_any_other_port_is_still_refused() {
    let peer = TcpListener::bind("127.0.0.1:0").expect("peer listener");
    peer.set_nonblocking(true).expect("nonblocking peer");
    let peer_address = peer.local_addr().expect("peer address");
    let forbidden = TcpListener::bind("127.0.0.1:0").expect("canary listener");
    forbidden.set_nonblocking(true).expect("nonblocking canary");
    let forbidden_address = forbidden.local_addr().expect("canary address");
    let mut hand = Hand::with_peers(
        &format!(
            r#"<h1>Fixture</h1><a href="http://{peer_address}/link">Peer</a><script>fetch('http://{forbidden_address}/fetch').catch(()=>{{}})</script>"#
        ),
        &[format!("http://{peer_address}")],
    );
    hand.go();
    let pressed = hand.call(r#"{"call":"press","in":[],"match":"exact","name":"Peer","role":"link"}"#);
    let _ = field(&pressed, "pressed");
    let deadline = hand.reader.now().saturating_add(skein_lib::Duration::from_secs(6));
    let mut stream = loop {
        match peer.accept() {
            Ok((stream, _)) => break stream,
            Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock),
        }
        assert!(hand.reader.now() < deadline, "8.5 named peer reached");
        hand.reader.pause();
    };
    stream.set_read_timeout(Some(Duration::from_secs(6))).expect("peer read deadline");
    let mut request = String::new();
    BufReader::new(&stream).read_line(&mut request).expect("peer request");
    assert_eq!(request, "GET /link HTTP/1.1\r\n");
    let body = "<h1>Named peer</h1>";
    write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
        .expect("peer response");
    let met = hand.call(
        r#"{"call":"wait","gone":false,"in":[],"match":"exact","name":"Named peer","role":"heading","seconds":6}"#,
    );
    let _ = field(&met, "met");
    let trouble = hand.call(r#"{"call":"trouble"}"#);
    let Value::Array(entries) = field(&trouble, "trouble") else { panic!("{trouble:?}") };
    assert!(
        entries.iter().any(|entry| text(field(entry, "kind")) == "network"
            && text(field(entry, "text")).contains(&forbidden_address.port().to_string())),
        "8.5: {trouble:?}"
    );
    assert!(
        !entries.iter().any(|entry| text(field(entry, "kind")) == "network"
            && text(field(entry, "text")).contains(&peer_address.port().to_string())),
        "8.5: {trouble:?}"
    );
    assert_eq!(forbidden.accept().expect_err("8.5 unnamed port never reached").kind(), std::io::ErrorKind::WouldBlock);
    hand.stop("-TERM");
}

#[test]
fn invented_sign_in_queries_are_redacted_everywhere_and_link_slots_are_bounded() {
    let mut hand =
        Hand::start(r"<h1>Fixture</h1><script>console.error(location.search);throw Error(location.href)</script>");
    for link in [format!("{}/index.html?{}", hand.origin, "q".repeat(15)), format!("{}/index.html", hand.origin)] {
        let request = json::object([("call", json::string("sign-in")), ("link", json::string(link))]);
        refused(&hand.call(std::str::from_utf8(&json::encode(&request)).expect("JSON")), "call");
    }
    let other = TcpListener::bind("127.0.0.1:0").expect("other origin");
    let request = json::object([
        ("call", json::string("sign-in")),
        ("link", json::string(format!("http://{}/?{}", other.local_addr().expect("bound address"), "q".repeat(16)))),
    ]);
    refused(&hand.call(std::str::from_utf8(&json::encode(&request)).expect("JSON")), "origin");
    let mut secrets = Vec::new();
    for n in 0..4 {
        let query = if n == 0 { "q".repeat(16) } else { format!("invented-hand-credential-{n:04}") };
        let link = format!("{}/index.html?{query}", hand.origin);
        let request = json::object([("call", json::string("sign-in")), ("link", json::string(link))]);
        let opened = hand.call(std::str::from_utf8(&json::encode(&request)).expect("JSON"));
        assert!(text(field(&opened, "opened")).contains("<link>"));
        secrets.push(query);
        for call in [r#"{"call":"address"}"#, r#"{"call":"outline","in":[]}"#, r#"{"call":"trouble"}"#] {
            let result = json::encode(&hand.call(call));
            for secret in &secrets {
                assert!(!result.windows(secret.len()).any(|w| w == secret.as_bytes()), "8.6 answer redaction");
            }
        }
    }
    for query in ["short", "invented-hand-credential-fifth"] {
        let request = json::object([
            ("call", json::string("sign-in")),
            ("link", json::string(format!("{}/index.html?{query}", hand.origin))),
        ]);
        refused(&hand.call(std::str::from_utf8(&json::encode(&request)).expect("JSON")), "call");
    }
    hand.stop("-TERM");
    let mut paths = vec![hand.scratch.path().join("stderr")];
    paths.extend(
        fs::read_dir(hand.scratch.path().join("out"))
            .expect("out")
            .map(|e| e.expect("file").path())
            .filter(|p| p.is_file()),
    );
    for path in paths {
        let bytes = fs::read(path).expect("evidence");
        for secret in &secrets {
            assert!(!bytes.windows(secret.len()).any(|w| w == secret.as_bytes()), "8.6 evidence redaction");
        }
    }
}

#[test]
fn trouble_is_bounded_drained_and_logged_as_canonical_records() {
    let mut hand = Hand::start(r"<h1>Fixture</h1><script>for(let i=0;i<90;i++)console.error('qa-trouble-'+i)</script>");
    hand.go();
    let first = hand.call(r#"{"call":"trouble"}"#);
    let Value::Array(entries) = field(&first, "trouble") else { panic!("{first:?}") };
    assert!(entries.len() <= 64, "8.7: {first:?}");
    assert!(integer(field(&first, "more")) > 0, "8.7 omitted entries counted: {first:?}");
    let second = hand.call(r#"{"call":"trouble"}"#);
    let Value::Array(entries) = field(&second, "trouble") else { panic!("{second:?}") };
    assert!(!entries.iter().any(|e| text(field(e, "text")).contains("qa-trouble-")), "8.7 drain: {second:?}");
    hand.stop("-TERM");
    let log = fs::read_to_string(hand.scratch.path().join("out/trouble.jsonl")).expect("8.7 live trouble log");
    assert!(!log.is_empty());
    for (index, line) in log.lines().enumerate() {
        let record = json::parse(line.as_bytes()).expect("trouble JSON");
        assert_eq!(json::encode(&record), line.as_bytes());
        assert_eq!(integer(field(&record, "n")), i64::try_from(index + 1).expect("entry number"));
        for key in ["kind", "text", "address", "call", "during"] {
            let _ = field(&record, key);
        }
    }
}

#[test]
fn browser_death_ends_hand_with_diagnostic_and_status_one() {
    let mut hand = Hand::start("<h1>Fixture</h1>");
    // The kernel lists this process's own children; no process environment is read.
    let pid = hand.child.0.id();
    let children = fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")).expect("hand child PIDs");
    assert!(!children.trim().is_empty(), "8.1 spawned browser");
    for child in children.split_whitespace() {
        assert!(Command::new("kill").args(["-KILL", child]).status().expect("kill browser").success());
    }
    let deadline = hand.reader.now().saturating_add(skein_lib::Duration::from_secs(10));
    loop {
        if let Some(status) = hand.child.0.try_wait().expect("wait hand") {
            assert_eq!(status.code(), Some(1), "8.1 browser death status");
            assert!(!hand.reader.stderr().trim().is_empty(), "8.1 browser death diagnostic");
            break;
        }
        assert!(hand.reader.now() < deadline, "8.1 browser death must end hand");
        hand.reader.pause();
    }
}

#[test]
fn trouble_log_stops_at_4096_entries_and_one_overflow_record() {
    let mut hand = Hand::start(
        r"<h1>Loading</h1><script>
let n=0;let timer=setInterval(()=>{for(let j=0;j<8;j++)console.error('bounded-trouble-'+n++);
if(n>=4200){clearInterval(timer);document.querySelector('h1').textContent='Finished'}},10)
</script>",
    );
    hand.go();
    let result = hand.call(
        r#"{"call":"wait","gone":false,"in":[],"match":"exact","name":"Finished","role":"heading","seconds":20}"#,
    );
    let _ = field(&result, "met");
    hand.stop("-TERM");
    let log = fs::read_to_string(hand.scratch.path().join("out/trouble.jsonl")).expect("trouble log");
    let records: Vec<_> = log.lines().collect();
    assert_eq!(records.len(), 4097, "8.7: 4096 entries plus one last overflow entry");
    let last = json::parse(records.last().expect("overflow record").as_bytes()).expect("overflow JSON");
    assert!(
        !text(field(&last, "text")).contains("bounded-trouble-"),
        "8.7 last entry counts entries not written: {last:?}"
    );
}

#[test]
fn complete_calls_keep_accept_order_after_a_partial_first_call() {
    let mut hand = Hand::start("<h1>Fixture</h1>");
    let mut first = hand.connect();
    first.write_all(b"{\"call\":").expect("partial first call");
    let mut second = hand.connect();
    second.write_all(b"{\"call\":\"shot\"}\n").expect("complete second call");
    first.write_all(b"\"shot\"}\n").expect("complete first call");
    let first = answer(first);
    let second = answer(second);
    assert_eq!(text(field(field(&first, "shot"), "file")), "shots/0001.png", "8.2 accepted order");
    assert_eq!(text(field(field(&second, "shot"), "file")), "shots/0002.png", "8.2 accepted order");
    hand.stop("-TERM");
}

#[test]
fn viewport_shots_are_numbered_pngs_with_matching_metadata() {
    let mut hand = Hand::start("<h1>Fixture</h1>");
    for n in 1..=2 {
        let result = hand.call(r#"{"call":"shot"}"#);
        let shot = field(&result, "shot");
        let file = format!("shots/{n:04}.png");
        assert_eq!(text(field(shot, "file")), file);
        assert_eq!(integer(field(shot, "width")), 320);
        assert_eq!(integer(field(shot, "height")), 240);
        let bytes = fs::read(hand.scratch.path().join("out").join(file)).expect("shot evidence");
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        let digest = Command::new("sha256sum")
            .arg(hand.scratch.path().join("out").join(text(field(shot, "file"))))
            .output()
            .expect("independent shot digest");
        assert!(digest.status.success());
        let digest = String::from_utf8(digest.stdout).expect("hex digest");
        assert_eq!(text(field(shot, "sha256")), digest.split_whitespace().next().expect("digest"));
        assert_eq!(integer(field(shot, "bytes")), i64::try_from(bytes.len()).expect("shot size"));
    }
    hand.stop("-TERM");
}
