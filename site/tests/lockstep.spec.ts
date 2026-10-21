import { test, expect } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
const ids = [
  "service",
  "overflow",
  "data",
  "metadata",
  "byte",
  "wrap",
  "tags",
  "quarantine",
  "placement",
  "pac",
  "cfi",
  "diversify",
  "isolation",
  "receipt",
];
test("linked scenes, assets, keyboard and idle rendering", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("response", (r) => {
    if (r.status() >= 400) errors.push(r.url());
  });
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto("./");
  await expect(page.locator(".chapter")).toHaveCount(14);
  for (const id of ids) {
    await page.locator('#chapter-rail a[href="#' + id + '"]').click();
    await expect(page.locator("#diagram")).toHaveAttribute("data-scene", id);
    const heading = page.locator("#heading-" + id);
    await expect(heading).toBeInViewport();
    const position = await heading.boundingBox();
    expect(position!.y).toBeGreaterThan(95);
    expect(position!.y).toBeLessThan(300);
  }
  await expect(page.getByRole("button", { name: "Next scene" })).toBeDisabled();
  await page.getByRole("button", { name: "Previous scene" }).focus();
  await page.keyboard.press("Enter");
  await expect(page.locator("#diagram")).toHaveAttribute(
    "data-scene",
    "isolation",
  );
  const frames = await page.locator("canvas").getAttribute("data-frames");
  await page.waitForTimeout(250);
  expect(await page.locator("canvas").getAttribute("data-frames")).toBe(frames);
  expect(await page.evaluate(() => document.fonts.check("30px Kesan"))).toBe(
    true,
  );
  expect(errors).toEqual([]);
});
test("natural scrolling synchronizes in both directions", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto("./");
  for (const id of ["overflow", "tags", "pac", "receipt", "data", "service"]) {
    await page
      .locator("#" + id)
      .evaluate((e) =>
        e.scrollIntoView({ block: "start", behavior: "instant" }),
      );
    await expect(page.locator("#diagram")).toHaveAttribute("data-scene", id);
  }
});
for (const viewport of [
  { width: 390, height: 844 },
  { width: 375, height: 667 },
  { width: 1440, height: 1000 },
  { width: 1024, height: 600 },
]) {
  test(
    "responsive and accessible " + viewport.width + "x" + viewport.height,
    async ({ page }) => {
      await page.setViewportSize(viewport);
      await page.emulateMedia({ reducedMotion: "reduce" });
      await page.goto("./#tags");
      await expect(page.locator("#diagram")).toHaveAttribute(
        "data-scene",
        "tags",
      );
      await expect(page.locator("#heading-tags")).toBeInViewport();
      const heading = await page.locator("#heading-tags").boundingBox(),
        stage = await page.locator(".memory-stage").boundingBox();
      if (viewport.width < 760)
        expect(heading!.y).toBeGreaterThanOrEqual(stage!.y + stage!.height);
      expect(
        await page.evaluate(
          () => document.documentElement.scrollWidth <= innerWidth,
        ),
      ).toBe(true);
      await page.screenshot({
        path:
          "test-results/lockstep-" +
          viewport.width +
          "x" +
          viewport.height +
          ".png",
      });
      expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
    },
  );
}
test("WebGL failure retains the story and colored heap", async ({ page }) => {
  await page.addInitScript(() => {
    const getContext = HTMLCanvasElement.prototype.getContext;
    HTMLCanvasElement.prototype.getContext = function (
      type: string,
      ...args: any[]
    ) {
      if (type.includes("webgl")) return null;
      return (getContext as any).call(this, type, ...args);
    } as any;
  });
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto("./#overflow");
  await expect(page.locator("#flat-memory")).toBeVisible();
  await expect(page.locator("#flat-memory .changed")).toHaveCount(2);
  await page.getByRole("button", { name: "Next scene" }).click();
  await expect(page.locator("#diagram")).toHaveAttribute("data-scene", "data");
  await expect(page.locator("#flat-memory .changed")).toHaveCount(1);
  await page.locator('#chapter-rail a[href="#wrap"]').click();
  await expect(page.locator("#flat-memory .changed")).toHaveCount(32);
});
test("context restoration preserves current scene", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto("./#overflow");
  await page.locator("canvas").evaluate((c: HTMLCanvasElement) => {
    const ext = c.getContext("webgl2")!.getExtension("WEBGL_lose_context")!;
    (window as any).restoreContext = () => ext.restoreContext();
    ext.loseContext();
  });
  await expect(page.locator("#flat-memory")).toBeVisible();
  await page.evaluate(() => (window as any).restoreContext());
  await expect(page.locator("#flat-memory")).toBeHidden();
  await expect(page.locator("#diagram")).toHaveAttribute(
    "data-scene",
    "overflow",
  );
});
test("entry animation stops and motion control persists", async ({ page }) => {
  await page.goto("./#overflow");
  await expect(page.locator("canvas")).toHaveAttribute(
    "data-animating",
    "false",
  );
  await page
    .getByRole("button", { name: "Pause typing and transitions" })
    .click();
  await page.getByRole("button", { name: "Next scene" }).click();
  await expect(page.locator("#diagram")).toHaveAttribute("data-scene", "data");
  await expect(page.locator("canvas")).toHaveAttribute(
    "data-animating",
    "false",
  );
  await expect(page.locator("#data .typed-text")).toHaveText(
    "overflow.reaches = permission",
  );
});

test("manual scrolling during an anchor transition resynchronizes the scene", async ({
  page,
}) => {
  await page.goto("./#service");
  await page.locator('#chapter-rail a[href="#receipt"]').click();
  await page
    .locator("#overflow")
    .evaluate((e) => e.scrollIntoView({ behavior: "instant", block: "start" }));
  await expect(page.locator("#diagram")).toHaveAttribute(
    "data-scene",
    "overflow",
  );
});
