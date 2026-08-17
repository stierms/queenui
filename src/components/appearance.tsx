import { Palette } from "lucide-react";
import { ChessPiece, type PieceSetId } from "../ChessPiece";
import { hasPreviewParam } from "../dev/preview";
import { boardThemes, pieceSets, type BoardThemeId } from "../lib/appearance";
import { Button, Popover } from "../ui/primitives";

export type AppearanceProps = {
  boardTheme: BoardThemeId;
  pieceSet: PieceSetId;
  onBoardThemeChange: (theme: BoardThemeId) => void;
  onPieceSetChange: (set: PieceSetId) => void;
};

export function AppearanceControls({
  boardTheme,
  pieceSet,
  onBoardThemeChange,
  onPieceSetChange,
  pieceCaptions = true,
}: AppearanceProps & { pieceCaptions?: boolean }) {
  const currentTheme =
    boardThemes.find((theme) => theme.id === boardTheme) ?? boardThemes[0];
  return (
    <>
      <section className="appearance-control-section">
        <span className="picker-label">Board color</span>
        <div className="board-theme-grid">
          {boardThemes.map((theme) => (
            <button
              type="button"
              className={boardTheme === theme.id ? "selected" : ""}
              aria-pressed={boardTheme === theme.id}
              onClick={() => onBoardThemeChange(theme.id)}
              key={theme.id}
            >
              <i
                style={{
                  background: `linear-gradient(135deg, ${theme.light} 0 50%, ${theme.dark} 50%)`,
                }}
              />
              <span>{theme.name}</span>
              {boardTheme === theme.id && <b>✓</b>}
            </button>
          ))}
        </div>
      </section>
      <section className="appearance-control-section">
        <span className="picker-label">Piece style</span>
        <div className="piece-set-list">
          {pieceSets.map((set) => (
            <button
              type="button"
              className={pieceSet === set.id ? "selected" : ""}
              aria-pressed={pieceSet === set.id}
              onClick={() => onPieceSetChange(set.id)}
              key={set.id}
            >
              <span
                className="piece-set-preview"
                style={{
                  background: `linear-gradient(135deg, ${currentTheme.light} 0 50%, ${currentTheme.dark} 50%)`,
                }}
              >
                <ChessPiece type="q" color="w" pieceSet={set.id} />
                <ChessPiece type="n" color="b" pieceSet={set.id} />
              </span>
              <span>
                <strong>{set.name}</strong>
                {pieceCaptions && <small>{set.description}</small>}
              </span>
              {pieceSet === set.id && <b>✓</b>}
            </button>
          ))}
        </div>
      </section>
    </>
  );
}

export function BoardAppearancePicker({
  boardTheme,
  pieceSet,
  onBoardThemeChange,
  onPieceSetChange,
}: AppearanceProps) {
  const previewOpen = hasPreviewParam("appearance-preview");
  return (
    <Popover.Root defaultOpen={previewOpen}>
      <Popover.Trigger asChild>
        <Button
          variant="icon"
          className="appearance-trigger"
          aria-label="Board appearance"
          title="Board appearance"
        >
          <Palette size={16} />
        </Button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          className="appearance-popover"
          sideOffset={8}
          align="end"
          collisionPadding={16}
        >
          <header>
            <span className="appearance-icon">
              <Palette size={17} />
            </span>
            <div>
              <strong>Board appearance</strong>
              <small>Saved automatically</small>
            </div>
          </header>
          <div className="popover-appearance-controls">
            <AppearanceControls
              boardTheme={boardTheme}
              pieceSet={pieceSet}
              onBoardThemeChange={onBoardThemeChange}
              onPieceSetChange={onPieceSetChange}
              pieceCaptions={false}
            />
          </div>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
