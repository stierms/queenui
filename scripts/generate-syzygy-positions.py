#!/usr/bin/env python3
"""Generate a reproducible legal, non-check position per six-man table.

Requires python-chess. Pass tablebase directories, or --table-list with one
material name (e.g. KQvKRR) per line. Output is TSV: table name, FEN.
"""

import argparse
import pathlib
import random

import chess


def position(material: str) -> str:
    rng = random.Random(material)
    sides = material.split("v")
    if len(sides) != 2 or sum(map(len, sides)) != 6:
        raise ValueError(f"Not a six-man table: {material}")
    pieces = [
        (chess.Piece.from_symbol(symbol if side == 0 else symbol.lower()))
        for side, symbols in enumerate(sides)
        for symbol in symbols
    ]
    for _ in range(100_000):
        board = chess.Board(None)
        for square, piece in zip(rng.sample(list(chess.SQUARES), 6), pieces):
            board.set_piece_at(square, piece)
        if board.is_valid() and not board.is_check() and any(board.legal_moves):
            return board.fen()
    raise RuntimeError(f"Could not generate a legal position for {material}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directories", nargs="*", type=pathlib.Path)
    parser.add_argument("--table-list", type=pathlib.Path)
    args = parser.parse_args()
    if args.table_list:
        materials = args.table_list.read_text().splitlines()
    else:
        materials = [
            file.stem
            for directory in args.directories
            for file in directory.glob("*.rtbw")
            if len(file.stem.replace("v", "")) == 6
        ]
    if not materials:
        parser.error("Provide directories containing six-man tables or --table-list")
    for material in sorted(set(materials)):
        print(f"{material}\t{position(material)}")


if __name__ == "__main__":
    main()
