import { expect, Page, test } from "@playwright/test";
import { Run } from "../src/pipeline";
import { elf } from "./elf";
import { sceneLabel } from "../src/pipelineScenes";

const currentId = (page: Page) => new URL(page.url()).searchParams.get("run");
const state = async (page: Page, id = currentId(page)): Promise<Run> =>
  (await page.request.get(`/api/pipeline/runs/${id}`)).json();
async function settled(page: Page, id: string) {
  await expect
    .poll(async () => (await state(page, id)).status, { timeout: 90000 })
    .toBe("completed");
  return state(page, id);
}
test.afterEach(async ({ page }) => {
  const id = currentId(page);
  if (id) {
    await page.request.post(`/api/pipeline/runs/${id}/cancel`);
    await expect
      .poll(async () =>
        ["completed", "cancelled", "failed", "interrupted"].includes(
          (await state(page, id)).status,
        ),
      )
      .toBe(true);
  }
});

test("a previously visited card opens a glowing confirmation and restarts the exact Rust server at the selected view", async ({
  page,
}, info) => {
  let restartRequests = 0;
  page.on("request", (request) => {
    if (request.method() === "POST" && request.url().endsWith("/restart"))
      restartRequests++;
  });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto("/");
  await page
    .getByRole("button", { name: "Run interactive pipeline", exact: true })
    .click();
  await expect(page).toHaveURL(/run=/);
  const original = await settled(page, currentId(page)!);
  const footer = page.locator(".focus-transport");
  const before = (await footer.boundingBox())!.height;
  // First visits navigate. Returning to a previously visited card offers restart.
  await page.getByTestId("group-lower").click();
  await expect(page.locator(".timeline-step.is-current")).toHaveAttribute(
    "data-group",
    "lower",
  );
  await page.getByTestId("group-audit").click();
  await expect(page.locator(".timeline-step.is-current")).toHaveAttribute(
    "data-group",
    "audit",
  );
  // Click the illustration, not just the header button.
  await page.locator("#timeline-lower .stage-illustration").click();
  const panel = page.getByRole("region", {
    name: "Restart analysis confirmation",
  });
  await expect(panel).toBeVisible();
  await expect(footer).toHaveAttribute("data-expanded", "true");
  expect((await footer.boundingBox())!.height).toBeGreaterThan(before);
  const confirm = page.getByRole("button", {
    name: `Restart Analysis from ${sceneLabel("lower")} ?`,
    exact: true,
  });
  await expect(confirm).toBeFocused();
  await expect(panel).toContainText("Required earlier stages run again.");
  expect(restartRequests).toBe(0);
  const buttonRect = (await confirm.boundingBox())!,
    barRect = (await footer.boundingBox())!;
  expect(
    Math.abs(
      buttonRect.x + buttonRect.width / 2 - barRect.x - barRect.width / 2,
    ),
  ).toBeLessThan(1);
  expect(
    await confirm.evaluate((el) => getComputedStyle(el).boxShadow),
  ).not.toBe("none");
  await expect(panel).toHaveCSS("opacity", "1");
  await expect(confirm).toHaveCSS("opacity", "1");
  await page.screenshot({
    path: info.outputPath("restart-confirmation-desktop.png"),
  });
  await page.keyboard.press("Escape");
  await expect(panel).toHaveCount(0);
  await expect(page.getByTestId("group-lower")).toBeFocused();
  await page.getByTestId("group-audit").click();
  await expect(page.locator(".timeline-step.is-current")).toHaveAttribute(
    "data-group",
    "audit",
  );
  await expect(panel).toContainText(
    `Restart Analysis from ${sceneLabel("audit")} ?`,
  );
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  expect(restartRequests).toBe(0);
  await page.locator("#timeline-lower .timeline-provenance").click();
  await page.setViewportSize({ width: 390, height: 844 });
  await page
    .locator("#timeline-lower")
    .evaluate((el) => el.scrollIntoView({ block: "center" }));
  await expect(confirm).toBeVisible();
  await expect(panel).toHaveCSS("opacity", "1");
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await page.screenshot({
    path: info.outputPath("restart-confirmation-mobile.png"),
  });
  await confirm.click();
  await expect.poll(() => currentId(page)).not.toBe(original.id);
  const nextId = currentId(page)!;
  await expect(page.locator(".timeline-step.is-current")).toHaveAttribute(
    "data-group",
    "lower",
  );
  expect(restartRequests).toBe(1);
  const next = await settled(page, nextId);
  expect(next.restart).toEqual({
    source_run_id: original.id,
    focus_group: "lower",
    kind: "fresh-execution",
  });
  expect(next.input?.sha256).toBe(original.input?.sha256);
  expect(next.engine_run_id).not.toBe(original.engine_run_id);
  expect(next.groups.ingest.completed).toBeGreaterThan(0);
  expect(next.groups.load.completed).toBeGreaterThan(0);
  expect(next.verification?.first_connection_responses).toBe(12);
  expect(next.verification?.verified_responses).toBe(15);
  expect((await state(page, original.id)).cursor).toBe(original.cursor);
  await info.attach("restart-receipt", {
    body: JSON.stringify({
      source_run: original.id,
      new_run: next.id,
      focus_group: next.restart.focus_group,
      input_sha256: next.input?.sha256,
      fresh_engine_run_id: next.engine_run_id,
      prerequisite_stages_reexecuted: true,
      verification: next.verification,
    }),
    contentType: "application/json",
  });
});

test("card confirmation preserves an active upload until confirmed, then reaps and reruns only that binary", async ({
  page,
}) => {
  await page.goto("/");
  await expect(
    page.getByRole("button", { name: "Inspect your binary" }),
  ).toBeEnabled();
  await page.getByLabel("Choose an x86-64 ELF binary").setInputFiles({
    name: "restart-loop.elf",
    mimeType: "application/octet-stream",
    buffer: elf(
      Buffer.from([
        0xb8, 3, 0, 0, 0, 0xbf, 255, 255, 255, 255, 0x0f, 0x05, 0xeb, 0xf2,
      ]),
    ),
  });
  await expect(page).toHaveURL(/run=/);
  const originalId = currentId(page)!;
  await expect.poll(async () => (await state(page)).cursor).toBeGreaterThan(50);
  await page.getByTestId("group-initialize").click();
  await expect(page.locator(".timeline-step.is-current")).toHaveAttribute(
    "data-group",
    "initialize",
  );
  await page.getByTestId("group-decode").click();
  await expect(page.locator(".timeline-step.is-current")).toHaveAttribute(
    "data-group",
    "decode",
  );
  await page.locator("#timeline-initialize .stage-illustration").click();
  const original = await state(page);
  const panel = page.getByRole("region", {
    name: "Restart analysis confirmation",
  });
  await expect(panel).toContainText("Stops the current run.");
  await expect
    .poll(async () => (await state(page)).cursor)
    .toBeGreaterThan(original.cursor);
  expect((await state(page)).status).toBe("running");
  await page
    .getByRole("button", {
      name: `Restart Analysis from ${sceneLabel("initialize")} ?`,
      exact: true,
    })
    .click();
  await expect.poll(() => currentId(page)).not.toBe(originalId);
  await expect(page.locator(".timeline-step.is-current")).toHaveAttribute(
    "data-group",
    "initialize",
  );
  await expect
    .poll(async () => (await state(page)).counters.rv64_steps ?? 0)
    .toBeGreaterThan(0);
  const next = await state(page),
    old = await state(page, originalId);
  expect(old.status).toBe("cancelled");
  expect(old.engine_result).toBeNull();
  expect(next.input?.sha256).toBe(original.input?.sha256);
  expect(next.engine_run_id).not.toBe(original.engine_run_id);
  expect(next.groups.ingest.completed).toBeGreaterThan(0);
  expect(
    (
      await page.request.post(`/api/pipeline/runs/${originalId}/restart`, {
        data: { focus_group: "invented" },
      })
    ).status(),
  ).toBe(400);
  expect(
    (
      await page.request.post(`/api/pipeline/runs/${originalId}/restart`, {
        data: { focus_group: "ingest" },
      })
    ).status(),
  ).toBe(409);
  expect(
    (
      await page.request.post(`/api/pipeline/runs/${next.id}/restart`, {
        headers: { Origin: "https://invalid.example" },
        data: { focus_group: "ingest" },
      })
    ).status(),
  ).toBe(403);
  expect((await state(page)).status).toBe("running");
});
