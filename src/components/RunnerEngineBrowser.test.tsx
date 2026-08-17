import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import * as commands from "../api/commands";
import { RunnerEngineBrowser } from "./RunnerEngineBrowser";

vi.mock("../api/commands", () => ({
  listEngineRoots: vi.fn(),
  browseEngineRoot: vi.fn(),
}));

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("scoped runner engine browser", () => {
  it("navigates only root-relative entries and registers the selected reference", async () => {
    vi.mocked(commands.listEngineRoots).mockResolvedValue([
      { id: "trusted", label: "Trusted engines" },
    ]);
    vi.mocked(commands.browseEngineRoot).mockImplementation(
      async (request) => ({
        rootId: request.rootId,
        relativePath: request.relativePath,
        nextCursor: null,
        entries:
          request.relativePath === ""
            ? [
                {
                  name: "stable",
                  relativePath: "stable",
                  kind: "directory",
                  size: 0,
                  modifiedAtMs: null,
                  executable: false,
                },
              ]
            : [
                {
                  name: "stockfish",
                  relativePath: "stable/stockfish",
                  kind: "file",
                  size: 1024,
                  modifiedAtMs: 1,
                  executable: true,
                },
              ],
      }),
    );
    const onRegister = vi.fn().mockResolvedValue(true);
    const user = userEvent.setup();
    render(<RunnerEngineBrowser onClose={() => {}} onRegister={onRegister} />);

    await user.click(await screen.findByRole("option", { name: /stable/i }));
    await user.click(await screen.findByRole("option", { name: /stockfish/i }));
    await user.click(
      screen.getByRole("button", { name: "Register selected engine" }),
    );

    expect(onRegister).toHaveBeenCalledWith("trusted", "stable/stockfish");
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(commands.browseEngineRoot).toHaveBeenCalledWith(
      expect.objectContaining({
        rootId: "trusted",
        relativePath: "stable",
      }),
    );
  });

  it("uses the opaque cursor to load another bounded page", async () => {
    vi.mocked(commands.listEngineRoots).mockResolvedValue([
      { id: "trusted", label: "Trusted engines" },
    ]);
    vi.mocked(commands.browseEngineRoot)
      .mockResolvedValueOnce({
        rootId: "trusted",
        relativePath: "",
        entries: [],
        nextCursor: "opaque-cursor",
      })
      .mockResolvedValueOnce({
        rootId: "trusted",
        relativePath: "",
        entries: [],
        nextCursor: null,
      });
    const user = userEvent.setup();
    render(
      <RunnerEngineBrowser
        onClose={() => {}}
        onRegister={() => Promise.resolve(true)}
      />,
    );

    await user.click(await screen.findByRole("button", { name: "Load more" }));
    await waitFor(() =>
      expect(commands.browseEngineRoot).toHaveBeenLastCalledWith(
        expect.objectContaining({ cursor: "opaque-cursor" }),
      ),
    );
  });
});
