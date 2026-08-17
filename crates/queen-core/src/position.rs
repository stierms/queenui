use shakmaty::{fen::Fen, uci::UciMove, CastlingMode, Chess, Color, EnPassantMode, Position};

pub const MAX_LIVE_FEN_BYTES: usize = 512;
pub const MAX_LIVE_MOVES_BYTES: usize = 32 * 1024;
pub const MAX_LIVE_PLIES: usize = 1_024;

/// A live Lichess position after its FEN and complete move history have been
/// parsed and replayed legally. Raw stream text never crosses from this type
/// into UCI framing or a move-submission URL.
#[derive(Clone)]
pub struct LivePosition {
    initial_fen: String,
    moves: Vec<String>,
    chess: Chess,
}

impl LivePosition {
    pub fn parse(initial_fen: &str, moves: &str) -> Result<Self, String> {
        reject_framing(initial_fen, "initial FEN", MAX_LIVE_FEN_BYTES)?;
        reject_framing(moves, "move history", MAX_LIVE_MOVES_BYTES)?;
        let normalized_fen = initial_fen.trim();
        let (mut chess, canonical_initial_fen) =
            if normalized_fen.is_empty() || normalized_fen == "startpos" {
                (Chess::default(), "startpos".to_string())
            } else {
                let chess: Chess = Fen::from_ascii(normalized_fen.as_bytes())
                    .map_err(|error| format!("Invalid live initial FEN: {error}"))?
                    .into_position(CastlingMode::Standard)
                    .map_err(|error| format!("Invalid live initial position: {error}"))?;
                let canonical = Fen::from_position(&chess, EnPassantMode::Legal).to_string();
                (chess, canonical)
            };

        let mut canonical_moves = Vec::new();
        for (ply, token) in moves.split_whitespace().enumerate() {
            if ply >= MAX_LIVE_PLIES {
                return Err(format!(
                    "Live move history exceeds the {MAX_LIVE_PLIES}-ply safety limit"
                ));
            }
            let uci: UciMove = token.parse().map_err(|error| {
                format!("Invalid live move {token} at ply {}: {error}", ply + 1)
            })?;
            let chess_move = uci.to_move(&chess).map_err(|error| {
                format!("Illegal live move {token} at ply {}: {error}", ply + 1)
            })?;
            canonical_moves.push(UciMove::from_standard(chess_move).to_string());
            chess.play_unchecked(chess_move);
        }

        Ok(Self {
            initial_fen: canonical_initial_fen,
            moves: canonical_moves,
            chess,
        })
    }

    pub fn initial_fen(&self) -> &str {
        &self.initial_fen
    }

    pub fn moves(&self) -> String {
        self.moves.join(" ")
    }

    pub fn ply_count(&self) -> usize {
        self.moves.len()
    }

    pub fn side_to_move(&self) -> Color {
        self.chess.turn()
    }

    pub fn is_white_to_move(&self) -> bool {
        self.side_to_move() == Color::White
    }

    pub fn uci_position_command(&self) -> String {
        let moves = self.moves();
        match (self.initial_fen.as_str(), moves.is_empty()) {
            ("startpos", true) => "position startpos".into(),
            ("startpos", false) => format!("position startpos moves {moves}"),
            (fen, true) => format!("position fen {fen}"),
            (fen, false) => format!("position fen {fen} moves {moves}"),
        }
    }

    /// Parses one engine token, proves it is legal in this exact position, and
    /// returns the canonical Standard-chess UCI spelling used in Lichess URLs.
    pub fn canonical_legal_move(&self, engine_token: &str) -> Result<String, String> {
        reject_framing(engine_token, "engine bestmove", 32)?;
        if engine_token.split_whitespace().count() != 1 {
            return Err("The engine bestmove must contain exactly one UCI move".into());
        }
        let uci: UciMove = engine_token
            .parse()
            .map_err(|error| format!("The engine returned an invalid UCI move: {error}"))?;
        let chess_move = uci
            .to_move(&self.chess)
            .map_err(|error| format!("The engine returned an illegal move: {error}"))?;
        Ok(UciMove::from_standard(chess_move).to_string())
    }
}

fn reject_framing(value: &str, label: &str, cap: usize) -> Result<(), String> {
    if value.contains(['\r', '\n']) {
        return Err(format!("The live {label} contains forbidden CR/LF framing"));
    }
    if value.len() > cap {
        return Err(format!(
            "The live {label} exceeds the {cap}-byte safety limit"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{LivePosition, MAX_LIVE_FEN_BYTES, MAX_LIVE_MOVES_BYTES, MAX_LIVE_PLIES};

    #[test]
    fn reconstructs_and_canonicalizes_legal_positions() {
        let position = LivePosition::parse("startpos", "e2e4 e7e5 g1f3").expect("position");
        assert_eq!(position.moves(), "e2e4 e7e5 g1f3");
        assert!(!position.is_white_to_move());
        assert_eq!(
            position.uci_position_command(),
            "position startpos moves e2e4 e7e5 g1f3"
        );
        assert_eq!(position.canonical_legal_move("b8c6").unwrap(), "b8c6");
        assert!(position.canonical_legal_move("../resign").is_err());
        assert!(position.canonical_legal_move("e2e4").is_err());
    }

    #[test]
    fn derives_turn_from_custom_fen_and_rejects_framing() {
        let fen = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR b KQkq - 0 1";
        let position = LivePosition::parse(fen, "").expect("black to move");
        assert!(!position.is_white_to_move());
        assert!(LivePosition::parse("startpos\nisready", "").is_err());
        assert!(LivePosition::parse("startpos\rquit", "").is_err());
        assert!(LivePosition::parse("startpos", "e2e4\nquit").is_err());
        assert!(LivePosition::parse("startpos", "e2e4\rquit").is_err());
        assert!(LivePosition::parse("startpos", "e2e5").is_err());
        assert!(LivePosition::parse("definitely not a FEN", "").is_err());
        let oversized_fen = " ".repeat(MAX_LIVE_FEN_BYTES + 1);
        let fen_error = LivePosition::parse(&oversized_fen, "")
            .err()
            .expect("oversized otherwise-empty FEN was accepted");
        assert!(
            fen_error.contains(&format!(
                "exceeds the {MAX_LIVE_FEN_BYTES}-byte safety limit"
            )),
            "{fen_error}"
        );
        let oversized_moves = " ".repeat(MAX_LIVE_MOVES_BYTES + 1);
        let moves_error = LivePosition::parse("startpos", &oversized_moves)
            .err()
            .expect("oversized otherwise-empty move history was accepted");
        assert!(
            moves_error.contains(&format!(
                "exceeds the {MAX_LIVE_MOVES_BYTES}-byte safety limit"
            )),
            "{moves_error}"
        );
    }

    #[test]
    fn caps_live_histories() {
        let oversized =
            std::iter::repeat_n(["g1f3", "g8f6", "f3g1", "f6g8"], MAX_LIVE_PLIES / 4 + 1)
                .flatten()
                .collect::<Vec<_>>()
                .join(" ");
        let error = match LivePosition::parse("startpos", &oversized) {
            Ok(_) => panic!("overlong legal history was accepted"),
            Err(error) => error,
        };
        assert!(error.contains("ply safety limit"), "{error}");
    }
}
