//! Stateful extractor that turns raw Player.log lines into [`ParsedEvent`]s.
//!
//! MTGA's detailed-log format is undocumented and irregular. The events we
//! care about all live on lines starting with `[UnityCrossThreadLogger]` (or
//! occasionally `[Client GRE]`). They take a few shapes:
//!
//!   1. Request/response with multi-line JSON body:
//!         `[UnityCrossThreadLogger]==> Event.Foo(123)`
//!         `{`
//!         `  "..." : "..."`
//!         `}`
//!   2. Header + single-line body on the same line.
//!   3. Game-state messages with a free-form header:
//!         `[UnityCrossThreadLogger]GRE to Client:`
//!         `{ ... }`
//!
//! We track JSON brace depth across lines (respecting strings/escapes) so the
//! body can span many lines.

use chrono::Utc;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;

use super::events::{Direction, ParsedEvent};

/// Strip an optional `M/D/YYYY H:MM:SS [AM|PM]:` timestamp prefix that MTGA
/// inserts between `[UnityCrossThreadLogger]` and the marker on most lines.
static TIMESTAMP_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\d{1,2}/\d{1,2}/\d{4} \d{1,2}:\d{2}:\d{2}(?: [AP]M)?:\s*").expect("ts re compiles")
});

/// Request/response arrow header.
/// Captures: 1=arrow ("==>" or "<=="), 2=label, 3=optional inline JSON tail
static HEADER_RE: Lazy<Regex> = Lazy::new(|| {
    // Examples matched (post-timestamp strip):
    //   ==> RankGetCombinedRankInfo
    //   <== DeckUpsertDeckV3(0fa6c67a-09ba-435e-bc58-e12567c213fd)
    //   <== Inventory.Updated {"gold": 0}
    Regex::new(r"^(==>|<==)\s+([A-Za-z0-9_.]+)(?:\([^)]*\))?\s*(\{.*)?$").expect("header regex compiles")
});

/// Match-flow header where MTGA writes `Match to <id>: <EventName>` or
/// `<id> to Match: <EventName>`. The event name maps directly to the kind.
static MATCH_FLOW_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^(?:Match to [A-Z0-9_]+|[A-Z0-9_]+ to Match):\s+([A-Za-z][A-Za-z0-9_]*)\s*(\{.*)?$")
        .expect("match flow regex compiles")
});

/// Legacy free-form match-flow markers — kept for older logs / future-proofing.
static FREE_MARKERS: &[(&str, &str)] = &[
    ("GRE to Client", "GreToClientEvent"),
    ("Client to GRE", "ClientToGreMessage"),
];

pub struct Accumulator {
    state: State,
}

enum State {
    Idle,
    Collecting {
        kind: String,
        direction: Direction,
        depth: i32,
        in_string: bool,
        escape: bool,
        buf: String,
        seen_open: bool,
    },
}

impl Accumulator {
    pub fn new() -> Self {
        Self { state: State::Idle }
    }

    /// Consume one log line. Returns zero or more completed events.
    pub fn feed(&mut self, line: String) -> Vec<ParsedEvent> {
        let mut out = Vec::new();
        // Take ownership so we can mutate freely.
        let state = std::mem::replace(&mut self.state, State::Idle);
        self.state = match state {
            State::Idle => self.start_or_skip(&line, &mut out),
            State::Collecting { kind, direction, depth, in_string, escape, mut buf, seen_open } => {
                if !seen_open && line.trim_start().starts_with('{') {
                    self.consume_json(
                        &line, kind, direction, depth, in_string, escape, &mut buf, true, &mut out,
                    )
                } else if seen_open {
                    self.consume_json(
                        &line, kind, direction, depth, in_string, escape, &mut buf, true, &mut out,
                    )
                } else if line.trim().is_empty() {
                    // Allow blank lines between header and body.
                    State::Collecting { kind, direction, depth, in_string, escape, buf, seen_open }
                } else {
                    // Body never arrived; restart from this line.
                    self.start_or_skip(&line, &mut out)
                }
            }
        };
        out
    }

    fn start_or_skip(&self, line: &str, out: &mut Vec<ParsedEvent>) -> State {
        if let Some((kind, direction, inline_tail)) = parse_header(line) {
            let mut buf = String::new();
            if let Some(tail) = inline_tail {
                let mut depth = 0;
                let mut in_string = false;
                let mut escape = false;
                let mut seen_open = false;
                let finished = scan_json(
                    &tail, &mut depth, &mut in_string, &mut escape, &mut buf, &mut seen_open,
                );
                if finished {
                    if let Ok(payload) = serde_json::from_str::<Value>(&buf) {
                        out.push(ParsedEvent {
                            timestamp: Utc::now(),
                            kind,
                            direction,
                            payload,
                        });
                    }
                    return State::Idle;
                }
                return State::Collecting {
                    kind,
                    direction,
                    depth,
                    in_string,
                    escape,
                    buf,
                    seen_open,
                };
            }
            return State::Collecting {
                kind,
                direction,
                depth: 0,
                in_string: false,
                escape: false,
                buf,
                seen_open: false,
            };
        }
        State::Idle
    }

    #[allow(clippy::too_many_arguments)]
    fn consume_json(
        &self,
        line: &str,
        kind: String,
        direction: Direction,
        mut depth: i32,
        mut in_string: bool,
        mut escape: bool,
        buf: &mut String,
        mut seen_open: bool,
        out: &mut Vec<ParsedEvent>,
    ) -> State {
        let finished = scan_json(line, &mut depth, &mut in_string, &mut escape, buf, &mut seen_open);
        if finished {
            match serde_json::from_str::<Value>(buf) {
                Ok(payload) => out.push(ParsedEvent {
                    timestamp: Utc::now(),
                    kind,
                    direction,
                    payload,
                }),
                Err(e) => tracing::trace!(error = %e, "json parse failed; dropping event"),
            }
            return State::Idle;
        }
        // Append a synthetic newline so the buffer remains valid JSON whitespace.
        buf.push('\n');
        State::Collecting {
            kind,
            direction,
            depth,
            in_string,
            escape,
            buf: std::mem::take(buf),
            seen_open,
        }
    }
}

fn parse_header(line: &str) -> Option<(String, Direction, Option<String>)> {
    // Modern detailed-log responses arrive in two-line pairs:
    //
    //   [UnityCrossThreadLogger]5/11/2026 5:11:38 PM   ← "preamble" line
    //   <== GraphGetGraphState(uuid)                   ← bare header (no prefix)
    //   { ... }
    //
    // So the prefix is optional. Requests still come prefixed (`[UCT]==> Foo`).
    let stripped = line
        .strip_prefix("[UnityCrossThreadLogger]")
        .or_else(|| line.strip_prefix("[Client GRE]"))
        .unwrap_or(line);
    let body = TIMESTAMP_RE.replace(stripped.trim_start(), "");
    let body = body.trim_start();
    if body.is_empty() {
        return None;
    }

    if let Some(caps) = HEADER_RE.captures(body) {
        let arrow = caps.get(1)?.as_str();
        let kind = caps.get(2)?.as_str().to_string();
        let direction = if arrow == "==>" { Direction::Request } else { Direction::Response };
        let inline = caps.get(3).map(|m| m.as_str().to_string());
        return Some((kind, direction, inline));
    }
    if let Some(caps) = MATCH_FLOW_RE.captures(body) {
        let kind = caps.get(1)?.as_str().to_string();
        let inline = caps.get(2).map(|m| m.as_str().to_string());
        return Some((kind, Direction::Note, inline));
    }
    for (marker, kind) in FREE_MARKERS {
        if let Some(after) = body.strip_prefix(marker) {
            let after = after.trim_start_matches(':').trim();
            let inline = if after.starts_with('{') { Some(after.to_string()) } else { None };
            return Some(((*kind).to_string(), Direction::Note, inline));
        }
    }
    None
}

/// Extend `buf` with characters from `line`, tracking brace/quote state.
/// Returns true once the JSON object closes (depth == 0 after seeing an open).
fn scan_json(
    line: &str,
    depth: &mut i32,
    in_string: &mut bool,
    escape: &mut bool,
    buf: &mut String,
    seen_open: &mut bool,
) -> bool {
    for ch in line.chars() {
        buf.push(ch);
        if *escape {
            *escape = false;
            continue;
        }
        if *in_string {
            match ch {
                '\\' => *escape = true,
                '"' => *in_string = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => *in_string = true,
            '{' | '[' => {
                *depth += 1;
                *seen_open = true;
            }
            '}' | ']' => {
                *depth -= 1;
                if *seen_open && *depth == 0 {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(lines: &[&str]) -> Vec<ParsedEvent> {
        let mut acc = Accumulator::new();
        let mut out = Vec::new();
        for l in lines {
            out.extend(acc.feed((*l).to_string()));
        }
        out
    }

    #[test]
    fn parses_request_with_multi_line_body() {
        let events = collect(&[
            "[UnityCrossThreadLogger]==> Deck.GetDeckListsV3(42)",
            "{",
            "  \"playerId\": \"abc\",",
            "  \"decks\": [1, 2, 3]",
            "}",
        ]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "Deck.GetDeckListsV3");
        assert_eq!(events[0].direction, Direction::Request);
        assert_eq!(events[0].payload["playerId"], "abc");
    }

    #[test]
    fn parses_response_with_inline_body() {
        let events = collect(&[
            "[UnityCrossThreadLogger]<== Inventory.Updated {\"gold\": 1234, \"gems\": 0}",
        ]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "Inventory.Updated");
        assert_eq!(events[0].direction, Direction::Response);
        assert_eq!(events[0].payload["gold"], 1234);
    }

    #[test]
    fn parses_gre_to_client_marker() {
        let events = collect(&[
            "[UnityCrossThreadLogger]GRE to Client:",
            "{",
            "  \"greToClientEvent\": {\"greToClientMessages\": []}",
            "}",
        ]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "GreToClientEvent");
        assert_eq!(events[0].direction, Direction::Note);
    }

    #[test]
    fn ignores_unrelated_lines() {
        let events = collect(&[
            "Mono path[0] = '...'",
            "[Accounts - Login] something",
            "[TaskLogger]Doorbell response: {}",
        ]);
        assert!(events.is_empty());
    }

    #[test]
    fn recovers_after_aborted_event() {
        let events = collect(&[
            "[UnityCrossThreadLogger]==> Event.Foo(1)",
            "Mono path[0] = ...", // body never came
            "[UnityCrossThreadLogger]<== Event.Bar {\"ok\": true}",
        ]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "Event.Bar");
    }

    #[test]
    fn handles_braces_in_strings() {
        let events = collect(&[
            "[UnityCrossThreadLogger]<== T {\"s\": \"contains } and { braces\"}",
        ]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].payload["s"], "contains } and { braces");
    }

    #[test]
    fn parses_modern_camelcase_response_with_uuid() {
        let events = collect(&[
            "[UnityCrossThreadLogger]5/11/2026 4:56:02 PM: <== DeckUpsertDeckV3(0fa6c67a-09ba-435e-bc58-e12567c213fd)",
            "{",
            "  \"id\": \"deck-1\",",
            "  \"name\": \"Mono Red\"",
            "}",
        ]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "DeckUpsertDeckV3");
        assert_eq!(events[0].direction, Direction::Response);
        assert_eq!(events[0].payload["name"], "Mono Red");
    }

    #[test]
    fn parses_match_to_player_marker() {
        let events = collect(&[
            "[UnityCrossThreadLogger]5/11/2026 4:58:15 PM: Match to I53ZEAJSORASVMQYSTBPI35BUA: GreToClientEvent",
            "{",
            "  \"greToClientEvent\": {\"greToClientMessages\": []}",
            "}",
        ]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "GreToClientEvent");
        assert_eq!(events[0].direction, Direction::Note);
    }

    #[test]
    fn parses_player_to_match_marker() {
        let events = collect(&[
            "[UnityCrossThreadLogger]5/11/2026 4:59:00 PM: I53ZEAJSORASVMQYSTBPI35BUA to Match: ClientToGremessage",
            "{\"type\": \"action\"}",
        ]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "ClientToGremessage");
    }

    #[test]
    fn parses_bare_response_after_timestamp_preamble() {
        // Modern response framing: a [UCT]<timestamp> line, then a bare `<==`
        // line on its own (no [UnityCrossThreadLogger] prefix), then JSON.
        let events = collect(&[
            "[UnityCrossThreadLogger]5/11/2026 5:11:38 PM",
            "<== DeckGetAllPreconDecksV3(e09f134f-377c-42f6-a7f6-d72d31d20d0b)",
            "{\"CacheVersion\": 555, \"PreconDecks\": []}",
        ]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "DeckGetAllPreconDecksV3");
        assert_eq!(events[0].direction, Direction::Response);
        assert_eq!(events[0].payload["CacheVersion"], 555);
    }

    #[test]
    fn timestamp_prefix_doesnt_break_arrow_match() {
        let events = collect(&[
            "[UnityCrossThreadLogger]5/11/2026 4:56:02 PM: ==> RankGetCombinedRankInfo {\"x\":1}",
        ]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "RankGetCombinedRankInfo");
        assert_eq!(events[0].direction, Direction::Request);
    }
}
