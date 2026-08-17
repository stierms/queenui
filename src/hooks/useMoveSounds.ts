import { useEffect, useRef } from "react";
import { playMoveSound } from "../lib/audio";
import { latestMoveWasCapture } from "../lib/chess";
import type { LiveGame } from "../types";

export function useMoveSounds(games: LiveGame[], enabled: boolean) {
  const previousMoves = useRef<Map<string, string>>(new Map());
  useEffect(() => {
    const next = new Map<string, string>();
    for (const game of games) {
      const previous = previousMoves.current.get(game.id);
      next.set(game.id, game.moves);
      if (
        enabled &&
        previous !== undefined &&
        previous !== game.moves &&
        game.moves.split(/\s+/).filter(Boolean).length >
          previous.split(/\s+/).filter(Boolean).length
      ) {
        playMoveSound(latestMoveWasCapture(game));
      }
    }
    previousMoves.current = next;
  }, [games, enabled]);
}
