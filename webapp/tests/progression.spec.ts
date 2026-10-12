import { test, expect } from "@playwright/test";
import { Run } from "../src/pipeline";

// Keep successful walkthroughs too: passing assertions do not replace watching
// the actual desktop/mobile transitions and checking readability.
test.use({ video: { mode: "on", size: { width: 1440, height: 1000 } } });
for (const viewport of [
  { name: "desktop", width: 1440, height: 1000 },
  { name: "mobile", width: 390, height: 844 },
]) {
  test.describe(viewport.name, () => {
    test.use({ viewport, hasTouch: viewport.name === "mobile" });
    test(`manual inspection keeps the real run independent on ${viewport.name}`, async ({
      page,
    }, info) => {
      await page.setViewportSize(viewport);
      await page.addInitScript(() => {
        (window as any).stageChanges = [];
        new MutationObserver(() => {
          const id = document.querySelector<HTMLElement>(
            ".timeline-step.is-current",
          )?.dataset.group;
          const changes: string[] = (window as any).stageChanges;
          if (id && changes.at(-1) !== id) changes.push(id);
        }).observe(document, {
          subtree: true,
          attributes: true,
          childList: true,
        });
      });
      const writes: string[] = [];
      page.on("request", (r) => {
        if (r.method() === "POST") writes.push(new URL(r.url()).pathname);
      });
      await page.goto("/");
      await expect(page.getByRole("heading", { level: 1 })).toHaveText(
        "Zero-trust binary execution via live ISA re-targeting.",
      );
      await page.screenshot({
        path: info.outputPath(`launch-${viewport.name}.png`),
      });
      await page
        .getByRole("button", { name: "Run interactive pipeline", exact: true })
        .click();
      await expect(page).toHaveURL(/run=/);
      const id = new URL(page.url()).searchParams.get("run");
      const current = page.locator(".timeline-step.is-current");
      const ring = page.getByTestId("stage-thermometer");
      const snapshot = async (): Promise<Run> =>
        (await page.request.get(`/api/pipeline/runs/${id}`)).json();
      try {
        await expect(current).toHaveAttribute("data-group", "run");
        const firstCursor = Number(await ring.getAttribute("data-cursor"));
        const initialScroll = await page.evaluate(() => scrollY);
        await expect
          .poll(async () => (await snapshot()).status, { timeout: 30000 })
          .toBe("completed");
        const final = await snapshot();
        expect(final.cursor).toBeGreaterThan(firstCursor);
        expect(final.verification?.verified_responses).toBe(15);
        expect(final.verification?.first_connection_responses).toBe(12);
        // Deliberately outwait both rejected 4s/6s timers, without any input.
        await page.waitForTimeout(7000);
        await expect(current).toHaveAttribute("data-group", "run");
        expect(await page.evaluate(() => scrollY)).toBe(initialScroll);
        await expect(page.getByRole("status")).toContainText(
          "Recorded results",
        );
        await expect(page.getByTestId("telemetry-gauge")).toHaveAttribute(
          "data-elapsed-ns",
          String(final.elapsed_ns),
        );
        const groups = Object.values(final.groups).sort(
          (a, b) => a.order - b.order,
        );
        const visits: { group: string; seq: number }[] = [];
        for (let i = 0; i < groups.length; i++) {
          const group = groups[i];
          const step = page.locator(`#timeline-${group.id}`);
          if (i % 2) {
            // A real wheel gesture (also supported by a mobile-sized viewport).
            const delta = await step.evaluate((el) => {
              const r = el.getBoundingClientRect();
              return r.top + r.height / 2 - innerHeight / 2;
            });
            await page.mouse.move(viewport.width / 2, viewport.height / 2);
            await page.mouse.wheel(0, delta);
          } else if (i) {
            const button = page.getByTestId(`group-${group.id}`);
            if (viewport.name === "mobile") await button.tap();
            else if (i === 6) {
              await button.focus();
              await button.press("Enter");
            } else await button.click();
          }
          await expect(current).toHaveAttribute("data-group", group.id);
          await expect(ring).toHaveAttribute("data-current-stage", group.id);
          await expect(
            page.locator('.stage-thermometer [aria-current="step"]'),
          ).toHaveCount(1);
          expect(
            await page
              .locator(
                '.stage-thermometer [aria-current="step"] .thermometer-dot',
              )
              .evaluate((el) => getComputedStyle(el, "::after").boxShadow),
          ).not.toBe("none");
          await expect(
            page.getByRole("region", { name: "Restart analysis confirmation" }),
          ).toHaveCount(0);
          await expect
            .poll(() =>
              step.evaluate((el) => {
                const r = el.getBoundingClientRect();
                return Math.abs(r.top + r.height / 2 - innerHeight / 2);
              }),
            )
            .toBeLessThan(4);
          await expect
            .poll(() =>
              step
                .locator(".timeline-card")
                .evaluate((el) => Number(getComputedStyle(el).opacity)),
            )
            .toBe(1);
          expect(
            await page.evaluate(
              () => document.documentElement.scrollWidth <= innerWidth,
            ),
          ).toBe(true);
          const seq = Number(await step.getAttribute("data-motion-seq"));
          expect(seq).toBe(group.last_event!.seq);
          visits.push({ group: group.id, seq });
          await page.screenshot({
            path: info.outputPath(
              `${String(i + 1).padStart(2, "0")}-${group.id}-${viewport.name}.png`,
            ),
          });
        }
        // The ring must not jump ahead, back and ahead again during a forward
        // click. It follows the viewed card throughout the scroll transition.
        const stageChanges = await page.evaluate(
          () => (window as any).stageChanges,
        );
        expect(stageChanges).toEqual(groups.map((g) => g.id));
        // Hovering a neighbor must not abruptly dim either card.
        const neighbor = page.locator("#timeline-cache .timeline-card");
        await page.mouse.move(5, 5);
        // Finish the scroll-driven fade before measuring hover in isolation.
        await expect
          .poll(() =>
            neighbor.evaluate(
              (el) =>
                el.getAnimations().filter((a) => a.playState === "running")
                  .length,
            ),
          )
          .toBe(0);
        const opacity = await neighbor.evaluate(
          (el) => getComputedStyle(el).opacity,
        );
        const rect = (await neighbor.boundingBox())!;
        await page.mouse.move(
          rect.x + 10,
          Math.max(5, rect.y + rect.height - 10),
        );
        await page.waitForTimeout(300);
        expect(
          await neighbor.evaluate((el) => getComputedStyle(el).opacity),
        ).toBe(opacity);
        // Scroll backwards: the viewed card and map must move together.
        await page.mouse.wheel(0, -30000);
        await expect.poll(() => page.evaluate(() => scrollY)).toBe(0);
        await expect(current).toHaveAttribute("data-group", "run");
        await expect(ring).toHaveAttribute("data-current-stage", "run");
        await expect(
          page.locator("#timeline-run .completion-detent"),
        ).toBeVisible();
        await expect(page.getByTestId("telemetry-gauge")).toHaveAttribute(
          "data-elapsed-ns",
          String(final.elapsed_ns),
        );
        expect(writes).toEqual(["/api/pipeline/runs"]);
        const journal = (
          await (
            await page.request.get(`/api/pipeline/runs/${id}/journal`)
          ).text()
        )
          .trim()
          .split("\n")
          .map((line) => JSON.parse(line));
        for (const visit of visits)
          expect(journal[visit.seq - 1]?.group).toBe(visit.group);
        await info.attach(
          viewport.name === "desktop"
            ? "progression-receipt"
            : "mobile-progression-receipt",
          {
            body: JSON.stringify({
              id,
              visits,
              stage_changes: stageChanges,
              engine_elapsed_ns: final.elapsed_ns,
              all_observed_groups_presented: true,
              stage_ring_tracks_manual_navigation: true,
              idle_hold_ms: 7000,
              no_navigation_writes: true,
              hover_opacity_stable: true,
            }),
            contentType: "application/json",
          },
        );
      } finally {
        await page.request.post(`/api/pipeline/runs/${id}/cancel`);
      }
    });
  });
}
