import { test, expect } from "@playwright/test";
import { elf } from "./elf";
import type { LedgerEvent, Run } from "../src/pipeline";

test.use({ video: { mode: "on", size: { width: 1440, height: 1000 } } });
for (const viewport of [
  { width: 1440, height: 1000, name: "desktop" },
  { width: 390, height: 844, name: "phone" },
]) {
  test(`recorded operations visibly transform and the clock works on ${viewport.name}`, async ({
    page,
  }, info) => {
    test.setTimeout(240_000);
    await page.setViewportSize(viewport);
    const errors: string[] = [];
    page.on("pageerror", (e) => errors.push(e.message));
    await page.goto("/");
    await page
      .getByRole("button", { name: "Run interactive pipeline", exact: true })
      .click();
    await expect(page).toHaveURL(/run=/);
    const id = new URL(page.url()).searchParams.get("run");
    const snapshot = async (): Promise<Run> =>
      (await page.request.get(`/api/pipeline/runs/${id}`)).json();
    const gauge = page.getByTestId("telemetry-gauge");
    const hand = gauge.locator(".engine-hand");
    await expect(gauge).toHaveAttribute("data-clock-mode", "live");
    const first = await hand.getAttribute("style");
    const firstMarker = await gauge.locator(".dial-position").boundingBox();
    await expect.poll(() => hand.getAttribute("style")).not.toBe(first);
    await expect
      .poll(async () => {
        const marker = await gauge.locator(".dial-position").boundingBox();
        return marker && firstMarker
          ? Math.hypot(marker.x - firstMarker.x, marker.y - firstMarker.y)
          : 0;
      })
      .toBeGreaterThan(1);
    await expect
      .poll(async () => (await snapshot()).status, { timeout: 120000 })
      .toBe("completed");
    const run = await snapshot();
    const journal: LedgerEvent[] = (
      await (await page.request.get(`/api/pipeline/runs/${id}/journal`)).text()
    )
      .trim()
      .split("\n")
      .map((line) => JSON.parse(line));
    await expect(gauge).toHaveAttribute(
      "data-elapsed-ns",
      String(run.elapsed_ns),
    );
    await expect(gauge).toHaveAttribute("data-clock-mode", "recorded");
    const seconds = await hand.getAttribute("style");
    const scenes = [
      "initialize",
      "decode",
      "audit",
      "lower",
      "codegen",
      "execute",
      "syscall",
      "result",
      "ingest",
    ];
    const factory = page.locator(".factory-backdrop canvas");
    await expect
      .poll(() => factory.evaluate((c: any) => !!c.factoryState))
      .toBe(true);
    await factory.evaluate(
      (c: any) => ((window as any).originalFactoryCanvas = c),
    );
    const untouched = await factory.evaluate(
      (c: any) => c.factoryState().clocks,
    );
    expect(
      Object.entries(untouched)
        .filter(([id]) => id !== "ingest")
        .every(([, time]) => time === 0),
    ).toBe(true);
    for (const scene of scenes) {
      await page.getByTestId(`group-${scene}`).focus();
      const card = page.locator(`#timeline-${scene}`),
        story = card.locator(".card-story");
      await expect(card).toHaveClass(/is-current/);
      await expect(story).toHaveAttribute("data-phase", "0");
      const cue = card.locator(".continue-cue");
      if (scene !== "result") {
        await expect(cue).toHaveAttribute("data-visible", "false");
        await expect(cue).toBeDisabled();
      } else await expect(cue).toHaveCount(0);
      await expect(gauge).toHaveAttribute("data-walkthrough-progress", /^0\./);
      await expect
        .poll(() => factory.evaluate((c: any) => c.factoryState().scene))
        .toBe(scene);
      expect(
        await factory.evaluate(
          (c: any) => c === (window as any).originalFactoryCanvas,
        ),
      ).toBe(true);
      // Capture before, operation, and result, not just a settled screenshot.
      await page.screenshot({ path: info.outputPath(`${scene}-before.png`) });
      if (scene === "initialize") {
        await expect(
          card.locator('.memory-region[data-populated="true"]'),
        ).toHaveCount(0);
        const permissions = card.locator(".region-permission").first();
        expect(
          Number(
            await permissions.evaluate((el) => getComputedStyle(el).opacity),
          ),
        ).toBeLessThan(0.2);
      }
      if (scene === "syscall") {
        expect(
          Number(
            await card
              .locator(".response-packet")
              .evaluate((el) => getComputedStyle(el).opacity),
          ),
        ).toBe(0);
      }
      await expect(story).toHaveAttribute("data-phase", "2");
      if (scene !== "result") await expect(cue).toHaveCSS("opacity", "0");
      await page.screenshot({
        path: info.outputPath(`${scene}-operation.png`),
      });
      if (scene === "initialize") {
        await page.getByRole("button", { name: "Pause animations" }).click();
        const progress = await gauge.getAttribute("data-walkthrough-progress");
        const clocks = await factory.evaluate(
          (c: any) => c.factoryState().clocks,
        );
        await page.waitForTimeout(350);
        expect(await gauge.getAttribute("data-walkthrough-progress")).toBe(
          progress,
        );
        expect(
          await factory.evaluate((c: any) => c.factoryState().clocks),
        ).toEqual(clocks);
        await page.getByRole("button", { name: "Resume animations" }).click();
      }
      await expect(story).toHaveAttribute("data-phase", "4");
      await expect(gauge).toHaveAttribute("data-walkthrough-progress", "1.000");
      if (scene !== "result") {
        await expect(cue).toHaveAttribute("data-visible", "true");
        await expect(cue).toHaveCSS("opacity", "1");
        await expect(
          page.locator('.continue-cue[data-visible="true"]'),
        ).toHaveCount(1);
      } else
        await expect(
          page.locator('.continue-cue[data-visible="true"]'),
        ).toHaveCount(0);
      await page.screenshot({ path: info.outputPath(`${scene}-result.png`) });
      expect(await hand.getAttribute("style")).toBe(seconds);
      expect(
        await page.evaluate(
          () => document.documentElement.scrollWidth <= innerWidth,
        ),
      ).toBe(true);
      if (scene === "initialize") {
        const segments = run.groups.load.output!.details.segments;
        await expect(
          card.locator('.memory-region[data-populated="true"]'),
        ).toHaveCount(segments.length);
        const zero = segments.find((s: any) => s.file_bytes === 0);
        expect(zero).toBeTruthy();
        await card
          .getByRole("button", {
            name: `Inspect memory region ${zero.address}`,
          })
          .click();
        await expect(card.locator(".memory-story")).toHaveAttribute(
          "data-inspected-region",
          zero.address,
        );
        await expect(card.locator(".region-detail")).toContainText("0 B file");
        await expect(card.locator(".state-slots i")).toHaveCount(
          run.groups.initialize.output!.state_words,
        );
        const transform = await card
          .locator(".region-zero")
          .last()
          .evaluate((el) => getComputedStyle(el).transform);
        expect(transform).toBe("matrix(1, 0, 0, 1, 0, 0)");
      }
      if (scene === "decode") {
        const instructions = run.groups.decode.output!.instructions;
        await card
          .getByRole("button", {
            name: `Inspect instruction ${instructions[0].ip}`,
            exact: true,
          })
          .click();
        await expect(card.locator(".decode-story")).toHaveAttribute(
          "data-selected-ip",
          instructions[0].ip,
        );
        expect(await card.locator(".opcode-track li").count()).toBe(
          instructions.length,
        );
      }
      if (scene === "audit") {
        const checks = run.groups.audit.output!.checks;
        expect(await card.locator(".policy-gate").count()).toBe(checks.length);
        for (const check of checks)
          await expect(
            card.locator(`.policy-${check.verdict}`).first(),
          ).toHaveAttribute("data-ready", "true");
        await expect(card.locator(".policy-guarded").first()).toContainText(
          "Runtime guard armed",
        );
      }
      if (scene === "lower") {
        const preview = card.locator(".artifact-preview"),
          name = await preview.getAttribute("data-artifact");
        const clif = await (
          await page.request.get(`/api/pipeline/runs/${id}/artifacts/${name}`)
        ).text();
        for (const node of await card.locator(".ir-graph code").all())
          expect(clif).toContain(await node.textContent());
        expect(await card.locator(".ir-graph code").count()).toBeGreaterThan(1);
      }
      if (scene === "codegen") {
        const nodes = card.locator(".emitted-row");
        await expect(nodes).toHaveCount(6);
        for (const node of await nodes.all())
          expect(
            Number(await node.evaluate((el) => getComputedStyle(el).opacity)),
          ).toBe(1);
      }
      if (scene === "execute") {
        const blocks = new Map<number, any>();
        for (const event of journal.filter(
          (e) => e.type === "block_execution_totals",
        ))
          for (const block of event.blocks) blocks.set(block.block_id, block);
        const tiles = await card
          .locator(".block-field button")
          .evaluateAll((items) =>
            items.map((el) => ({
              id: Number((el as HTMLElement).dataset.block),
              work: Number((el as HTMLElement).dataset.work),
            })),
          );
        expect(tiles.length).toBeGreaterThan(1);
        for (const tile of tiles)
          expect(tile.work).toBe(blocks.get(tile.id).rv64_steps);
      }
      if (scene === "syscall") {
        const selected = Number(
          await card.locator(".syscall-story").getAttribute("data-source-seq"),
        );
        const event = journal.find((e) => e.seq === selected)!;
        expect(["write", "read"]).toContain(event.detail.name);
        expect(event.detail.result).toBeGreaterThan(0);
        await expect(card.locator(".response-packet")).toContainText(
          String(event.detail.result),
        );
        expect(
          Number(
            await card
              .locator(".response-packet")
              .evaluate((el) => getComputedStyle(el).opacity),
          ),
        ).toBe(1);
      }
      if (scene === "result") {
        await expect(card.locator(".matrix-network")).toContainText(
          "18 responses checked",
        );
        expect(
          Number(
            await card
              .locator(".matrix-network")
              .evaluate((el) => getComputedStyle(el).opacity),
          ),
        ).toBe(1);
        expect(
          await card
            .locator(".performance-summary")
            .evaluate((el) => el.scrollHeight <= el.clientHeight + 1),
        ).toBe(true);
      }
    }
    // The completion cue fades while the same card is leaving its reading
    // position, before the next card is selected. Returning restores it.
    const cue = page.locator("#timeline-ingest .continue-cue");
    await page.screenshot({ path: info.outputPath("continue-cue.png") });
    const bar = (await page.locator(".focus-transport").boundingBox())!;
    const cueBox = (await cue.boundingBox())!;
    expect(cueBox.y + cueBox.height).toBeLessThan(bar.y);
    expect(bar.height).toBeLessThanOrEqual(56);
    expect(bar.width).toBeLessThanOrEqual(170);
    await cue.evaluate((el) => {
      (window as any).cueFade = [];
      const until = performance.now() + 500;
      function frame() {
        (window as any).cueFade.push(Number(getComputedStyle(el).opacity));
        if (performance.now() < until) requestAnimationFrame(frame);
      }
      requestAnimationFrame(frame);
    });
    await page.mouse.move(viewport.width / 2, viewport.height / 2);
    await page.mouse.wheel(0, 48);
    await expect(page.locator(".timeline-step.is-current")).toHaveAttribute(
      "data-group",
      "ingest",
    );
    await expect(cue).toHaveCSS("opacity", "0");
    expect(
      await page.evaluate(() =>
        (window as any).cueFade.some(
          (opacity: number) => opacity > 0 && opacity < 1,
        ),
      ),
    ).toBe(true);
    await page.mouse.wheel(0, -48);
    await expect(cue).toHaveCSS("opacity", "1");
    await cue.click();
    await expect(page.locator(".timeline-step.is-current")).toHaveAttribute(
      "data-group",
      "initialize",
    );
    const nextCue = page.locator("#timeline-initialize .continue-cue");
    await expect(nextCue).toHaveCSS("opacity", "0");
    await expect(nextCue).toHaveCSS("opacity", "1");
    await page.getByRole("button", { name: "Pause animations" }).click();
    // CSS pauses settle on the compositor's next frame. Wait for that commit
    // before comparing the held pose, rather than sampling the click frame.
    await expect
      .poll(() =>
        nextCue
          .locator(".cue-motion")
          .evaluate((el) =>
            el
              .getAnimations()
              .every(
                (animation) =>
                  animation.playState === "paused" && !animation.pending,
              ),
          ),
      )
      .toBe(true);
    const pausedCue = await nextCue
      .locator(".cue-motion")
      .evaluate((el) => getComputedStyle(el).transform);
    await page.waitForTimeout(250);
    expect(
      await nextCue
        .locator(".cue-motion")
        .evaluate((el) => getComputedStyle(el).transform),
    ).toBe(pausedCue);
    await page.getByRole("button", { name: "Resume animations" }).click();
    await nextCue.press("Enter");
    await expect(page.locator(".timeline-step.is-current")).toHaveAttribute(
      "data-group",
      "decode",
    );
    expect(new URL(page.url()).searchParams.get("run")).toBe(id);
    // The exact same saved run must open with a fresh, reader-controlled scene.
    await page.reload();
    await page.getByTestId("group-initialize").focus();
    await expect(
      page.locator("#timeline-initialize .card-story"),
    ).toHaveAttribute("data-phase", "0");
    await page.emulateMedia({ reducedMotion: "reduce" });
    await expect(
      page.locator("#timeline-initialize .card-story"),
    ).toHaveAttribute("data-phase", "4");
    await expect(page.locator("#timeline-initialize .continue-cue")).toHaveCSS(
      "opacity",
      "1",
    );
    expect(
      await page
        .locator("#timeline-initialize .continue-cue")
        .evaluate((el) => el.getAnimations({ subtree: true }).length),
    ).toBe(0);
    await expect
      .poll(() => factory.evaluate((c: any) => c.factoryState?.().running))
      .toBe(false);
    await page.setViewportSize({ width: 320, height: 700 });
    await page.screenshot({ path: info.outputPath("reduced-small-phone.png") });
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBe(true);
    expect(errors).toEqual([]);
  });
}

test("different real inputs change the depicted byte groups and memory regions", async ({
  page,
}) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  const volumes = [];
  for (const size of [512, 16384]) {
    await page.goto("/");
    await expect(
      page.getByRole("button", { name: "Inspect your binary" }),
    ).toBeEnabled();
    const program = elf(
      Buffer.from([0xb8, 60, 0, 0, 0, 0xbf, 0, 0, 0, 0, 0x0f, 0x05]),
      size,
    );
    await page.locator('input[type="file"]').setInputFiles({
      name: `size-${size}.elf`,
      mimeType: "application/octet-stream",
      buffer: program,
    });
    await expect(page).toHaveURL(/run=/);
    const id = new URL(page.url()).searchParams.get("run");
    await expect
      .poll(
        async () =>
          (await (await page.request.get(`/api/pipeline/runs/${id}`)).json())
            .status,
      )
      .toBe("completed");
    const count = await page.locator(".file-chunks i").count();
    volumes.push(count);
    await expect(page.locator(".file-chunks")).toHaveAttribute(
      "aria-label",
      `${count} groups representing ${size} file bytes`,
    );
    await expect(page.locator(".memory-region")).toHaveCount(1);
    await expect(page.locator(".memory-region")).toHaveAttribute(
      "data-bytes",
      String(size),
    );
    await expect(page.locator(".matrix-network")).toHaveCount(0);
  }
  expect(volumes[1]).toBeGreaterThan(volumes[0]);
});
