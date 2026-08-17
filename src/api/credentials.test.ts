import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { forgetRunnerCredential, removeLichessAccount } from "./credentials";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

describe("credential IPC boundary", () => {
  beforeEach(() => vi.mocked(invoke).mockReset());

  it("sends the exact account-removal command and arguments", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    await removeLichessAccount("queenbot");
    expect(invoke).toHaveBeenCalledWith("remove_lichess_account", {
      accountId: "queenbot",
    });
  });

  it("sends the argument-free runner-forget command", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    await forgetRunnerCredential();
    expect(invoke).toHaveBeenCalledWith("forget_runner_credential");
  });
});
