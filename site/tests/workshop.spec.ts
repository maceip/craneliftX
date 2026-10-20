import { test, expect } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";

test("all five scenarios explain and render all four states", async ({
  page,
}, info) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("response", (response) => {
    if (response.status() >= 400)
      errors.push(`${response.status()} ${response.url()}`);
  });
  await page.goto("./");
  await expect(page.locator("#heap-canvas")).toHaveAttribute(
    "data-ready",
    "true",
  );
  await expect(page.locator("#scene-fallback")).toBeHidden();
  await page.screenshot({
    path: info.outputPath("desktop.png"),
    fullPage: true,
  });
  const ids = ["neighbor", "callback", "metadata", "off-by-one", "size-wrap"];
  for (let lesson = 0; lesson < 5; lesson++) {
    await page.locator(`button[data-lesson="${lesson}"]`).click();
    for (let step = 0; step < 4; step++) {
      await page.locator(`button[data-step="${step}"]`).click();
      await expect(page.locator("#scene-wrap")).toHaveAttribute(
        "data-step",
        String(step),
      );
      await expect(page.locator("#scene-wrap")).toHaveAttribute(
        "data-lesson",
        ids[lesson],
      );
      await expect(page.locator("#step-text")).not.toBeEmpty();
      await expect(page.locator("#step-action")).not.toBeEmpty();
    }
    const corrupt = await page.locator(".memory-cell.corrupt").count();
    expect(corrupt).toBe([4, 8, 4, 1, 8][lesson]);
    if (lesson === 3)
      await expect(page.locator(".memory-cell.corrupt")).toHaveText("00");
    // Reduced motion settles the scene deterministically for visual evidence.
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page
      .locator(".lab")
      .screenshot({ path: info.outputPath(`${ids[lesson]}.png`) });
    await page.emulateMedia({ reducedMotion: "no-preference" });
  }
  await expect(page.locator("#explored-count")).toHaveText("5 / 5 explored");
  await page.locator('button[data-lesson="0"]').click();
  await expect(page.locator(".memory-cell.corrupt")).toHaveCount(0);
  const accessibility = await new AxeBuilder({ page })
    .withTags(["wcag2a", "wcag2aa", "wcag21aa"])
    .analyze();
  expect(
    accessibility.violations.map((v) => ({
      id: v.id,
      nodes: v.nodes.map((n) => ({
        target: n.target,
        summary: n.failureSummary,
      })),
    })),
  ).toEqual([]);
  expect(errors).toEqual([]);
});

test("mobile layout, scenario links, keyboard steps, and reduced motion", async ({
  page,
}, info) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto("./#off-by-one");
  await expect(page.locator("#lesson-title")).toHaveText("One byte too many");
  await expect(page.locator("#play")).toBeDisabled();
  await page.locator('button[data-step="2"]').focus();
  await page.keyboard.press("Enter");
  await expect(page.locator("#step-action")).toContainText("buffer[8] = 0");
  await expect(page.locator("#play")).toHaveText("Use step controls");
  for (let i = 0; i < 5; i++) {
    await page.locator(`button[data-lesson="${i}"]`).click();
    await page.locator('button[data-step="3"]').click();
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBe(true);
  }
  await page.locator('button[data-lesson="0"]').click();
  await page.screenshot({
    path: info.outputPath("mobile-full.png"),
    fullPage: true,
  });
  await page
    .locator(".lab")
    .screenshot({ path: info.outputPath("mobile-lab.png") });
  const accessibility = await new AxeBuilder({ page })
    .withTags(["wcag2a", "wcag2aa", "wcag21aa"])
    .analyze();
  expect(
    accessibility.violations.map((v) => ({
      id: v.id,
      nodes: v.nodes.map((n) => ({
        target: n.target,
        summary: n.failureSummary,
      })),
    })),
  ).toEqual([]);
});

test("playback advances, resets, and stops offscreen", async ({ page }) => {
  await page.clock.install();
  await page.goto("./");
  await page.locator("#scene-wrap").scrollIntoViewIfNeeded();
  await page.locator("#play").click();
  await page.clock.fastForward(4900);
  await expect(page.locator("#scene-wrap")).toHaveAttribute("data-step", "1");
  await page.locator("#play").click();
  await page.clock.fastForward(6000);
  await expect(page.locator("#scene-wrap")).toHaveAttribute("data-step", "1");
  await page.locator('button[data-step="3"]').click();
  await page.getByRole("button", { name: "Replay scenario" }).click();
  await expect(page.locator("#scene-wrap")).toHaveAttribute("data-step", "0");
  await page.locator("footer").scrollIntoViewIfNeeded();
  await expect(
    page.getByRole("button", { name: "Play scenario", exact: true }),
  ).toBeAttached();
  await page.clock.fastForward(6000);
  await expect(page.locator("#scene-wrap")).toHaveAttribute("data-step", "0");
});

test("WebGL unavailable leaves a usable text and memory workshop", async ({
  page,
}) => {
  await page.addInitScript(() => {
    const getContext = HTMLCanvasElement.prototype.getContext;
    HTMLCanvasElement.prototype.getContext = function (
      type: string,
      ...args: unknown[]
    ) {
      if (type.startsWith("webgl")) return null;
      return getContext.call(this, type, ...args);
    } as typeof getContext;
  });
  await page.goto("./#callback");
  await expect(page.locator("#scene-fallback")).toBeVisible();
  await page.locator('button[data-step="3"]').click();
  await expect(page.locator("#scene-fallback")).toContainText(
    "attacker-selected callable target",
  );
  await expect(page.locator(".memory-cell.corrupt")).toHaveCount(8);
});

test("context loss preserves controls and restoration redraws the current lesson", async ({
  page,
}) => {
  await page.goto("./");
  await expect(page.locator("#heap-canvas")).toHaveAttribute(
    "data-ready",
    "true",
  );
  await page.evaluate(() => {
    const gl = document.querySelector("canvas")!.getContext("webgl2")!;
    const extension = gl.getExtension("WEBGL_lose_context")!;
    (
      window as typeof window & { restoreTestContext: () => void }
    ).restoreTestContext = () => extension.restoreContext();
    extension.loseContext();
  });
  await expect(page.locator("#scene-fallback")).toBeVisible();
  await page.locator('button[data-lesson="3"]').click();
  await page.locator('button[data-step="2"]').click();
  await page.evaluate(() =>
    (
      window as typeof window & { restoreTestContext: () => void }
    ).restoreTestContext(),
  );
  await expect(page.locator("#scene-fallback")).toBeHidden();
  await expect(page.locator("#scene-wrap")).toHaveAttribute(
    "data-lesson",
    "off-by-one",
  );
  await expect(page.locator("#scene-wrap")).toHaveAttribute("data-step", "2");
});
