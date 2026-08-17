#!/bin/sh

# Deterministic protocol fixture used to prove that a runner can launch and
# communicate with an executable on its own machine. It is not a chess engine.
while IFS= read -r command; do
  case "$command" in
    uci)
      printf '%s\n' \
        'id name QueenUI fake UCI fixture' \
        'id author QueenUI tests' \
        'option name Threads type spin default 1 min 1 max 32' \
        'uciok'
      ;;
    isready)
      printf '%s\n' 'readyok'
      ;;
    go*)
      printf '%s\n' \
        'info depth 1 score cp 0 nodes 1 nps 1 time 1 pv e2e4' \
        'bestmove e2e4'
      ;;
    stop)
      printf '%s\n' 'bestmove e2e4'
      ;;
    quit)
      exit 0
      ;;
  esac
done
