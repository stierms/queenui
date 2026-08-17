import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests/browser",
  fullyParallel: true,
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? "github" : "line",
  projects: [
    process.platform === "win32"
      ? { name: "msedge", use: { channel: "msedge" } }
      : { name: "chromium", use: { browserName: "chromium" } },
  ],
  use: {
    baseURL: "http://127.0.0.1:1420",
    headless: true,
  },
  webServer: {
    command: "npm run dev -- --host 127.0.0.1",
    url: "http://127.0.0.1:1420",
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
