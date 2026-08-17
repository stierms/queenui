import { expect, test } from "@playwright/test";

const viewports = [
  { name: "wide desktop", width: 2560, height: 1440 },
  { name: "standard desktop", width: 1280, height: 900 },
  { name: "compact desktop", width: 1000, height: 800 },
];

for (const viewport of viewports) {
  test(`${viewport.name} uses its available space without clipping`, async ({
    page,
  }) => {
    await page.setViewportSize(viewport);
    await page.goto("/?game-preview");

    await expect(page.getByRole("heading", { name: "Overview" })).toBeVisible();
    await expect(page.getByLabel("Current chess position")).toBeVisible();

    const overflow = await page.evaluate(() => ({
      viewport: document.documentElement.clientWidth,
      document: document.documentElement.scrollWidth,
      body: document.body.scrollWidth,
    }));
    expect(overflow.document).toBeLessThanOrEqual(overflow.viewport + 1);
    expect(overflow.body).toBeLessThanOrEqual(overflow.viewport + 1);

    const smallestOperationalText = await page
      .locator(".telemetry-grid span")
      .first()
      .evaluate((element) =>
        Number.parseFloat(getComputedStyle(element).fontSize),
      );
    expect(smallestOperationalText).toBeGreaterThanOrEqual(10);
  });
}

test("wide desktop expands the workspace and board", async ({ page }) => {
  await page.setViewportSize({ width: 2560, height: 1440 });
  await page.goto("/?game-preview");

  // The workspace is width-capped so the board — not stretched panels —
  // absorbs a wide desktop: content centers at the cap while the board
  // grows with the tall viewport.
  const content = await page.locator(".dashboard-content").boundingBox();
  const board = await page.locator(".board").boundingBox();
  expect(content?.width).toBeGreaterThan(1600);
  expect(content?.width).toBeLessThanOrEqual(1800);
  expect(board?.width).toBeGreaterThanOrEqual(800);
});

test("compact desktop uses the icon rail and stacks game detail", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1000, height: 800 });
  await page.goto("/?game-preview");

  const sidebar = await page.locator(".sidebar").boundingBox();
  const boardArea = await page.locator(".board-wrap").boundingBox();
  const details = await page.locator(".game-details").boundingBox();
  expect(sidebar?.width).toBeLessThanOrEqual(85);
  expect(details?.y).toBeGreaterThanOrEqual(
    (boardArea?.y ?? 0) + (boardArea?.height ?? 0) - 1,
  );
});

test("challenge dialog is named, keyboard dismissible, and restores focus", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto("/?game-preview");

  const trigger = page.getByRole("button", { name: "New challenge" });
  await trigger.click();

  const dialog = page.getByRole("dialog", { name: "Create a challenge" });
  await expect(dialog).toBeVisible();
  await expect(
    dialog.getByText(
      "The selected account is connected before the challenge is sent.",
    ),
  ).toBeVisible();

  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  await expect(trigger).toBeFocused();
});

test("board appearance popover remains inside the compact viewport", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1000, height: 800 });
  await page.goto("/?game-preview");

  await page.getByRole("button", { name: "Export PGN" }).hover();
  await expect(page.getByRole("tooltip", { name: "Export PGN" })).toBeVisible();
  await page.mouse.move(2, 2);

  const trigger = page.getByRole("button", { name: "Board appearance" });
  await trigger.click();
  const popover = page.locator(".appearance-popover");
  await expect(popover).toBeVisible();

  const bounds = await popover.boundingBox();
  expect(bounds?.x).toBeGreaterThanOrEqual(0);
  expect((bounds?.x ?? 0) + (bounds?.width ?? 0)).toBeLessThanOrEqual(1000);
  expect(bounds?.y).toBeGreaterThanOrEqual(0);
  expect((bounds?.y ?? 0) + (bounds?.height ?? 0)).toBeLessThanOrEqual(800);

  await page.keyboard.press("Escape");
  await expect(popover).toBeHidden();
  await expect(trigger).toBeFocused();
});

test("settings is a functional compact-screen destination", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1000, height: 800 });
  await page.goto("/?game-preview");

  await page.getByRole("button", { name: "Settings" }).click();
  await expect(
    page.getByRole("heading", { name: "Board and pieces" }),
  ).toBeVisible();

  await page.getByRole("button", { name: "Walnut" }).click();
  await page.getByRole("button", { name: /Ink/ }).click();
  /*
   * The swatch beside this text is `aria-hidden` on purpose: its old
   * `aria-label` sat on a bare `<div>`, where name computation drops it, and
   * the selection is stated as visible text next to it (SettingsPage.tsx,
   * "Current presentation"). This asserts the text that is actually there.
   */
  await expect(page.getByText("Walnut · Ink")).toBeVisible();

  const overflow = await page.evaluate(() => ({
    viewport: document.documentElement.clientWidth,
    document: document.documentElement.scrollWidth,
  }));
  expect(overflow.document).toBeLessThanOrEqual(overflow.viewport + 1);
});

test("engine configuration fits compact desktop layouts", async ({ page }) => {
  await page.setViewportSize({ width: 1000, height: 800 });
  await page.goto("/?game-preview");

  await page.getByRole("button", { name: "Engines" }).click();
  await page.getByRole("button", { name: "Configure" }).click();
  const dialog = page.getByRole("dialog", {
    name: "Configure Queen 0.42 NNUE",
  });
  await expect(dialog).toBeVisible();

  const bounds = await dialog.boundingBox();
  expect(bounds?.x).toBeGreaterThanOrEqual(0);
  expect((bounds?.x ?? 0) + (bounds?.width ?? 0)).toBeLessThanOrEqual(1000);
  expect(bounds?.y).toBeGreaterThanOrEqual(0);
  expect((bounds?.y ?? 0) + (bounds?.height ?? 0)).toBeLessThanOrEqual(800);

  await dialog.getByRole("tab", { name: /UCI options/ }).click();
  await expect(
    dialog.getByRole("button", { name: "Re-probe engine" }),
  ).toBeVisible();
  const optionList = dialog.locator(".uci-option-list");
  await expect(optionList).toBeVisible();
  expect(
    await optionList.evaluate(
      (element) => element.scrollHeight > element.clientHeight,
    ),
  ).toBe(true);
  const save = dialog.getByRole("button", { name: "Save UCI options" });
  await expect(save).toBeVisible();
  await optionList.evaluate((element) => {
    element.scrollTop = element.scrollHeight;
  });
  await expect(save).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
});

test("the games page keeps its headers docked while the list scrolls", async ({
  page,
}) => {
  await page.setViewportSize({ width: 2560, height: 1440 });
  await page.goto("/?games-preview");

  await expect(page.getByRole("heading", { name: "All games" })).toBeVisible();
  const topbar = page.locator(".topbar");
  const toolbar = page.locator(".games-toolbar");
  const topbarHeight = (await topbar.boundingBox())?.height ?? 0;

  await page.evaluate(() => window.scrollTo(0, 1200));
  await page.waitForFunction(() => window.scrollY > 600);

  // Both headers stay pinned: the topbar at the viewport top, the games
  // toolbar immediately below it.
  expect((await topbar.boundingBox())?.y).toBeCloseTo(0, 0);
  expect((await toolbar.boundingBox())?.y).toBeCloseTo(topbarHeight, 0);
  await expect(
    page.getByRole("heading", { name: "Games", exact: true }),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: /^All/ })).toBeVisible();
});

test("the logs viewer windows a large session instead of mounting it", async ({
  page,
}) => {
  await page.setViewportSize({ width: 2560, height: 1440 });
  await page.goto("/?logs-preview");

  await expect(
    page.getByRole("tab", { name: /Engine sessions/ }),
  ).toBeVisible();
  const canvas = page.locator(".log-canvas");
  await expect(canvas).toBeVisible();

  // The preview session runs to thousands of lines; only a window of rows may
  // ever be in the DOM, or a real 20 000-line session would lock the app up.
  const totalLines = await page
    .locator(".logs-session-numbers span")
    .first()
    .innerText();
  expect(
    Number.parseInt(totalLines.replace(/[^0-9]/g, ""), 10),
  ).toBeGreaterThan(1000);
  expect(await canvas.locator("> *").count()).toBeLessThan(200);

  const overflow = await page.evaluate(() => ({
    viewport: document.documentElement.clientWidth,
    document: document.documentElement.scrollWidth,
  }));
  expect(overflow.document).toBeLessThanOrEqual(overflow.viewport + 1);

  await page.getByRole("tab", { name: /App diagnostics/ }).click();
  await expect(
    page.getByText("Event stream reconnected after 2 attempts"),
  ).toBeVisible();
});
