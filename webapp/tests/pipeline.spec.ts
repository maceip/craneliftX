import {
  pipelineScenes,
  sceneStatus,
  sceneForGroup,
} from "../src/pipelineScenes";
import { elf } from "./elf";
import { test, expect, Page } from "@playwright/test";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  applyLedger,
  blankGroup,
  groupState,
  Run,
  RunSummary,
} from "../src/pipeline";

const runId = (page: Page) => new URL(page.url()).searchParams.get("run");
const snapshot = async (page: Page): Promise<Run> =>
  (await page.request.get(`/api/pipeline/runs/${runId(page)}`)).json();
const runSummary = async (page: Page): Promise<RunSummary> => {
  // Use the browser's existing HTTPS connection for time-sensitive observations.
  // A fresh Node API connection can finish connecting after a short guest exits.
  const history: RunSummary[] = await page.evaluate(async () =>
    (await fetch("/api/pipeline/runs")).json(),
  );
  const current = history.find((entry) => entry.id === runId(page));
  if (!current) throw new Error("Current run is missing from history");
  return current;
};
const sequence = async (page: Page) =>
  Number(await page.getByTestId("event-sequence").textContent());
async function upload(page: Page, name: string, buffer: Buffer) {
  await page.goto("/");
  await expect(
    page.getByRole("button", { name: "Inspect your binary" }),
  ).toBeEnabled();
  await page
    .getByLabel("Choose an x86-64 ELF binary")
    .setInputFiles({ name, mimeType: "application/octet-stream", buffer });
  await expect(page).toHaveURL(/run=/, { timeout: 120000 });
}
async function openEvidence(page: Page, group?: string) {
  if (
    !(await page
      .getByRole("dialog", { name: "Execution evidence" })
      .isVisible())
  )
    await page.getByRole("button", { name: "View evidence" }).click();
  if (group)
    await page.getByLabel("Inspect operation group").selectOption(group);
}
test.afterEach(async ({ page, context }) => {
  await context.setOffline(false);
  if (runId(page)) {
    await expect
      .poll(
        async () => {
          try {
            const cancelled = await page.request.post(
              `/api/pipeline/runs/${runId(page)}/cancel`,
            );
            return cancelled.ok();
          } catch {
            return false;
          }
        },
        { timeout: 15000 },
      )
      .toBe(true);
    await expect
      .poll(
        async () => {
          try {
            return ["completed", "failed", "cancelled", "interrupted"].includes(
              (await runSummary(page)).status,
            );
          } catch {
            return false;
          }
        },
        { timeout: 15000 },
      )
      .toBe(true);
  }
});

test("13 groups consume real Rust server events live, recover a connection, and reconcile exact evidence", async ({
  page,
  context,
}, info) => {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto("/");
  await expect(page.locator(".launch-action")).toHaveCount(2);
  await expect(page.locator(".timeline-stage-button")).toHaveCount(0);
  await expect(page.locator("nav, .timeline-inspector, .masthead")).toHaveCount(
    0,
  );
  await page.screenshot({
    path: info.outputPath("idle-desktop.png"),
    fullPage: true,
  });
  await page.getByRole("button", { name: "Run interactive pipeline" }).click();
  await expect(page).toHaveURL(/run=/);
  await expect(page.locator(".timeline-stage-button")).toHaveCount(9);
  await expect(
    page.getByRole("status").filter({ hasText: "Live stream" }),
  ).toBeVisible();
  await expect.poll(() => sequence(page)).toBeGreaterThan(100);
  const before = await runSummary(page),
    first = await sequence(page);
  expect(before.status).toBe("running");
  await expect.poll(() => sequence(page)).toBeGreaterThan(first + 60);
  expect((await runSummary(page)).status).toBe("running");
  await page.screenshot({
    path: info.outputPath("live-desktop.png"),
    fullPage: true,
  });
  await context.setOffline(true);
  await page.waitForTimeout(900);
  await context.setOffline(false);
  await expect.poll(() => sequence(page)).toBeGreaterThan(first + 100);
  await page.reload();
  await expect(page.getByTestId("group-result")).toBeVisible();
  await expect
    .poll(async () => (await runSummary(page)).status, { timeout: 90000 })
    .toBe("completed");
  const final = await snapshot(page);
  await expect(page.getByTestId("event-sequence")).toHaveText(
    String(final.cursor),
  );
  expect(final.verification?.verified_responses).toBe(18);
  expect(final.verification?.first_connection_responses).toBe(15);
  expect(final.verification?.payload_bytes).toBe(271443);
  expect(final.verification?.reconnects_on_first_connection).toBe(0);
  expect(final.counters.cache_invalidations).toBeGreaterThan(0);
  await page.getByTestId("group-result").click();
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("telemetry-gauge")).toHaveAttribute(
    "data-elapsed-ns",
    String(final.elapsed_ns),
  );
  const metrics = {
    elapsed_ns: final.elapsed_ns,
    rv64_steps: final.counters.rv64_steps,
    compiled_blocks: final.counters.compiled_blocks,
    unique_lifted_instructions: final.counters.unique_lifted_instructions,
    unique_lifted_bytes: final.counters.unique_lifted_bytes,
    cache_invalidations: final.counters.cache_invalidations,
  };
  for (const [key, value] of Object.entries(metrics)) {
    await expect(page.locator(`[data-metric="${key}"]`)).toHaveAttribute(
      "data-value",
      String(value),
    );
    await expect(
      page.locator(`[data-metric="${key}"]`).getByRole("img"),
    ).toHaveAttribute(
      "aria-label",
      key === "elapsed_ns"
        ? (value! / 1e9).toFixed(3)
        : value!.toLocaleString("en-US"),
    );
  }
  await expect(
    page.getByTestId("telemetry-gauge").getByRole("img"),
  ).toHaveAttribute("aria-label", (final.elapsed_ns / 1e9).toFixed(1));
  await page.screenshot({ path: info.outputPath("focus-result-desktop.png") });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({ path: info.outputPath("focus-result-mobile.png") });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await openEvidence(page);
  await expect(
    page.getByRole("heading", { name: "Persistent connection verified" }),
  ).toBeVisible();
  const journal = (
    await (
      await page.request.get(`/api/pipeline/runs/${final.id}/journal`)
    ).text()
  )
    .trim()
    .split("\n")
    .map((s) => JSON.parse(s));
  expect(journal.map((e) => e.seq)).toEqual(
    Array.from({ length: final.cursor }, (_, i) => i + 1),
  );
  let projected = {
    ...final,
    cursor: 0,
    engine_run_id: null,
    recent: [],
    history: [],
    artifact_count: 0,
    groups: Object.fromEntries(
      Object.values(final.groups).map((g) => [g.id, blankGroup(g)]),
    ),
  } as Run;
  for (const event of journal) projected = applyLedger(projected, event);
  expect(projected.groups).toEqual(final.groups);
  const scenes = pipelineScenes(final, Object.values(final.groups));
  expect(scenes).toHaveLength(9);
  expect(
    scenes.flatMap((scene) => scene.sources.map((g) => g.id)).sort(),
  ).toEqual(Object.keys(final.groups).sort());
  expect(scenes.reduce((count, scene) => count + scene.count, 0)).toBe(
    final.cursor,
  );
  for (const scene of scenes) {
    const button = page.getByTestId(`group-${scene.id}`);
    await expect(button).toHaveAttribute("data-state", scene.state);
    await expect(button.locator("strong")).toHaveText(scene.label);
    await expect(button.locator("small")).toHaveText(
      `${scene.count.toLocaleString("en-US")} ${scene.count === 1 ? "event" : "events"} · ${scene.completed.toLocaleString("en-US")} completed ${scene.completed === 1 ? "span" : "spans"}`,
    );
  }
  const partial = {
    ...final,
    groups: {
      ...final.groups,
      initialize: blankGroup(final.groups.initialize),
    },
  };
  const partialPrepare = pipelineScenes(
    partial,
    Object.values(final.groups),
  ).find((scene) => scene.id === "initialize")!;
  expect(partialPrepare.state).toBe("Observed");
  expect(sceneStatus(partialPrepare, partial.status)).toBe(
    "Partially recorded",
  );
  const primaryCodegen = scenes.find((scene) => scene.id === "codegen")!;
  expect(primaryCodegen.primary).toBe(final.groups.codegen);
  expect(primaryCodegen.last_event).toEqual(final.groups.artifacts.last_event);
  expect(final.groups.audit.completed).toBe(final.counters.compiled_blocks);
  expect(final.groups.audit.output?.verdict).toBe("accepted");
  await openEvidence(page, "audit");
  await expect(page.getByTestId("audit-output")).toBeVisible();
  await expect(
    page.getByRole("list", { name: "Instruction policy checks" }),
  ).toBeVisible();
  await page.screenshot({
    path: info.outputPath("audit-desktop.png"),
    fullPage: true,
  });
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(page.getByTestId("audit-output")).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  await page.screenshot({
    path: info.outputPath("audit-mobile.png"),
    fullPage: true,
  });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await openEvidence(page, "decode");
  await page.getByRole("button", { name: "View raw JSON" }).click();
  expect(JSON.parse(await page.getByTestId("raw-event").innerText())).toEqual(
    final.groups.decode.output,
  );
  for (const source of Object.values(final.groups)) {
    await openEvidence(page, source.id);
    await expect(
      page.getByLabel("Inspect operation group").locator("option"),
    ).toHaveCount(13);
    await expect(page.getByLabel("Inspect operation group")).toHaveValue(
      source.id,
    );
    await page.getByRole("button", { name: "Back to execution" }).click();
    await expect(page.locator(".timeline-step.is-current")).toHaveAttribute(
      "data-group",
      sceneForGroup(source.id),
    );
  }
  await openEvidence(page, "artifacts");
  await page.getByRole("button", { name: "View output" }).click();
  const artifact = final.groups.artifacts.output!;
  const download = await page.request.get(
    `/api/pipeline/runs/${final.id}/artifacts/${artifact.name}`,
  );
  const data = await download.body();
  expect(data.length).toBe(artifact.bytes);
  expect(createHash("sha256").update(data).digest("hex")).toBe(artifact.sha256);
  await page.getByLabel("Filter ledger by group").selectOption("syscall");
  const tags = await page
    .locator(".ledger-scroll tbody .group-tag")
    .allTextContents();
  expect(tags.length).toBeGreaterThan(0);
  expect(tags.every((tag) => tag === "syscall")).toBe(true);
  await page.getByLabel("Filter ledger by group").selectOption("all");
  await page.locator(".event-link").first().click();
  await expect(page.getByTestId("raw-event")).toContainText('"seq":');
  const replay = await page.request.get(
    `/api/pipeline/runs/${final.id}/events?after=0`,
    { headers: { "Last-Event-ID": "10" } },
  );
  const received = (await replay.text())
    .split(/\r?\n/)
    .filter((l) => l.startsWith("data: "))
    .map((l) => JSON.parse(l.slice(6)))
    .filter((e) => e.seq);
  expect(received).toEqual(journal.slice(10));
  await openEvidence(page, "decode");
  await page.getByRole("button", { name: "View output" }).click();
  await page.screenshot({
    path: info.outputPath("completed-desktop.png"),
    fullPage: true,
  });
  await page.setViewportSize({ width: 390, height: 844 });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await page.screenshot({
    path: info.outputPath("completed-mobile.png"),
    fullPage: true,
  });
  expect(errors).toEqual([]);
  await info.attach("pipeline-receipt", {
    body: JSON.stringify({
      id: final.id,
      first_live_cursor: first,
      terminal_cursor: final.cursor,
      live_status: before.status,
      verification: final.verification,
      counters: final.counters,
      groups: final.groups,
      exact_replay: true,
      disconnected_browser_recovered: true,
      artifact_hash_verified: true,
    }),
    contentType: "application/json",
  });
});

test("unsupported uploaded instructions fail at lowering without advancing later groups", async ({
  page,
}, info) => {
  await upload(page, "unsupported.elf", elf(Buffer.from([0x0f, 0x0b])));
  await expect.poll(async () => (await runSummary(page)).status).toBe("failed");
  await expect(page.getByTestId("group-lower")).toHaveAttribute(
    "data-state",
    "Failed",
  );
  await expect(page.getByTestId("group-codegen")).toHaveAttribute(
    "data-state",
    "Not exercised",
  );
  await expect(page.getByTestId("group-audit")).toHaveAttribute(
    "data-state",
    "Completed",
  );
  await page.getByTestId("group-lower").click();
  await expect(page.getByRole("alert")).toContainText("unsupported");
  await page.screenshot({
    path: info.outputPath("failed-desktop.png"),
    fullPage: true,
  });
});

test("cancellation interrupts open spans and never manufactures an engine result", async ({
  page,
}) => {
  await upload(page, "loop.elf", elf(Buffer.from([0xeb, 0xfe])));
  await expect
    .poll(async () => (await snapshot(page)).counters.rv64_steps ?? 0)
    .toBeGreaterThan(0);
  await page.getByRole("button", { name: "Stop execution" }).click();
  await expect
    .poll(async () => (await runSummary(page)).status)
    .toBe("cancelled");
  await expect(page.getByTestId("group-execute")).toHaveAttribute(
    "data-state",
    "Interrupted",
  );
  await expect(page.getByTestId("group-result")).toHaveAttribute(
    "data-state",
    "Not exercised",
  );
  expect((await snapshot(page)).engine_result).toBeNull();
  await page.reload();
  await expect(page.getByTestId("group-execute")).toHaveAttribute(
    "data-state",
    "Interrupted",
  );
});

test("16 MiB input keeps the browser live through hundreds of lazy compilations and an nginx restart", async ({
  page,
}, info) => {
  const blocks = 256;
  const add = Buffer.from([0x48, 0x83, 0xc0, 1]),
    jump = Buffer.from([0xe9, 0, 0, 0, 0]);
  const block = Buffer.concat([...Array.from({ length: 31 }, () => add), jump]);
  const code = Buffer.concat([
    Buffer.from([0x31, 0xff, ...jump]),
    ...Array.from({ length: blocks }, () => block),
    Buffer.from([0xb8, 60, 0, 0, 0, 0x0f, 0x05]),
  ]);
  await upload(page, "large-lazy.elf", elf(code, 16 * 1024 * 1024));
  await expect
    .poll(async () =>
      Number(
        (await page.getByTestId("compiled-blocks").textContent())?.replaceAll(
          ",",
          "",
        ) ?? 0,
      ),
    )
    .toBeGreaterThan(10);
  const first = await sequence(page);
  expect((await runSummary(page)).status).toBe("running");
  await page.screenshot({
    path: info.outputPath("large-live.png"),
    fullPage: true,
  });
  if (!process.env.ISR_BASE_URL) {
    const metadata = JSON.parse(
      readFileSync(join(tmpdir(), "isr-e2e-stack-8878.json"), "utf8"),
    );
    writeFileSync(metadata.control, "proxy");
    await expect(
      page.getByRole("status").filter({ hasText: "Reconnecting" }),
    ).toBeVisible();
    const interruptedCursor = await sequence(page);
    await expect
      .poll(() => sequence(page), { timeout: 30000 })
      .toBeGreaterThan(interruptedCursor);
  }
  await expect
    .poll(() => sequence(page), { timeout: 30000 })
    .toBeGreaterThan(first + 120);
  await expect
    .poll(
      async () => {
        try {
          return (await runSummary(page)).status;
        } catch {
          return "proxy unavailable";
        }
      },
      { timeout: 120000 },
    )
    .toBe("completed");
  const final = await snapshot(page);
  await expect(page.getByTestId("event-sequence")).toHaveText(
    String(final.cursor),
  );
  expect(final.counters.compiled_blocks).toBe(blocks + 2);
  expect(final.counters.unique_lifted_bytes).toBe(code.length);
  expect(final.input?.bytes).toBe(16 * 1024 * 1024);
  expect(
    await page.locator(".ledger-scroll tbody tr").count(),
  ).toBeLessThanOrEqual(60);
  expect(final.recent.length).toBeLessThanOrEqual(120);
  await info.attach("large-live-receipt", {
    body: JSON.stringify({
      first_live_cursor: first,
      final_cursor: final.cursor,
      input_bytes: final.input?.bytes,
      actual_source_bytes: code.length,
      counters: final.counters,
      nginx_restart: !process.env.ISR_BASE_URL,
    }),
    contentType: "application/json",
  });
});

test("unavailable execution and invalid requests are visible and rejected", async ({
  page,
  request,
}) => {
  await page.goto("/");
  expect(
    (
      await request.post("/api/pipeline/runs", { data: { mode: "fabricated" } })
    ).status(),
  ).toBe(400);
  expect(
    (
      await request.post("/api/pipeline/runs", {
        headers: { Origin: "https://foreign.example" },
        data: { mode: "rust-server" },
      })
    ).status(),
  ).toBe(403);
  expect(
    (await request.get("/api/pipeline/runs/missing/events")).status(),
  ).toBe(404);
  await upload(page, "wrong-format.elf", Buffer.from("This is not an ELF"));
  await expect.poll(async () => (await runSummary(page)).status).toBe("failed");
  await expect(page.getByTestId("group-initialize")).toHaveAttribute(
    "data-state",
    "Failed",
  );
  await expect(page.getByTestId("group-decode")).toHaveAttribute(
    "data-state",
    "Not exercised",
  );
});

test("shellcode policy findings show actual rejection before lowering and survive reload", async ({
  page,
}, info) => {
  await upload(page, "interrupt.elf", elf(Buffer.from([0xcd, 0x80])));
  await expect.poll(async () => (await runSummary(page)).status).toBe("failed");
  await expect(page.getByTestId("group-audit")).toHaveAttribute(
    "data-state",
    "Failed",
  );
  await expect(page.getByTestId("group-lower")).toHaveAttribute(
    "data-state",
    "Not exercised",
  );
  await openEvidence(page, "audit");
  await expect(page.getByTestId("audit-output")).toContainText(
    "Instruction policy rejected",
  );
  await expect(
    page.locator('.audit-check[data-verdict="rejected"]'),
  ).toContainText("int 80h");
  await expect(
    page.locator('.audit-check[data-verdict="rejected"]'),
  ).toContainText("0x20100");
  await page.screenshot({
    path: info.outputPath("audit-rejected-desktop.png"),
    fullPage: true,
  });
  const final = await snapshot(page);
  expect(final.counters.rv64_steps ?? 0).toBe(0);
  await page.reload();
  await openEvidence(page, "audit");
  await expect(page.getByTestId("audit-output")).toContainText(
    "Instruction policy rejected",
  );
  await page.getByRole("button", { name: "View raw JSON" }).click();
  expect(JSON.parse(await page.getByTestId("raw-event").innerText())).toEqual(
    final.groups.audit.output,
  );
});

test("runtime stack rejection is shown after accepted inspection and actual RV64 execution", async ({
  page,
}, info) => {
  await upload(
    page,
    "transient-stack.elf",
    elf(
      Buffer.from([
        0x48, 0x81, 0xec, 0, 0, 32, 0, 0x48, 0x81, 0xc4, 0, 0, 32, 0, 0x31,
        0xff, 0xb8, 60, 0, 0, 0, 0x0f, 0x05,
      ]),
    ),
  );
  await expect.poll(async () => (await runSummary(page)).status).toBe("failed");
  await expect(page.getByTestId("group-audit")).toHaveAttribute(
    "data-state",
    "Failed",
  );
  await openEvidence(page, "audit");
  await expect(page.getByTestId("audit-output")).toContainText(
    "Live stack bounds",
  );
  await expect(page.getByTestId("audit-output")).toContainText("0x20100");
  const final = await snapshot(page);
  expect(final.counters.rv64_steps).toBeGreaterThan(0);
  expect(final.counters.syscalls).toBe(0);
  expect(final.groups.audit.output?.phase).toBe("execute");
  await page.setViewportSize({ width: 390, height: 844 });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  await page.screenshot({
    path: info.outputPath("audit-runtime-rejected-mobile.png"),
    fullPage: true,
  });
});

test("timeline motion follows the reader, preserves neighbors and pauses independently of live values", async ({
  page,
  context,
}, info) => {
  await page.addInitScript(() => {
    (window as any).motionReceipts = [];
    (window as any).operationReceipts = [];
    new MutationObserver(() => {
      const map = document.querySelector<HTMLElement>(
        '[data-testid="stage-thermometer"]',
      );
      if (!map?.dataset.currentStage) return;
      const receipts = (window as any).operationReceipts;
      const seq = Number(map.dataset.cursor);
      if (receipts.length < 500 && receipts.at(-1)?.seq !== seq)
        receipts.push({
          seq,
          group: map.dataset.currentStage,
          selected: document.querySelector<HTMLElement>(
            ".timeline-step.is-current",
          )?.dataset.group,
          observed: Array.from(
            map.querySelectorAll<HTMLElement>('[data-observed="true"]'),
          ).map((dot) => dot.dataset.group),
        });
    }).observe(document, { subtree: true, attributes: true, childList: true });
    document.addEventListener("animationstart", (event) => {
      const step = (event.target as Element).closest<HTMLElement>(
        ".timeline-step",
      );
      if (step && (window as any).motionReceipts.length < 500)
        (window as any).motionReceipts.push({
          group: step.dataset.group,
          seq: Number(step.dataset.motionSeq),
          kind: step.dataset.motion,
          animation: event.animationName,
        });
    });
  });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto("/");
  const timeline = page.getByTestId("pipeline-timeline");
  await expect(page.locator("[data-stage-visual]")).toHaveCount(0);
  await expect(
    page.locator('.timeline-step:not([data-motion="off"])'),
  ).toHaveCount(0);
  await expect(page.locator("[data-ledger-dot]")).toHaveCount(0);
  // A real x86 ELF loops through close(-1). The explicit host bridge returns
  // EBADF; durable syscall receipts keep it observable until we cancel it.
  const program = elf(
    Buffer.from([
      0xb8, 3, 0, 0, 0, 0xbf, 255, 255, 255, 255, 0x0f, 0x05, 0xeb, 0xf2,
    ]),
  );
  await upload(page, "timeline-syscalls.elf", program);
  await expect.poll(() => sequence(page)).toBeGreaterThan(100);
  expect((await runSummary(page)).status).toBe("running");
  await expect(page.locator(".stage-thermometer li")).toHaveCount(9);
  await expect(
    page.locator('.stage-thermometer [data-current="true"]'),
  ).toHaveCount(1);
  await page.getByTestId("group-execute").click();
  await page.keyboard.press("Escape");
  await page
    .locator("#timeline-execute")
    .evaluate((el) => el.scrollIntoView({ block: "center" }));
  await expect(timeline).toHaveAttribute("data-navigation", "manual");
  await expect(page.locator("#timeline-execute")).toHaveAttribute(
    "data-motion",
    "focused",
  );
  const runningAnimations = () =>
    timeline.evaluate(
      (el) =>
        el
          .getAnimations({ subtree: true })
          .filter((a) => a.playState === "running").length,
    );
  await expect.poll(runningAnimations).toBeGreaterThan(0);
  await page.getByRole("button", { name: "Pause animations" }).click();
  await expect.poll(runningAnimations).toBe(0);
  let cursor = await sequence(page);
  await expect.poll(() => sequence(page)).toBeGreaterThan(cursor + 20);
  await expect(page.getByTestId("group-execute")).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await page.getByRole("button", { name: "Resume animations" }).click();
  await expect.poll(runningAnimations).toBeGreaterThan(0);
  await page.emulateMedia({ reducedMotion: "reduce" });
  await expect.poll(runningAnimations).toBe(0);
  cursor = await sequence(page);
  await expect.poll(() => sequence(page)).toBeGreaterThan(cursor + 20);
  await page.emulateMedia({ reducedMotion: "no-preference" });
  await expect.poll(runningAnimations).toBeGreaterThan(0);
  await context.setOffline(true);
  await expect(timeline).toHaveAttribute("data-motion-enabled", "true");
  await expect.poll(runningAnimations).toBeGreaterThan(0);
  await context.setOffline(false);
  await expect(timeline).toHaveAttribute("data-motion-enabled", "true");
  await page
    .locator("#timeline-execute")
    .evaluate((el) => el.scrollIntoView({ block: "center" }));
  const neighborsVisible = async () => {
    await expect
      .poll(() =>
        timeline.evaluate((el) => {
          const footer = el
            .querySelector(".focus-transport")!
            .getBoundingClientRect();
          return ["previous", "current", "next"].every((context) => {
            const card = el.querySelector(
              `[data-context="${context}"] .timeline-card`,
            );
            if (!card) return false;
            const rect = card.getBoundingClientRect();
            return (
              Math.min(rect.bottom, footer.top) - Math.max(rect.top, 0) > 35
            );
          });
        }),
      )
      .toBe(true);
  };
  const consistentGeometry = async () => {
    for (const selector of [
      ".timeline-stage-button",
      ".stage-illustration",
      ".timeline-provenance",
    ]) {
      const dimensions = await page.locator(selector).evaluateAll((elements) =>
        elements.map((el) => {
          const rect = el.getBoundingClientRect();
          return [rect.x, rect.width, rect.height].map((n) => Math.round(n));
        }),
      );
      expect(
        new Set(dimensions.map((d) => JSON.stringify(d))).size,
        selector,
      ).toBe(1);
    }
    const buttons = await page
      .locator(".timeline-controls button")
      .evaluateAll((elements) =>
        elements.map((el) => {
          const css = getComputedStyle(el);
          return [
            el.getBoundingClientRect().height,
            css.fontSize,
            css.borderRadius,
          ];
        }),
      );
    expect(new Set(buttons.map((d) => JSON.stringify(d))).size).toBe(1);
  };
  await neighborsVisible();
  await consistentGeometry();
  await page.screenshot({ path: info.outputPath("timeline-live-desktop.png") });
  await page.setViewportSize({ width: 390, height: 844 });
  await page
    .locator("#timeline-execute")
    .evaluate((el) => el.scrollIntoView({ block: "center" }));
  await neighborsVisible();
  await consistentGeometry();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await page.screenshot({ path: info.outputPath("timeline-live-mobile.png") });
  await page.mouse.move(200, 400);
  await page.mouse.wheel(0, -150);
  await expect(timeline).toHaveAttribute("data-navigation", "manual");
  const selected = await page
    .locator(".timeline-step.is-current")
    .getAttribute("data-group");
  cursor = await sequence(page);
  await expect.poll(() => sequence(page)).toBeGreaterThan(cursor + 20);
  await expect(page.locator(".timeline-step.is-current")).toHaveAttribute(
    "data-group",
    selected!,
  );
  // Scrolling updates the inspection map without changing engine execution.
  await page.mouse.wheel(0, -20000);
  await expect.poll(() => page.evaluate(() => window.scrollY)).toBe(0);
  const firstCard = page.locator("#timeline-ingest .timeline-card");
  await expect
    .poll(() =>
      firstCard.evaluate((el) => Number(getComputedStyle(el).opacity)),
    )
    .toBe(1);
  await expect(
    page.locator("#timeline-ingest .completion-detent"),
  ).toBeVisible();
  await expect(
    page.locator('.stage-thermometer [data-current="true"]'),
  ).toHaveCount(1);
  await expect(page.getByTestId("stage-thermometer")).toHaveAttribute(
    "data-current-stage",
    "ingest",
  );
  await page.screenshot({ path: info.outputPath("scroll-history-mobile.png") });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page
    .locator("#timeline-ingest")
    .evaluate((el) => el.scrollIntoView({ block: "center" }));
  await expect
    .poll(() =>
      page
        .locator("#timeline-ingest .timeline-card")
        .evaluate((el) => Number(getComputedStyle(el).opacity)),
    )
    .toBe(1);
  await expect(
    page.locator("#timeline-ingest .completion-detent"),
  ).toHaveAttribute("aria-label", "Completed in this run");
  await expect(
    page.locator("#timeline-ingest .stage-illustration > div"),
  ).toHaveCSS("opacity", "1");
  await page.screenshot({
    path: info.outputPath("scroll-history-desktop.png"),
  });
  await page.setViewportSize({ width: 375, height: 667 });
  await page
    .locator("#timeline-ingest")
    .evaluate((el) => el.scrollIntoView({ block: "center" }));
  const gauge = await page.getByTestId("telemetry-gauge").boundingBox();
  const heading = await page
    .locator("#timeline-ingest .timeline-stage-button")
    .boundingBox();
  expect(gauge!.y + gauge!.height).toBeLessThan(heading!.y + 40);
  await page.screenshot({
    path: info.outputPath("scroll-history-small-mobile.png"),
  });
  await page.getByRole("button", { name: "Stop execution" }).click();
  await expect
    .poll(async () => (await runSummary(page)).status)
    .toBe("cancelled");
  const final = await snapshot(page);
  // A steady ring locates the inspected card even when execution has ended.
  await expect(
    page.locator('.stage-thermometer [data-current="true"]'),
  ).toHaveCount(1);
  await expect(page.getByTestId("event-sequence")).toHaveText(
    String(final.cursor),
  );
  await expect.poll(runningAnimations).toBeGreaterThan(0);
  await expect(page.getByTestId("ingest-hash")).toHaveText(
    createHash("sha256").update(program).digest("hex"),
  );
  const decode = final.groups.decode.output!;
  expect(
    await page.locator(".opcode-track li").evaluateAll((items) =>
      items.map((i) => ({
        ip: (i as HTMLElement).dataset.ip,
        bytes: (i as HTMLElement).dataset.bytes,
        asm: i.querySelector("span")!.textContent,
      })),
    ),
  ).toEqual(decode.instructions);
  await expect(
    page.locator('[data-stage-visual="decode"] .raw-byte-chips code'),
  ).toHaveCount(decode.instructions.length);
  expect(
    await page.locator(".segment-row").evaluateAll((items) =>
      items.map((i) => ({
        address: (i as HTMLElement).dataset.address,
        memory_bytes: Number((i as HTMLElement).dataset.bytes),
      })),
    ),
  ).toEqual(
    final.groups.load
      .output!.details.segments.slice(0, 4)
      .map((s: any) => ({ address: s.address, memory_bytes: s.memory_bytes })),
  );
  for (const group of ["lower", "codegen"]) {
    const output = final.groups[group].output!;
    await expect(
      page.locator(`[data-stage-visual="${group}"] [data-block-id]`),
    ).toHaveAttribute("data-block-id", String(output.block_id));
    await expect(
      page.locator(
        `[data-stage-visual="${group}"] .translation-visual[data-source-seq]`,
      ),
    ).toHaveAttribute("data-source-seq", String(output.seq));
  }
  const journal = (
    await (
      await page.request.get(`/api/pipeline/runs/${final.id}/journal`)
    ).text()
  )
    .trim()
    .split("\n")
    .map((s) => JSON.parse(s));
  const motions = await page.evaluate(() => (window as any).motionReceipts);
  expect(motions.length).toBeGreaterThan(0);
  expect(motions.every((m: any) => m.kind === "focused")).toBe(true);
  for (const motion of motions)
    expect(sceneForGroup(journal[motion.seq - 1]?.group)).toBe(motion.group);
  const operations = await page.evaluate(
    () => (window as any).operationReceipts,
  );
  expect(operations.length).toBeGreaterThan(0);
  let projected = {
    ...final,
    cursor: 0,
    status: "running",
    recent: [],
    history: [],
    engine_result: null,
    engine_run_id: null,
    groups: Object.fromEntries(
      Object.values(final.groups).map((g) => [g.id, blankGroup(g)]),
    ),
  } as Run;
  for (let i = 0; i < operations.length; i++) {
    expect(operations[i].group).toBe(operations[i].selected);
    if (i > 0)
      expect(operations[i].observed).toEqual(
        expect.arrayContaining(operations[i - 1].observed),
      );
  }
  const bySeq = new Map(operations.map((r: any) => [r.seq, r]));
  for (const event of journal) {
    projected = applyLedger(projected, event);
    if (bySeq.has(event.seq)) {
      const receipt: any = bySeq.get(event.seq);
      expect(
        pipelineScenes(projected, Object.values(projected.groups)).find(
          (scene) => scene.id === receipt.group,
        )!.count,
      ).toBeGreaterThan(0);
      expect(receipt.observed).toEqual(
        pipelineScenes(projected, Object.values(projected.groups))
          .filter((g) => g.count > 0)
          .map((g) => g.id),
      );
    }
  }
  await page.reload();
  await expect(timeline).toHaveAttribute("data-motion-enabled", "true");
  await expect(
    page.locator('.timeline-step:not([data-motion="off"])'),
  ).toHaveCount(1);
  await expect(page.locator('[data-stage-visual="result"]')).toHaveText(
    "Run ended without a terminal engine receipt",
  );
  await info.attach("timeline-receipt", {
    body: JSON.stringify({
      id: final.id,
      motion_receipts: motions,
      desktop_and_mobile_neighbors_visible: true,
      values_match_engine: true,
      pause_does_not_pause_events: true,
      scrolling_controls_readability: true,
      stage_map_matches_presented_card_and_observed_receipts: operations,
    }),
    contentType: "application/json",
  });
});
