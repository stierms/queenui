import { useEffect, useMemo, useState } from "react";
import { ChessPiece, type PieceKind } from "../ChessPiece";
import { Chessboard } from "../components/board";
import {
  APPEARANCE_IDENTITIES,
  identityBoardStyle,
} from "../lib/appearanceIdentities";
import type { LiveGame } from "../types";

const KINDS: PieceKind[] = ["k", "q", "r", "b", "n", "p"];

const POSITIONS: { id: string; label: string; moves: string }[] = [
  { id: "start", label: "Start", moves: "" },
  {
    id: "ruy",
    label: "Ruy Lopez",
    moves:
      "e2e4 e7e5 g1f3 b8c6 f1b5 a7a6 b5a4 g8f6 e1g1 f8e7 f1e1 b7b5 a4b3 d7d6 c2c3 e8g8 h2h3",
  },
  {
    id: "end",
    label: "Ending",
    moves:
      "e2e4 e7e5 g1f3 b8c6 f1b5 a7a6 b5c6 d7c6 d2d4 e5d4 d1d4 d8d4 f3d4 c8d7 c1e3 g8f6 b1c3 f8e7 e1c1 e8g8",
  },
];

function galleryGame(moves: string): LiveGame {
  return {
    id: "skins-gallery",
    accountId: "gallery",
    botUsername: "QueenBot",
    opponent: "preview",
    botRating: 2487,
    opponentRating: 2500,
    color: "white",
    initialFen: "startpos",
    moves,
    status: "started",
    whiteTime: 180000,
    blackTime: 180000,
    whiteIncrement: 0,
    blackIncrement: 0,
    clockUpdatedAt: Date.now(),
    result: null,
    engineLine: null,
    engineInfo: null,
    engineThinking: false,
    error: null,
  };
}

export function SkinsGallery() {
  const [positionId, setPositionId] = useState(POSITIONS[1]!.id);
  const [focused, setFocused] = useState<string | null>(null);
  const moves =
    POSITIONS.find((position) => position.id === positionId)?.moves ?? "";
  const game = useMemo(() => galleryGame(moves), [moves]);
  const identities = focused
    ? APPEARANCE_IDENTITIES.filter((identity) => identity.id === focused)
    : APPEARANCE_IDENTITIES;

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      setFocused(null);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  return (
    <div className="skins-gallery">
      <header className="skins-gallery-bar">
        <div>
          <p className="eyebrow">Appearance preview</p>
          <h1>Paired board identities</h1>
          <p>
            DEV comparison gallery — includes gallery-only concepts and
            identities available in Settings. Enlarge a board for Focus scale;
            Esc returns to the grid.
          </p>
        </div>
        <div
          className="skins-gallery-positions"
          role="group"
          aria-label="Position"
        >
          {POSITIONS.map((position) => (
            <button
              key={position.id}
              type="button"
              className={positionId === position.id ? "selected" : ""}
              aria-pressed={positionId === position.id}
              onClick={() => setPositionId(position.id)}
            >
              {position.label}
            </button>
          ))}
        </div>
      </header>
      <div className={`skins-gallery-grid ${focused ? "is-focused" : ""}`}>
        {identities.map((identity) => (
          <article
            key={identity.id}
            className={`skins-gallery-card board-identity-${identity.id}`}
            style={identityBoardStyle(identity)}
          >
            <header>
              <div>
                <h2>{identity.name}</h2>
                <small>
                  {identity.pieceName} pieces · {identity.thesis}
                </small>
              </div>
              <button
                type="button"
                onClick={() =>
                  setFocused((current) =>
                    current === identity.id ? null : identity.id,
                  )
                }
              >
                {focused === identity.id ? "Show all" : "Enlarge"}
              </button>
            </header>
            <div className="skins-gallery-board">
              <Chessboard game={game} pieceSet={identity.pieceSet} />
            </div>
            <div className="skins-gallery-strip" aria-hidden="true">
              {(["w", "b"] as const).map((color) => (
                <span key={color}>
                  {KINDS.map((kind) => (
                    <ChessPiece
                      key={`${color}-${kind}`}
                      type={kind}
                      color={color}
                      pieceSet={identity.pieceSet}
                    />
                  ))}
                </span>
              ))}
            </div>
          </article>
        ))}
      </div>
    </div>
  );
}
