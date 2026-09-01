use crate::models::OpeningBookConfig;
use pgn_reader::{Reader, SanPlus, Visitor};
use polyglot_book_rs::polyglot_hash_from_fen;
use rand::Rng;
use shakmaty::{fen::Fen, uci::UciMove, CastlingMode, Chess, EnPassantMode, Position};
use std::{
    collections::HashMap,
    fs::File,
    io::{self, BufReader, Read},
    ops::ControlFlow,
    path::Path,
};

// Polyglot expands predictably to one compact record per 16-byte file entry,
// so established libraries can have a higher bounded ceiling. PGN parsing
// builds a hash map of positions and can expand far beyond its input size.
pub const POLYGLOT_OPENING_BOOK_FILE_CAP: u64 = 512 * 1024 * 1024;
pub const POLYGLOT_OPENING_BOOK_ENTRY_CAP: usize = 33_554_432;
pub const PGN_OPENING_BOOK_FILE_CAP: u64 = 64 * 1024 * 1024;
pub const PGN_OPENING_BOOK_ENTRY_CAP: usize = 5_000_000;
const POLYGLOT_RECORD_BYTES: u64 = 16;
const POLYGLOT_READ_BUFFER_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug)]
struct RawPolyglotEntry {
    key: u64,
    move_data: u16,
    weight: u16,
}

struct PolyglotBook {
    entries: Vec<RawPolyglotEntry>,
}

impl PolyglotBook {
    fn load(path: &str) -> io::Result<Self> {
        let file = File::open(path)?;
        let file_size = file.metadata()?.len();
        if file_size % POLYGLOT_RECORD_BYTES != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "truncated 16-byte Polyglot entry",
            ));
        }

        let entry_count = usize::try_from(file_size / POLYGLOT_RECORD_BYTES).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Polyglot entry count does not fit in memory",
            )
        })?;
        let mut reader = BufReader::with_capacity(POLYGLOT_READ_BUFFER_BYTES, file);
        let mut entries = Vec::with_capacity(entry_count);
        let mut record = [0_u8; POLYGLOT_RECORD_BYTES as usize];
        let mut previous_key = None;
        let mut already_sorted = true;

        for _ in 0..entry_count {
            reader.read_exact(&mut record)?;
            let key = u64::from_be_bytes(record[0..8].try_into().expect("fixed key slice"));
            if previous_key.is_some_and(|previous| previous > key) {
                already_sorted = false;
            }
            previous_key = Some(key);
            entries.push(RawPolyglotEntry {
                key,
                move_data: u16::from_be_bytes(record[8..10].try_into().expect("fixed move slice")),
                weight: u16::from_be_bytes(record[10..12].try_into().expect("fixed weight slice")),
            });
        }

        if !already_sorted {
            entries.sort_unstable_by_key(|entry| entry.key);
        }
        Ok(Self { entries })
    }

    fn entry_count(&self) -> usize {
        self.entries.len()
    }

    fn moves_for_fen(&self, fen: &str) -> Vec<BookMove> {
        let Ok(key) = polyglot_hash_from_fen(fen) else {
            return Vec::new();
        };
        let start = self.entries.partition_point(|entry| entry.key < key);
        let end = self.entries.partition_point(|entry| entry.key <= key);
        self.entries[start..end]
            .iter()
            .map(|entry| BookMove {
                uci: decode_polyglot_move(entry.move_data),
                weight: u32::from(entry.weight),
            })
            .collect()
    }
}

fn decode_polyglot_move(move_data: u16) -> String {
    let from = ((move_data >> 6) & 0x3f) as u8;
    let to = (move_data & 0x3f) as u8;
    let mut uci = String::with_capacity(5);
    for square in [from, to] {
        uci.push(char::from(b'a' + square % 8));
        uci.push(char::from(b'1' + square / 8));
    }
    if let Some(promotion) = match (move_data >> 12) & 0x7 {
        1 => Some('n'),
        2 => Some('b'),
        3 => Some('r'),
        4 => Some('q'),
        _ => None,
    } {
        uci.push(promotion);
    }
    uci
}

pub struct OpeningBook {
    config: OpeningBookConfig,
    source: BookSource,
}

enum BookSource {
    Polyglot(PolyglotBook),
    Pgn(HashMap<String, Vec<BookMove>>),
}

#[derive(Clone, Debug)]
struct BookMove {
    uci: String,
    weight: u32,
}

#[derive(Clone, Debug)]
pub struct BookInspection {
    pub name: String,
    pub format: String,
    pub entry_count: usize,
}

pub struct PreparedBook {
    pub inspection: BookInspection,
    source: BookSource,
}

impl PreparedBook {
    pub fn finish(self, config: OpeningBookConfig) -> OpeningBook {
        OpeningBook {
            config,
            source: self.source,
        }
    }
}

impl OpeningBook {
    pub fn load(config: &OpeningBookConfig) -> Result<Self, String> {
        let path = Path::new(&config.path);
        let source = match config.format.as_str() {
            "polyglot" => {
                enforce_polyglot_size(path)?;
                let book = PolyglotBook::load(&config.path)
                    .map_err(|error| format!("Could not load Polyglot book: {error}"))?;
                enforce_polyglot_entry_cap(book.entry_count())?;
                BookSource::Polyglot(book)
            }
            "pgn" => {
                enforce_file_cap(path, PGN_OPENING_BOOK_FILE_CAP, "PGN")?;
                BookSource::Pgn(load_pgn(&config.path)?.0)
            }
            _ => {
                return Err(format!(
                    "Unsupported opening-book format: {}",
                    config.format
                ))
            }
        };
        Ok(Self {
            config: config.clone(),
            source,
        })
    }

    pub fn choose_move(&self, initial_fen: &str, moves: &str) -> Option<String> {
        let ply = moves.split_whitespace().count() as u32;
        if !self.config.enabled || ply >= self.config.max_plies {
            return None;
        }
        let position = position_after(initial_fen, moves).ok()?;
        let mut candidates = match &self.source {
            BookSource::Polyglot(book) => book
                .moves_for_fen(&Fen::from_position(&position, EnPassantMode::Legal).to_string())
                .into_iter()
                .map(|mut entry| {
                    entry.uci = normalize_polyglot_castling(entry.uci);
                    entry
                })
                .collect(),
            BookSource::Pgn(positions) => positions
                .get(&position_key(&position))
                .cloned()
                .unwrap_or_default(),
        };
        candidates.retain(|candidate| {
            candidate
                .uci
                .parse::<UciMove>()
                .ok()
                .and_then(|uci| uci.to_move(&position).ok())
                .is_some()
        });
        candidates.sort_by(|left, right| right.weight.cmp(&left.weight));
        if candidates.is_empty() {
            return None;
        }
        let percent = self.config.top_move_percent.clamp(1, 100) as usize;
        let candidate_count = (candidates.len() * percent).div_ceil(100).max(1);
        let selected = rand::rng().random_range(0..candidate_count);
        Some(candidates[selected].uci.clone())
    }
}

pub fn inspect(path: &str) -> Result<BookInspection, String> {
    Ok(prepare(path)?.inspection)
}

/// Parses a selected book once. The async caller performs this whole function
/// on a bounded blocking worker, then imports the bytes and caches `finish()`'s
/// already-parsed source without parsing the PGN a second time.
pub fn prepare(path: &str) -> Result<PreparedBook, String> {
    let path = Path::new(path);
    if !path.is_file() {
        return Err("The selected opening-book file does not exist.".into());
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Opening book")
        .to_string();
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("bin") => {
            enforce_polyglot_size(path)?;
            let book = PolyglotBook::load(path.to_string_lossy().as_ref())
                .map_err(|error| format!("Could not read Polyglot book: {error}"))?;
            if book.entry_count() == 0 {
                return Err("The selected Polyglot book contains no entries.".into());
            }
            enforce_polyglot_entry_cap(book.entry_count())?;
            Ok(PreparedBook {
                inspection: BookInspection {
                    name,
                    format: "polyglot".into(),
                    entry_count: book.entry_count(),
                },
                source: BookSource::Polyglot(book),
            })
        }
        Some("pgn") => {
            enforce_file_cap(path, PGN_OPENING_BOOK_FILE_CAP, "PGN")?;
            let (positions, entry_count) = load_pgn(path.to_string_lossy().as_ref())?;
            if entry_count == 0 {
                return Err("The selected PGN contains no legal mainline moves.".into());
            }
            Ok(PreparedBook {
                inspection: BookInspection {
                    name,
                    format: "pgn".into(),
                    entry_count,
                },
                source: BookSource::Pgn(positions),
            })
        }
        _ => Err("Choose a Polyglot .bin or portable .pgn opening book.".into()),
    }
}

fn enforce_file_cap(path: &Path, cap: u64, format: &str) -> Result<u64, String> {
    let size = path
        .metadata()
        .map_err(|error| format!("Could not inspect opening-book size: {error}"))?
        .len();
    if size > cap {
        return Err(format!(
            "{format} opening books are limited to {} MiB",
            cap / 1024 / 1024
        ));
    }
    Ok(size)
}

fn enforce_polyglot_size(path: &Path) -> Result<(), String> {
    let size = enforce_file_cap(path, POLYGLOT_OPENING_BOOK_FILE_CAP, "Polyglot")?;
    if size % 16 != 0 {
        return Err("The selected Polyglot book has a truncated 16-byte entry".into());
    }
    Ok(())
}

fn enforce_polyglot_entry_cap(entry_count: usize) -> Result<(), String> {
    if entry_count > POLYGLOT_OPENING_BOOK_ENTRY_CAP {
        return Err(format!(
            "The Polyglot book exceeds the {POLYGLOT_OPENING_BOOK_ENTRY_CAP}-entry safety limit"
        ));
    }
    Ok(())
}

fn position_after(initial_fen: &str, moves: &str) -> Result<Chess, String> {
    let mut position = if initial_fen.is_empty() || initial_fen == "startpos" {
        Chess::default()
    } else {
        Fen::from_ascii(initial_fen.as_bytes())
            .map_err(|error| format!("Invalid initial FEN: {error}"))?
            .into_position(CastlingMode::Standard)
            .map_err(|error| format!("Invalid initial position: {error}"))?
    };
    for token in moves.split_whitespace() {
        let uci: UciMove = token
            .parse()
            .map_err(|error| format!("Invalid move {token}: {error}"))?;
        let chess_move = uci
            .to_move(&position)
            .map_err(|error| format!("Illegal move {token}: {error}"))?;
        position.play_unchecked(chess_move);
    }
    Ok(position)
}

fn position_key(position: &Chess) -> String {
    Fen::from_position(position, EnPassantMode::Legal)
        .to_string()
        .split_whitespace()
        .take(4)
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_polyglot_castling(uci: String) -> String {
    match uci.as_str() {
        "e1h1" => "e1g1".into(),
        "e1a1" => "e1c1".into(),
        "e8h8" => "e8g8".into(),
        "e8a8" => "e8c8".into(),
        _ => uci,
    }
}

fn load_pgn(path: &str) -> Result<(HashMap<String, Vec<BookMove>>, usize), String> {
    let file = File::open(path).map_err(|error| format!("Could not open PGN book: {error}"))?;
    let mut reader = Reader::new(file);
    let mut visitor = PgnGameVisitor;
    let mut weighted: HashMap<String, HashMap<String, u32>> = HashMap::new();
    let mut entries: usize = 0;
    while let Some(game) = reader
        .read_game(&mut visitor)
        .map_err(|error| format!("Could not parse PGN book: {error}"))?
    {
        let game = game?;
        entries = entries
            .checked_add(game.len())
            .ok_or_else(|| "PGN opening-book entry count overflowed".to_string())?;
        if entries > PGN_OPENING_BOOK_ENTRY_CAP {
            return Err(format!(
                "The PGN book exceeds the {PGN_OPENING_BOOK_ENTRY_CAP}-entry safety limit"
            ));
        }
        for (position, chess_move) in game {
            *weighted
                .entry(position)
                .or_default()
                .entry(chess_move)
                .or_default() += 1;
        }
    }
    let positions = weighted
        .into_iter()
        .map(|(position, moves)| {
            (
                position,
                moves
                    .into_iter()
                    .map(|(uci, weight)| BookMove { uci, weight })
                    .collect(),
            )
        })
        .collect();
    Ok((positions, entries))
}

struct PgnGameVisitor;

impl Visitor for PgnGameVisitor {
    type Tags = ();
    type Movetext = (Chess, Vec<(String, String)>);
    type Output = Result<Vec<(String, String)>, String>;

    fn begin_tags(&mut self) -> ControlFlow<Self::Output, Self::Tags> {
        ControlFlow::Continue(())
    }

    fn begin_movetext(&mut self, _tags: Self::Tags) -> ControlFlow<Self::Output, Self::Movetext> {
        ControlFlow::Continue((Chess::default(), Vec::new()))
    }

    fn san(
        &mut self,
        movetext: &mut Self::Movetext,
        san_plus: SanPlus,
    ) -> ControlFlow<Self::Output> {
        let chess_move = match san_plus.san.to_move(&movetext.0) {
            Ok(chess_move) => chess_move,
            Err(error) => return ControlFlow::Break(Err(format!("Illegal PGN move: {error}"))),
        };
        let key = position_key(&movetext.0);
        let uci = UciMove::from_standard(chess_move).to_string();
        movetext.1.push((key, uci));
        movetext.0.play_unchecked(chess_move);
        ControlFlow::Continue(())
    }

    fn end_game(&mut self, movetext: Self::Movetext) -> Self::Output {
        Ok(movetext.1)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        decode_polyglot_move, inspect, normalize_polyglot_castling, position_after, position_key,
        OpeningBook, PGN_OPENING_BOOK_FILE_CAP, POLYGLOT_OPENING_BOOK_FILE_CAP,
    };
    use crate::models::OpeningBookConfig;
    use shakmaty::Chess;
    use std::fs;

    #[test]
    fn reconstructs_positions_from_uci_moves() {
        let position = position_after("startpos", "e2e4 e7e5 g1f3").expect("position");
        assert_ne!(position_key(&position), position_key(&Chess::default()));
    }

    #[test]
    fn converts_polyglot_castling_to_standard_uci() {
        assert_eq!(normalize_polyglot_castling("e1h1".into()), "e1g1");
        assert_eq!(normalize_polyglot_castling("e7e5".into()), "e7e5");
        assert_eq!(decode_polyglot_move((12 << 6) | 28), "e2e4");
        assert_eq!(decode_polyglot_move((55 << 6) | 63 | (4 << 12)), "h7h8q");
    }

    #[test]
    fn loads_sorted_and_unsorted_polyglot_records_with_buffered_lookup() {
        const START_POSITION_KEY: u64 = 0x463b_9618_1691_fc9c;

        fn record(key: u64, move_data: u16, weight: u16) -> [u8; 16] {
            let mut record = [0_u8; 16];
            record[0..8].copy_from_slice(&key.to_be_bytes());
            record[8..10].copy_from_slice(&move_data.to_be_bytes());
            record[10..12].copy_from_slice(&weight.to_be_bytes());
            record
        }

        let path = std::env::temp_dir().join(format!(
            "queenui-book-buffered-{}.bin",
            uuid::Uuid::new_v4()
        ));
        let mut bytes = Vec::new();
        // Deliberately put a later key first to exercise the unsorted fallback.
        bytes.extend_from_slice(&record(START_POSITION_KEY + 1, (6 << 6) | 21, 1));
        bytes.extend_from_slice(&record(START_POSITION_KEY, (11 << 6) | 27, 1));
        bytes.extend_from_slice(&record(START_POSITION_KEY, (12 << 6) | 28, 2));
        fs::write(&path, bytes).expect("write Polyglot book");

        let path_string = path.to_string_lossy().to_string();
        let inspection = inspect(&path_string).expect("inspect Polyglot book");
        assert_eq!(inspection.format, "polyglot");
        assert_eq!(inspection.entry_count, 3);
        let book = OpeningBook::load(&OpeningBookConfig {
            enabled: true,
            path: path_string,
            name: inspection.name,
            format: inspection.format,
            max_plies: 8,
            top_move_percent: 50,
            entry_count: inspection.entry_count,
        })
        .expect("load Polyglot book");
        assert_eq!(book.choose_move("startpos", "").as_deref(), Some("e2e4"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn builds_and_queries_a_portable_pgn_book() {
        let path = std::env::temp_dir().join(format!("queenui-book-{}.pgn", uuid::Uuid::new_v4()));
        fs::write(&path, "[Result \"*\"]\n\n1. e4 e5 2. Nf3 Nc6 *\n").expect("write PGN");
        let path_string = path.to_string_lossy().to_string();
        let inspection = inspect(&path_string).expect("inspect PGN");
        assert_eq!(inspection.format, "pgn");
        assert_eq!(inspection.entry_count, 4);
        let book = OpeningBook::load(&OpeningBookConfig {
            enabled: true,
            path: path_string,
            name: inspection.name,
            format: inspection.format,
            max_plies: 4,
            top_move_percent: 100,
            entry_count: inspection.entry_count,
        })
        .expect("load PGN");
        assert_eq!(book.choose_move("startpos", "").as_deref(), Some("e2e4"));
        assert_eq!(
            book.choose_move("startpos", "e2e4").as_deref(),
            Some("e7e5")
        );
        assert_eq!(book.choose_move("startpos", "e2e4 e7e5 g1f3 b8c6"), None);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejects_oversized_pgn_books_before_parsing() {
        let path =
            std::env::temp_dir().join(format!("queenui-book-cap-{}.pgn", uuid::Uuid::new_v4()));
        let file = fs::File::create(&path).expect("create sparse PGN");
        file.set_len(PGN_OPENING_BOOK_FILE_CAP + 1)
            .expect("extend sparse PGN");
        let error = inspect(path.to_string_lossy().as_ref()).unwrap_err();
        assert!(error.contains("PGN opening books are limited to 64 MiB"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejects_oversized_or_truncated_polyglot_books_before_parsing() {
        let oversized =
            std::env::temp_dir().join(format!("queenui-book-cap-{}.bin", uuid::Uuid::new_v4()));
        let file = fs::File::create(&oversized).expect("create sparse Polyglot book");
        file.set_len(POLYGLOT_OPENING_BOOK_FILE_CAP + 16)
            .expect("extend sparse Polyglot book");
        let error = inspect(oversized.to_string_lossy().as_ref()).unwrap_err();
        assert!(error.contains("Polyglot opening books are limited to 512 MiB"));
        let _ = fs::remove_file(oversized);

        let truncated = std::env::temp_dir().join(format!(
            "queenui-book-truncated-{}.bin",
            uuid::Uuid::new_v4()
        ));
        fs::write(&truncated, [0; 15]).expect("write truncated Polyglot book");
        let error = inspect(truncated.to_string_lossy().as_ref()).unwrap_err();
        assert!(error.contains("truncated 16-byte entry"));
        let _ = fs::remove_file(truncated);
    }
}
