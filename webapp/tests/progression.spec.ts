import { readFileSync } from "node:fs";
import { pipelineScenes } from "../src/pipelineScenes";
import { test, expect } from "@playwright/test";
import { Run } from "../src/pipeline";
import { createHash } from "node:crypto";

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
        (window as any).cardAnimations = [];
        document.addEventListener("animationstart", (event) => {
          const step = (event.target as Element).closest<HTMLElement>(
            ".timeline-step",
          );
          if (step)
            (window as any).cardAnimations.push({
              group: step.dataset.group,
              selected: step.classList.contains("is-current"),
              animation: event.animationName,
            });
        });
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
      await page.evaluate(() => document.fonts.ready);
      expect(
        await page.evaluate(
          () =>
            document.fonts.check("600 22px Inter") &&
            document.fonts.check('400 12px "JetBrains Mono"'),
        ),
      ).toBe(true);
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
        await expect(current).toHaveAttribute("data-group", "ingest");
        const firstCursor = Number(await ring.getAttribute("data-cursor"));
        const initialScroll = await page.evaluate(() => scrollY);
        await expect
          .poll(async () => (await snapshot()).status, { timeout: 120000 })
          .toBe("completed");
        const final = await snapshot();
        expect(final.cursor).toBeGreaterThan(firstCursor);
        expect(final.verification?.verified_responses).toBe(18);
        expect(final.verification?.first_connection_responses).toBe(15);
        expect(final.verification?.payload_bytes).toBe(271443);
        // Deliberately outwait both rejected 4s/6s timers, without any input.
        await page.waitForTimeout(7000);
        await expect(current).toHaveAttribute("data-group", "ingest");
        expect(await page.evaluate(() => scrollY)).toBe(initialScroll);
        await expect(page.getByRole("status")).toContainText(
          "Recorded results",
        );
        await expect(page.getByTestId("telemetry-gauge")).toHaveAttribute(
          "data-elapsed-ns",
          String(final.elapsed_ns),
        );
        const groups = pipelineScenes(final, Object.values(final.groups));
        expect(groups.map((g) => g.label)).toEqual(
          [
            ...readFileSync(
              new URL(
                "../../docs/design-source/chunder-pipeline.dc.html",
                import.meta.url,
              ),
              "utf8",
            ).matchAll(/<h2[^>]*>([^<]+)<\/h2>/g),
          ]
            .slice(0, 9)
            .map((match) => match[1]),
        );
        await expect(page.locator(".timeline-step")).toHaveCount(9);
        await expect(ring.locator("li")).toHaveCount(9);
        // The entire real run has finished while the reader stayed on card 1.
        // No later card may have spent its animation on background receipts.
        expect(
          await page.evaluate(() => [
            ...new Set((window as any).cardAnimations.map((a: any) => a.group)),
          ]),
        ).toEqual(["ingest"]);
        const effects = Object.fromEntries(
          groups.map((group) => [group.id, "story-arrive"]),
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
              // Focus alone must select and animate, without an Enter/click.
              await button.focus();
            } else await button.click();
          }
          await expect(current).toHaveAttribute("data-group", group.id);
          await expect(step).toHaveAttribute("data-motion", "focused");
          await expect(
            page.locator(
              '.timeline-step:not(.is-current)[data-motion="focused"]',
            ),
          ).toHaveCount(0);
          await expect
            .poll(() =>
              page.evaluate(
                ({ id, effect }) =>
                  (window as any).cardAnimations.some(
                    (a: any) => a.group === id && a.animation === effect,
                  ),
                { id: group.id, effect: effects[group.id] },
              ),
            )
            .toBe(true);
          await expect(ring).toHaveAttribute("data-current-stage", group.id);
          await expect(
            page.getByRole("group", { name: "Pipeline architecture" }),
          ).toHaveAttribute("data-stage", group.id);
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
          // The new design's code panels must contain saved output from this
          // block, never the illustrative IR/assembly in the designer export.
          for (const preview of await step.locator(".artifact-preview").all()) {
            const name = await preview.getAttribute("data-artifact");
            expect(name).toBeTruthy();
            const receipt = final.recent.find(
              (e) => e.type === "artifact" && e.name === name,
            )!;
            expect(receipt).toBeTruthy();
            expect(name).toBe(
              `block-${String(group.primary.last_event!.block_id).padStart(5, "0")}-${group.primary.last_event!.rip.slice(2)}.${name!.split(".").at(-1)}`,
            );
            const response = await page.request.get(
              `/api/pipeline/runs/${id}/artifacts/${name}`,
            );
            expect(response.ok()).toBe(true);
            const data = await response.body();
            expect(createHash("sha256").update(data).digest("hex")).toBe(
              receipt.sha256,
            );
            const expected = name!.endsWith(".clif")
              ? data
                  .subarray(0, 8192)
                  .toString("utf8")
                  .split("\n")
                  .slice(0, 7)
                  .join("\n")
              : Array.from(
                  { length: Math.ceil(Math.min(data.length, 48) / 8) },
                  (_, row) =>
                    [...data.subarray(row * 8, Math.min(row * 8 + 8, 48))]
                      .map((b) => b.toString(16).padStart(2, "0"))
                      .join(" "),
                ).join("\n");
            await expect(preview.locator("pre")).toHaveAttribute(
              "data-loaded",
              "true",
            );
            await expect(preview.locator("pre")).toHaveText(expected);
          }
          const clipped = await step.evaluate((el) => {
            const footer = Math.min(
              el.querySelector(".timeline-provenance")!.getBoundingClientRect()
                .top,
              el
                .querySelector(".stage-illustration > :first-child")!
                .getBoundingClientRect().bottom,
            );
            return [
              ...el.querySelectorAll(
                ".scene-context, .prepare-illustration small, .register-illustration > div",
              ),
            ]
              .filter(
                (item) => item.getBoundingClientRect().bottom > footer + 1,
              )
              .map((item) => item.textContent);
          });
          expect(clipped).toEqual([]);
          // Fixed telemetry must leave the currently viewed card unobscured.
          const cardBox = (await step.locator(".timeline-card").boundingBox())!;
          const gaugeBox = (await page
            .locator(".telemetry-stack")
            .boundingBox())!;
          expect(
            gaugeBox.x >= cardBox.x + cardBox.width ||
              gaugeBox.y + gaugeBox.height <= cardBox.y,
          ).toBe(true);
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
        expect(
          await page.evaluate(() =>
            (window as any).cardAnimations.every((a: any) => a.selected),
          ),
        ).toBe(true);
        // Hovering a neighbor must not abruptly dim either card.
        const neighbor = page.locator("#timeline-syscall .timeline-card");
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
        await page.evaluate(() => {
          (window as any).returnedToStart = false;
          window.addEventListener(
            "scrollend",
            () => {
              (window as any).returnedToStart = true;
            },
            { once: true },
          );
        });
        await page.mouse.wheel(0, -30000);
        await expect.poll(() => page.evaluate(() => scrollY)).toBe(0);
        // Hitting the top does not finish a native wheel gesture. Let the
        // compositor settle before starting a separate keyboard interaction.
        await expect
          .poll(() => page.evaluate(() => (window as any).returnedToStart))
          .toBe(true);
        await expect(current).toHaveAttribute("data-group", "ingest");
        await expect(ring).toHaveAttribute("data-current-stage", "ingest");
        await expect(
          page.locator("#timeline-ingest .completion-detent"),
        ).toBeVisible();
        await expect(page.getByTestId("telemetry-gauge")).toHaveAttribute(
          "data-elapsed-ns",
          String(final.elapsed_ns),
        );
        const factory = page.locator(".factory-backdrop canvas");
        const runningScans = () =>
          factory.evaluate((el: any) => Number(!!el.factoryState?.().running));
        await expect.poll(runningScans).toBe(1);
        const before = await factory.evaluate(
          (el: any) => el.factoryState().clocks.ingest,
        );
        await expect
          .poll(() =>
            factory.evaluate((el: any) => el.factoryState().clocks.ingest),
          )
          .toBeGreaterThan(before);
        // Motion controls continue to work after completion, with readable
        // recorded values and no new execution/navigation writes.
        await page.getByRole("button", { name: "Pause animations" }).click();
        await expect.poll(runningScans).toBe(0);
        await page.getByTestId("group-initialize").focus();
        await expect(current).toHaveAttribute("data-group", "initialize");
        await expect(current).toHaveAttribute("data-motion", "off");
        await page.getByRole("button", { name: "Resume animations" }).click();
        await expect(current).toHaveAttribute("data-motion", "focused");
        await page.emulateMedia({ reducedMotion: "reduce" });
        await expect(current).toHaveAttribute("data-motion", "off");
        await page.getByTestId("group-initialize").focus();
        await expect(
          page.locator("#timeline-initialize .continue-cue"),
        ).toBeEnabled();
        await page.keyboard.press("Tab");
        // The card now has real inspection controls. Tab visits each memory
        // region before reaching the next card; all of them keep this card
        // selected and respect reduced motion.
        await expect(current).toHaveAttribute("data-group", "initialize");
        await expect(
          page.locator("#timeline-initialize .memory-region").first(),
        ).toBeFocused();
        const controls = await page
          .locator("#timeline-initialize .card-story button")
          .count();
        for (let i = 0; i < controls; i++) await page.keyboard.press("Tab");
        await expect(
          page.locator("#timeline-initialize .continue-cue"),
        ).toBeFocused();
        await page.keyboard.press("Enter");
        await expect(current).toHaveAttribute("data-group", "decode");
        await expect(current).toHaveAttribute("data-motion", "off");
        await page.emulateMedia({ reducedMotion: "no-preference" });
        await expect(current).toHaveAttribute("data-motion", "focused");
        await page.getByRole("button", { name: "View evidence" }).click();
        await expect(
          page.locator('.timeline-step[data-motion="focused"]'),
        ).toHaveCount(0);
        // A restored snapshot has no fresh SSE receipt to trigger motion.
        await page.reload();
        await expect(current).toHaveAttribute("data-group", "ingest");
        await expect.poll(runningScans).toBe(1);
        await expect(current).toHaveAttribute("data-motion", "focused");
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
          expect(
            groups.find((g) => g.id === visit.group)!.sources.map((g) => g.id),
          ).toContain(journal[visit.seq - 1]?.group);
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
              scene_groups: Object.fromEntries(
                groups.map((g) => [g.id, g.sources.map((source) => source.id)]),
              ),
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
