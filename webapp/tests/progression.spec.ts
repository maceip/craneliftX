import { test, expect } from "@playwright/test";
import { Run } from "../src/pipeline";

test("a fast real demo presents every observed group before settling on the result", async ({
  page,
}, info) => {
  await page.addInitScript(() => {
    (window as any).stageVisits = [];
    new MutationObserver(() => {
      const current = document.querySelector(".timeline-step.is-current");
      if (!current) return;
      const group = current.getAttribute("data-group");
      const seq = Number(current.getAttribute("data-motion-seq"));
      const visits = (window as any).stageVisits;
      if (seq && visits.at(-1)?.group !== group)
        visits.push({
          group,
          seq,
          at: performance.now(),
          ring: document.querySelector<HTMLElement>(
            '[data-testid="stage-thermometer"]',
          )?.dataset.currentStage,
        });
    }).observe(document, { subtree: true, attributes: true, childList: true });
  });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto("/");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText(
    "Translate and execute x86-64 binaries on RISC-V.",
  );
  await expect(
    page.getByRole("button", { name: "Upload binary" }),
  ).toBeEnabled();
  await expect(page.locator(".launch-description")).toHaveText(
    "x86-64 → iced-x86 → Cranelift IR → RV64",
  );
  await page.screenshot({ path: info.outputPath("launch-copy-desktop.png") });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({ path: info.outputPath("launch-copy-mobile.png") });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.getByRole("button", { name: "Run demo", exact: true }).click();
  await expect(page).toHaveURL(/run=/);
  const id = new URL(page.url()).searchParams.get("run");
  try {
    await expect(page.locator(".timeline-step.is-current")).toHaveAttribute(
      "data-group",
      "run",
    );
    await expect(page.getByTestId("pipeline-timeline")).toHaveAttribute(
      "data-reviewing",
      "true",
    );
    await expect(page.getByRole("status")).toContainText(
      "Engine complete · Reviewing recorded stages",
      { timeout: 30000 },
    );
    // A completed engine must not cause the presentation to jump to Result.
    await expect(page.locator(".timeline-step.is-current")).not.toHaveAttribute(
      "data-group",
      "result",
    );
    const final: Run = await (
      await page.request.get(`/api/pipeline/runs/${id}`)
    ).json();
    expect(final.status).toBe("completed");
    expect(final.verification?.first_connection_responses).toBe(12);
    await expect(page.getByTestId("telemetry-gauge")).toHaveAttribute(
      "data-elapsed-ns",
      String(final.elapsed_ns),
    );
    // Pausing presentation must not change the recorded execution time.
    await page.getByRole("button", { name: "Pause animations" }).click();
    const paused = await page
      .locator(".timeline-step.is-current")
      .getAttribute("data-group");
    await page.waitForTimeout(4300);
    await expect(page.locator(".timeline-step.is-current")).toHaveAttribute(
      "data-group",
      paused!,
    );
    await expect(page.getByTestId("stage-thermometer")).toHaveAttribute(
      "data-current-stage",
      paused!,
    );
    await expect(page.getByTestId("telemetry-gauge")).toHaveAttribute(
      "data-elapsed-ns",
      String(final.elapsed_ns),
    );
    await page.getByRole("button", { name: "Resume animations" }).click();
    await page.screenshot({
      path: info.outputPath("recorded-stages-desktop.png"),
    });
    await expect(page.getByTestId("pipeline-timeline")).toHaveAttribute(
      "data-reviewing",
      "false",
      { timeout: 75000 },
    );
    await expect(page.locator(".timeline-step.is-current")).toHaveAttribute(
      "data-group",
      "result",
    );
    const visits = await page.evaluate(() => (window as any).stageVisits);
    const groups = Object.values(final.groups)
      .sort((a, b) => a.order - b.order)
      .filter((g) => g.count > 0);
    expect(visits.map((v: any) => v.group)).toEqual(groups.map((g) => g.id));
    for (let i = 1; i < visits.length; i++) {
      const dense = ["decode", "audit"].includes(visits[i - 1].group);
      expect(visits[i].at - visits[i - 1].at).toBeGreaterThan(
        dense ? 5800 : 3500,
      );
    }
    for (const visit of visits) expect(visit.ring).toBe(visit.group);
    await expect(page.getByTestId("stage-thermometer")).toHaveAttribute(
      "data-current-stage",
      "result",
    );
    await expect(
      page.locator('.stage-thermometer [aria-current="step"]'),
    ).toHaveCount(1);
    const titles = [
      "Initialize Pipeline",
      "Read ELF Image",
      "Validate & Map Segments",
      "Initialize Guest Context",
      "Disassemble Block",
      "Verify Safety Policy",
      "Lift to Cranelift IR",
      "Compile to RV64",
      "Commit Block Artifacts",
      "Execute RV64 Code",
      "Handle System Call",
      "Code Cache Status",
      "Execution Summary",
    ];
    await expect(page.locator(".timeline-stage-button strong")).toHaveText(
      titles,
    );
    const journal = (
      await (await page.request.get(`/api/pipeline/runs/${id}/journal`)).text()
    )
      .trim()
      .split("\n")
      .map((line) => JSON.parse(line));
    for (const visit of visits)
      expect(journal[visit.seq - 1]?.group).toBe(visit.group);
    await expect(
      page.getByRole("button", { name: "Follow live events" }),
    ).toHaveCount(0);
    await expect(
      page.getByRole("button", { name: "Pause animations" }),
    ).toHaveCount(0);
    // Pointer movement must not make a neighboring card jump in brightness.
    const card = page.locator("#timeline-cache .timeline-card");
    await page.mouse.move(10, 10);
    const opacity = await card.evaluate((el) => getComputedStyle(el).opacity);
    const rect = await card.boundingBox();
    await page.mouse.move(
      rect!.x + 20,
      Math.max(5, rect!.y + rect!.height - 20),
    );
    await page.waitForTimeout(300);
    expect(await card.evaluate((el) => getComputedStyle(el).opacity)).toBe(
      opacity,
    );
    await info.attach("progression-receipt", {
      body: JSON.stringify({
        id,
        visits,
        engine_elapsed_ns: final.elapsed_ns,
        all_observed_groups_presented: true,
        stage_ring_tracks_readable_progression: true,
        minimum_stage_reading_ms: 4000,
        dense_stage_reading_ms: 6000,
        hover_opacity_stable: true,
      }),
      contentType: "application/json",
    });
  } finally {
    await page.request.post(`/api/pipeline/runs/${id}/cancel`);
  }
});
