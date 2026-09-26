//! The language server is a program an editor talks to, so this drives it the
//! way an editor does: spawn the binary, speak JSON-RPC over its stdin and
//! stdout, and read what comes back.
//!
//! `cargo test -p cypcb-lsp --test the_language_server_answers`
//!
//! Nothing had ever run this crate. `server` was an off-by-default feature, so
//! `cargo build`, `cargo test` and the quality gate all compiled the crate with
//! `backend.rs` cfg'd out - 2,984 lines of language server that no command in
//! the repository touched. Asking for it directly said why:
//!
//! ```text
//! error[E0195]: lifetime parameters or bounds on method `initialize` do not
//!               match the trait declaration
//! error: could not compile `cypcb-lsp` (lib) due to 10 previous errors
//! ```
//!
//! One `#[async_trait::async_trait]` against a `tower-lsp` that takes native
//! async methods. The server is on by default now, which is what keeps this
//! test - and the compiler - pointed at it.

#![cfg(feature = "server")]

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// A board with two parts on one net, small enough to read in a failure.
const BOARD: &str = r#"version 1

board probe {
    size 20mm x 20mm
    layers 2
}

component R1 resistor "0402" {
    value "10k"
    at 5mm, 5mm
}

component R2 resistor "0402" {
    value "1k"
    at 12mm, 5mm
}

net SIG {
    R1.2
    R2.1
}
"#;

/// The same board with `size` given a unit the language does not have.
const BROKEN: &str = r#"version 1

board probe {
    size 20furlongs x 20mm
    layers 2
}
"#;

/// Where a substring starts, as a zero-based LSP line and character.
///
/// Written this way so a hover position survives an edit to the board above -
/// a hard-coded line number would make this test fail for the wrong reason.
fn position_of(source: &str, needle: &str) -> (u32, u32) {
    let offset = source.find(needle).expect("the needle is in the source");
    let before = &source[..offset];
    let line = before.matches('\n').count() as u32;
    let column = before.rsplit('\n').next().map_or(0, str::len) as u32;
    (line, column)
}

/// One end of an LSP connection: the child process and its message stream.
struct Server {
    child: Child,
    stdin: ChildStdin,
    messages: Receiver<Value>,
    next_id: i64,
}

impl Server {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_cypcb-lsp"))
            .arg("--stdio")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the language server binary runs");

        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");

        // A reader thread, so a server that answers nothing times out here
        // instead of hanging the suite.
        let (tx, messages) = channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Some(message) = read_message(&mut reader) {
                if tx.send(message).is_err() {
                    break;
                }
            }
        });

        Server {
            child,
            stdin,
            messages,
            next_id: 1,
        }
    }

    fn send(&mut self, message: Value) {
        let body = serde_json::to_string(&message).expect("the message serializes");
        write!(self.stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body)
            .expect("the server is still listening");
        self.stdin.flush().expect("the write reaches the server");
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        self.wait_for(&format!("a response to {method}"), |message| {
            message.get("id").and_then(Value::as_i64) == Some(id)
        })
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    /// Read messages until one matches, or give up after five seconds.
    fn wait_for(&self, what: &str, matches: impl Fn(&Value) -> bool) -> Value {
        let deadline = Duration::from_secs(5);
        let mut seen: Vec<String> = Vec::new();
        loop {
            match self.messages.recv_timeout(deadline) {
                Ok(message) => {
                    if matches(&message) {
                        return message;
                    }
                    seen.push(
                        message
                            .get("method")
                            .and_then(Value::as_str)
                            .unwrap_or("(a response)")
                            .to_string(),
                    );
                }
                Err(RecvTimeoutError::Timeout) => {
                    panic!("waited 5s for {what}; the server sent {seen:?}")
                }
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("the server exited before sending {what}; it sent {seen:?}")
                }
            }
        }
    }

    fn initialize(&mut self) -> Value {
        self.initialize_with(json!({}))
    }

    /// Initialize as a client with these capabilities.
    fn initialize_with(&mut self, capabilities: Value) -> Value {
        let result = self.request(
            "initialize",
            json!({"capabilities": capabilities, "processId": Value::Null, "rootUri": Value::Null}),
        );
        self.notify("initialized", json!({}));
        result
    }

    fn open(&mut self, uri: &str, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument": {
                "uri": uri,
                "languageId": "cypcb",
                "version": 1,
                "text": text,
            }}),
        );
    }

    fn diagnostics_for(&self, uri: &str) -> Vec<Value> {
        let message = self.wait_for(&format!("diagnostics for {uri}"), |message| {
            message.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
                && message.pointer("/params/uri").and_then(Value::as_str) == Some(uri)
        });
        message
            .pointer("/params/diagnostics")
            .and_then(Value::as_array)
            .cloned()
            .expect("a diagnostics notification carries a list")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Read one `Content-Length`-framed JSON-RPC message, or `None` at end of stream.
fn read_message(reader: &mut BufReader<impl Read>) -> Option<Value> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length: ") {
            length = value.parse::<usize>().ok();
        }
    }

    let mut body = vec![0u8; length?];
    reader.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

#[test]
fn it_starts_and_says_what_it_can_do() {
    let mut server = Server::start();
    let result = server.initialize();

    assert_eq!(
        result
            .pointer("/result/serverInfo/name")
            .and_then(Value::as_str),
        Some("cypcb-lsp"),
        "the server has to name itself in its initialize result: {result}"
    );
    // Each of these is a request the editor will only send if the server
    // advertises it, and each has an implementation in this crate.
    for capability in ["hoverProvider", "completionProvider", "definitionProvider"] {
        assert!(
            result
                .pointer(&format!("/result/capabilities/{capability}"))
                .is_some(),
            "{capability} is implemented and has to be advertised: {result}"
        );
    }
}

/// And it advertises nothing it has not implemented, which is the half a
/// source grep cannot see.
///
/// `the_matrix_is_honest_about_us` decides what the comparison matrix may claim
/// about this server by looking for `<name>_provider: Some(` in `backend.rs`.
/// A field set in that struct and a capability the server answers with are not
/// the same thing, and the difference is how K011 stayed false for five days: a
/// line that exists is not a line that runs. Four of the seven capabilities
/// that test knows about have no implementation here, so the wire is asked
/// about them directly. The day one of them is written into `initialize`, this
/// case fails and whoever wrote it has to make the server answer the request an
/// editor will now send.
#[test]
fn it_advertises_nothing_it_has_not_implemented() {
    let mut server = Server::start();
    let result = server.initialize();

    for capability in [
        "referencesProvider",
        "renameProvider",
        "documentFormattingProvider",
        "semanticTokensProvider",
    ] {
        assert!(
            result
                .pointer(&format!("/result/capabilities/{capability}"))
                .is_none(),
            "{capability} has no implementation in this crate, so the server must not \
             advertise it - an editor that sees it will send the request: {result}"
        );
    }
}

#[test]
fn a_file_it_cannot_read_comes_back_as_a_diagnostic() {
    let uri = "file:///virtual/lsp-probe/broken.cypcb";
    let mut server = Server::start();
    server.initialize();
    server.open(uri, BROKEN);

    let diagnostics = server.diagnostics_for(uri);
    assert!(
        !diagnostics.is_empty(),
        "a board measured in furlongs has to produce a diagnostic"
    );

    let (line, _) = position_of(BROKEN, "20furlongs");
    let on_that_line = diagnostics
        .iter()
        .any(|d| d.pointer("/range/start/line").and_then(Value::as_u64) == Some(u64::from(line)));
    assert!(
        on_that_line,
        "the diagnostic has to point at line {line}, where the bad unit is: {diagnostics:?}"
    );
    assert!(
        diagnostics
            .iter()
            .all(|d| d.get("severity").is_some() && d.get("message").is_some()),
        "an editor draws severity and message, so both have to be there: {diagnostics:?}"
    );
}

#[test]
fn a_board_being_written_is_not_an_error() {
    // The other half: a server that answered "broken" to everything would pass
    // the test above. This board parses; its pins are unconnected and unrouted
    // because that is what a board looks like while somebody is writing it,
    // and every one of those came back at LSP severity 1 - an error.
    let uri = "file:///virtual/lsp-probe/good.cypcb";
    let mut server = Server::start();
    server.initialize();
    server.open(uri, BOARD);

    let diagnostics = server.diagnostics_for(uri);
    let errors: Vec<&Value> = diagnostics
        .iter()
        .filter(|d| d.get("severity").and_then(Value::as_u64) == Some(1))
        .collect();
    assert!(
        errors.is_empty(),
        "an unfinished board has nothing an editor should paint red: {errors:?}"
    );
    assert!(
        !diagnostics.is_empty(),
        "the unconnected pins still have to be reported, as warnings"
    );
}

#[test]
fn a_diagnostic_points_at_the_part_it_names() {
    // Every DRC violation arrived at line 0, character 0, because
    // `DrcViolation::source_span` is `None` in every constructor in the DRC
    // crate. Twenty parts meant twenty squiggles stacked on the first
    // character of the file, none of them where the part was written.
    let uri = "file:///virtual/lsp-probe/where.cypcb";
    let mut server = Server::start();
    server.initialize();
    server.open(uri, BOARD);

    let diagnostics = server.diagnostics_for(uri);
    let (r2_line, _) = position_of(BOARD, "R2 resistor");

    let about_r2: Vec<&Value> = diagnostics
        .iter()
        .filter(|d| {
            d.get("message")
                .and_then(Value::as_str)
                .is_some_and(|m| m.contains("R2."))
        })
        .collect();
    assert!(
        !about_r2.is_empty(),
        "R2 has an unconnected pin, so something has to say so: {diagnostics:?}"
    );
    for diagnostic in &about_r2 {
        assert_eq!(
            diagnostic
                .pointer("/range/start/line")
                .and_then(Value::as_u64),
            Some(u64::from(r2_line)),
            "a diagnostic about R2 belongs on the line R2 is declared: {diagnostic}"
        );
    }
}

#[test]
fn hovering_a_component_explains_it() {
    let uri = "file:///virtual/lsp-probe/hover.cypcb";
    let mut server = Server::start();
    server.initialize();
    server.open(uri, BOARD);

    // The refdes of the second part, which is where a user points to ask what
    // R2 is.
    let (line, character) = position_of(BOARD, "R2 resistor");
    let result = server.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
        }),
    );

    let contents =
        serde_json::to_string(result.pointer("/result/contents").unwrap_or(&Value::Null))
            .expect("hover contents serialize");
    assert!(
        contents.contains("R2"),
        "hovering R2 has to say something about R2: {result}"
    );
    assert!(
        contents.contains("1k"),
        "and the value is the thing a reader is looking for: {result}"
    );
}

/// A board whose net says what it carries, so the card has a number to work.
const CARRIES_A_CURRENT: &str = r#"version 1

board probe {
    size 20mm x 20mm
    layers 2
}

component R1 resistor "0402" {
    value "10k"
    at 5mm, 5mm
}

component R2 resistor "0402" {
    value "1k"
    at 12mm, 5mm
}

net POWER [current 1A] {
    R1.1
    R2.1
}

trace POWER {
    from R1.1
    to R2.1
    layer Top
    width 0.1mm
}
"#;

#[test]
fn hovering_a_current_gives_the_width_the_checker_would_demand() {
    // One number, two surfaces. The card and the report both come from
    // `TraceWidthCalculator` with the same defaults - an outer layer, 1oz of
    // copper and a 10C rise - and IPC-2221 puts 1A at **0.300mm**, worked by
    // hand in `cypcb-calc`'s own test. The card rounds to two decimals; the
    // report prints three.
    //
    // Nothing held the two together. A hover that quoted a different width
    // from the checker would send a designer to widen a trace the checker
    // then still refuses, or to leave one it will.
    let uri = "file:///virtual/lsp-probe/current.cypcb";
    let mut server = Server::start();
    server.initialize();
    server.open(uri, CARRIES_A_CURRENT);

    let (line, character) = position_of(CARRIES_A_CURRENT, "current 1A");
    let result = server.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
        }),
    );

    let contents =
        serde_json::to_string(result.pointer("/result/contents").unwrap_or(&Value::Null))
            .expect("hover contents serialize");
    assert!(
        contents.contains("IPC-2221 width: 0.30mm"),
        "the card has to state the width the standard asks for: {result}"
    );

    // And the command line says the same thing about the same board.
    let dir = std::env::temp_dir().join(format!("cypcb-lsp-current-card-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a place to work");
    let board = dir.join("board.cypcb");
    std::fs::write(&board, CARRIES_A_CURRENT).expect("the board is writable");

    // The checker lives beside this server in the same target directory:
    // `CARGO_BIN_EXE_*` is only set for the crate that builds the binary, and
    // that crate is `cypcb-cli`.
    let checker = std::path::Path::new(env!("CARGO_BIN_EXE_cypcb-lsp")).with_file_name("cypcb");
    assert!(
        checker.exists(),
        "the checker is built beside the server: {}",
        checker.display()
    );
    let output = std::process::Command::new(&checker)
        .arg("check")
        .arg(&board)
        .output()
        .expect("the binary runs");
    let said = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(
        said.contains("IPC-2221 wants 0.300mm"),
        "the checker states the same figure to three decimals:\n{said}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn completion_works_on_the_half_typed_line_it_is_asked_about() {
    // The moment completion is for: the user has typed `component R3 resistor "`
    // and stopped. The file does not parse - it cannot, the string is open and
    // the block is unclosed - and that is precisely when the list of footprints
    // is worth having.
    let uri = "file:///virtual/lsp-probe/typing.cypcb";
    let mut typing = String::from(BOARD);
    typing.push_str("\ncomponent R3 resistor \"");

    let mut server = Server::start();
    server.initialize();
    server.open(uri, &typing);

    let (line, character) = position_of(&typing, "component R3 resistor \"");
    let result = server.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": uri},
            "position": {
                "line": line,
                "character": character + "component R3 resistor \"".len() as u32,
            },
        }),
    );

    let labels: Vec<&str> = result
        .pointer("/result")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("label").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        labels.contains(&"0402"),
        "a footprint list is what the cursor is asking for here: {result}"
    );
}

#[test]
fn completion_inside_a_block_nobody_has_closed_yet_offers_its_properties() {
    // The other half of the same defect: the block is open, so the parser has
    // no component to return, so the cursor read as "after the last
    // definition" and the editor offered `version` and `board` inside a
    // component body.
    let uri = "file:///virtual/lsp-probe/open-block.cypcb";
    let mut typing = String::from(BOARD);
    typing.push_str("\ncomponent R3 resistor \"0402\" {\n    ");

    let mut server = Server::start();
    server.initialize();
    server.open(uri, &typing);

    let line = typing.matches('\n').count() as u32;
    let result = server.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": 4},
        }),
    );

    let labels: Vec<&str> = result
        .pointer("/result")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("label").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        labels.contains(&"value") && labels.contains(&"at"),
        "inside a component body the properties are what to offer: {labels:?}"
    );
    assert!(
        !labels.contains(&"board"),
        "and `board` inside a component body pastes a board into it: {labels:?}"
    );
}

#[test]
fn going_to_a_definition_lands_on_the_part() {
    let uri = "file:///virtual/lsp-probe/goto.cypcb";
    let mut server = Server::start();
    server.initialize();
    server.open(uri, BOARD);

    // Ctrl-click on `R1.2` inside the net block. The answer has to be where R1
    // is declared, which is the whole point of the request.
    let (line, character) = position_of(BOARD, "R1.2");
    let result = server.request(
        "textDocument/definition",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character + 1},
        }),
    );

    let (declared_line, _) = position_of(BOARD, "component R1");
    assert_eq!(
        result.pointer("/result/uri").and_then(Value::as_str),
        Some(uri),
        "the definition is in the same file: {result}"
    );
    assert_eq!(
        result
            .pointer("/result/range/start/line")
            .and_then(Value::as_u64),
        Some(u64::from(declared_line)),
        "R1.2 has to lead to the line `component R1` is on: {result}"
    );
}

#[test]
fn completing_a_footprint_offers_the_library() {
    let uri = "file:///virtual/lsp-probe/complete.cypcb";
    let mut server = Server::start();
    server.initialize();
    server.open(uri, BOARD);

    // Inside the quoted footprint of R1, where a user asks what they may type.
    let (line, character) = position_of(BOARD, "0402\"");
    let result = server.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
        }),
    );

    let items = result
        .pointer("/result")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        !items.is_empty(),
        "the built-in footprint library is not empty, so neither is this: {result}"
    );

    let labels: Vec<&str> = items
        .iter()
        .filter_map(|item| item.get("label").and_then(Value::as_str))
        .collect();
    assert!(
        labels.contains(&"0402"),
        "the library ships a 0402 and the list has to carry it: {labels:?}"
    );
    assert!(
        items
            .iter()
            .all(|item| item.get("kind").is_some() && item.get("detail").is_some()),
        "an editor sorts by kind and shows the detail, so both have to be there: {items:?}"
    );
}

#[test]
fn a_semantic_error_reaches_the_editor() {
    // The editor was told about parse errors and DRC violations and nothing
    // else. Everything `SyncError` reports - an unknown footprint, a duplicate
    // refdes, a net naming a pin the part does not have, a module pin left
    // unconnected, a broken interface contract - was collected by the sync and
    // dropped on the floor, so `cypcb check` refused a file the editor called
    // clean.
    let mut server = Server::start();
    server.initialize();

    let uri = "file:///unknown-footprint.cypcb";
    server.open(
        uri,
        "version 1\n\nboard b {\n    size 20mm x 20mm\n    layers 2\n}\n\ncomponent R1 resistor \"NOSUCHFOOTPRINT\" {\n    at 5mm, 5mm\n}\n",
    );

    let diagnostics = server.diagnostics_for(uri);
    let messages: Vec<String> = diagnostics
        .iter()
        .filter_map(|d| d.get("message").and_then(Value::as_str))
        .map(str::to_string)
        .collect();

    assert!(
        messages.iter().any(|m| m.contains("NOSUCHFOOTPRINT")),
        "the footprint does not exist and the editor said nothing about it: {messages:?}"
    );
}

#[test]
fn a_design_that_imports_its_blocks_is_understood() {
    // `import Divider, LedDriver from "lib/blocks.cypcb"` is resolved by every
    // CLI command and was resolved by nothing in the editor, because the
    // document threw its own URI away - `DocumentState::new(_uri, ...)`. A
    // design split across files came up empty in the editor while the same
    // file checked fine on the command line.
    let example = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("the crate sits two levels below the repo root")
        .join("examples/v2-imports.cypcb");
    let text = std::fs::read_to_string(&example).expect("the example is there");
    let uri = format!("file://{}", example.display());

    let mut server = Server::start();
    server.initialize();
    server.open(&uri, &text);

    let diagnostics = server.diagnostics_for(&uri);
    let messages: Vec<String> = diagnostics
        .iter()
        .filter_map(|d| d.get("message").and_then(Value::as_str))
        .map(str::to_string)
        .collect();

    let about_modules: Vec<&String> = messages
        .iter()
        .filter(|m| m.contains("unknown module") || m.contains("import"))
        .collect();
    assert!(
        about_modules.is_empty(),
        "the modules come from the imported file and the editor could not find them: {about_modules:?}"
    );

    // The positive half, and the one that fails without import resolution: the
    // parts an imported module brings have to be in the model the editor
    // checks. `cypcb check` on this file reports ten unrouted pins, every one
    // of them named after an instance of an imported block.
    assert!(
        messages.iter().any(|m| m.contains("DIV_A_RTOP")),
        "the imported blocks were never instantiated, so the editor is checking an empty board: {messages:?}"
    );
}

/// A part on a line that opens with an emoji: one `char`, two UTF-16 units,
/// four UTF-8 bytes. Its footprint does not exist, so the line carries an
/// error after the emoji.
const AFTER_AN_EMOJI: &str = "version 1

board probe {
    size 20mm x 20mm
    layers 2
}

/* \u{1F50C} */ component R1 resistor \"NO_SUCH_FP\" {
    value \"10k\"
    at 5mm, 5mm
}
";

/// Where a substring starts, as a zero-based line and a column counted in
/// `encoding` - the count the client and server agreed on.
fn position_in(source: &str, needle: &str, encoding: &str) -> (u32, u32) {
    let offset = source.find(needle).expect("the needle is in the source");
    let before = &source[..offset];
    let line = before.matches('\n').count() as u32;
    let start_of_line = before.rsplit('\n').next().unwrap_or("");
    let column = match encoding {
        "utf-8" => start_of_line.len(),
        "utf-16" => start_of_line.encode_utf16().count(),
        other => panic!("no such encoding in this test: {other}"),
    } as u32;
    (line, column)
}

/// Open the board as a client that offers `offered`, and check the server
/// counts the way it said it would: the diagnostic lands on the footprint
/// name and hover changes card exactly at the footprint's quotes.
fn columns_agree_after_an_emoji(offered: Value, expected: &str) {
    let uri = "file:///virtual/lsp-probe/emoji.cypcb";
    let mut server = Server::start();
    let result = server.initialize_with(offered);
    assert_eq!(
        result
            .pointer("/result/capabilities/positionEncoding")
            .and_then(Value::as_str),
        Some(expected),
        "the server has to say what it counts in: {result}"
    );
    server.open(uri, AFTER_AN_EMOJI);

    let diagnostics = server.diagnostics_for(uri);
    let unknown = diagnostics
        .iter()
        .find(|d| {
            d.get("message")
                .and_then(Value::as_str)
                .is_some_and(|m| m.contains("NO_SUCH_FP"))
        })
        .unwrap_or_else(|| panic!("the footprint is unknown: {diagnostics:?}"));
    let (line, character) = position_in(AFTER_AN_EMOJI, "\"NO_SUCH_FP\"", expected);
    assert_eq!(
        (
            unknown.pointer("/range/start/line").and_then(Value::as_u64),
            unknown
                .pointer("/range/start/character")
                .and_then(Value::as_u64),
        ),
        (Some(u64::from(line)), Some(u64::from(character))),
        "in {expected} the name starts at {line}:{character}: {unknown}"
    );

    // Hover changes card at the footprint's quotes: the space before the
    // opening quote is R1, the closing quote is still the footprint. A
    // column read one unit off in either direction lands on the other card.
    let hover_at = |server: &mut Server, needle: &str| -> String {
        let (line, character) = position_in(AFTER_AN_EMOJI, needle, expected);
        let hover = server.request(
            "textDocument/hover",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": line, "character": character},
            }),
        );
        serde_json::to_string(hover.pointer("/result/contents").unwrap_or(&Value::Null))
            .expect("hover contents serialize")
    };
    let before = hover_at(&mut server, " \"NO_SUCH_FP\"");
    assert!(
        before.contains("**R1**") && !before.contains("**Footprint:"),
        "in {expected}, the space before the footprint is R1: {before}"
    );
    let closing = hover_at(&mut server, "\" {");
    assert!(
        closing.contains("**Footprint: NO_SUCH_FP**"),
        "in {expected}, the closing quote is the footprint: {closing}"
    );
}

/// A client that offers nothing gets UTF-16, which LSP 3.17 makes the default.
#[test]
fn a_client_that_offers_nothing_is_counted_in_utf16() {
    columns_agree_after_an_emoji(json!({}), "utf-16");
}

/// A client that offers UTF-8 gets it: the text is stored in it.
#[test]
fn a_client_that_offers_utf8_is_counted_in_utf8() {
    columns_agree_after_an_emoji(
        json!({"general": {"positionEncodings": ["utf-16", "utf-8"]}}),
        "utf-8",
    );
}

/// A client that offers only encodings the server does not speak gets UTF-16,
/// the one every client has to speak.
#[test]
fn a_client_that_offers_only_utf32_is_counted_in_utf16() {
    columns_agree_after_an_emoji(
        json!({"general": {"positionEncodings": ["utf-32"]}}),
        "utf-16",
    );
}

/// A footprint written here, so a library of any size can be built without
/// checking a single file from KiCad into the repository.
fn synthetic_footprint(name: &str) -> String {
    format!(
        "(footprint \"{name}\"\n\t(layer \"F.Cu\")\n\t(attr smd)\n\
         \t(pad \"1\" smd rect (at -0.5 0) (size 0.5 0.5) (layers \"F.Cu\" \"F.Paste\" \"F.Mask\"))\n\
         \t(pad \"2\" smd rect (at 0.5 0) (size 0.5 0.5) (layers \"F.Cu\" \"F.Paste\" \"F.Mask\"))\n)\n"
    )
}

/// A project directory whose index holds `count` generated footprints, named
/// `SYN_00000` upwards, imported the way `cypcb library import` imports.
fn a_project_with_an_index_of(tag: &str, count: usize) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cypcb-lsp-size-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let libraries = dir.join("libraries");
    let pretty = libraries.join("Synthetic.pretty");
    std::fs::create_dir_all(&pretty).expect("a place to write the library");
    for n in 0..count {
        let name = format!("SYN_{n:05}");
        std::fs::write(
            pretty.join(format!("{name}.kicad_mod")),
            synthetic_footprint(&name),
        )
        .expect("the footprint is written");
    }
    let mut manager =
        cypcb_library::LibraryManager::new(&dir.join("cypcb-library.db")).expect("an index");
    manager.add_kicad_search_path(libraries.clone());
    manager
        .auto_import_folder(&libraries)
        .expect("the generated library imports");
    dir
}

const SIZED_BOARD: &str = r#"version 1

board sized {
    size 30mm x 30mm
    layers 2
}

component R1 resistor "kicad::SYN_00001" {
    value "330"
    at 15mm, 15mm
}
"#;

/// Completion inside a footprint string, against an index the size of the
/// KiCad library, answers inside 50ms - the first request and every one after.
///
/// Timing, so it is not part of the suite. Run it in release:
/// `cargo test -p cypcb-lsp --release --test the_language_server_answers -- --ignored --nocapture`
#[test]
#[ignore = "timing: run in release with --ignored"]
fn completion_over_ten_thousand_indexed_names_answers_inside_50ms() {
    const NAMES: usize = 10_000;
    let dir = a_project_with_an_index_of("timing", NAMES);
    let path = dir.join("board.cypcb");
    std::fs::write(&path, SIZED_BOARD).expect("the board is written");
    let uri = format!("file://{}", path.display());

    let mut server = Server::start();
    server.initialize();
    server.open(&uri, SIZED_BOARD);
    server.diagnostics_for(&uri);

    let (line, character) = position_of(SIZED_BOARD, "SYN_00001\"");
    let mut times = Vec::new();
    let mut offered = 0;
    for _ in 0..21 {
        let started = Instant::now();
        let result = server.request(
            "textDocument/completion",
            json!({
                "textDocument": {"uri": uri},
                "position": {"line": line, "character": character},
            }),
        );
        times.push(started.elapsed());
        offered = result
            .pointer("/result")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
    }
    let first = times[0];
    let mut after = times[1..].to_vec();
    after.sort();
    let median = after[after.len() / 2];
    let worst = after[after.len() - 1];
    eprintln!(
        "COMPLETION names={NAMES} items={offered} first={first:?} median={median:?} worst={worst:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);

    assert!(offered >= NAMES, "every indexed name is offered: {offered}");
    let limit = Duration::from_millis(50);
    assert!(first < limit, "the first request took {first:?}");
    assert!(worst < limit, "the slowest later request took {worst:?}");
}
