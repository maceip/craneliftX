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
function elf(code: Buffer, size = 256 + code.length) {
  const image = Buffer.alloc(size);
  Buffer.from([0x7f, 0x45, 0x4c, 0x46, 2, 1, 1]).copy(image);
  image.writeUInt16LE(2, 16);
  image.writeUInt16LE(62, 18);
  image.writeUInt32LE(1, 20);
  image.writeBigUInt64LE(0x20100n, 24);
  image.writeBigUInt64LE(64n, 32);
  for (const [at, value] of [
    [52, 64],
    [54, 56],
    [56, 1],
    [58, 64],
  ])
    image.writeUInt16LE(value, at);
  image.writeUInt32LE(1, 64);
  image.writeUInt32LE(5, 68);
  image.writeBigUInt64LE(0x20000n, 80);
  image.writeBigUInt64LE(0x20000n, 88);
  image.writeBigUInt64LE(BigInt(size), 96);
  image.writeBigUInt64LE(BigInt(size), 104);
  image.writeBigUInt64LE(4096n, 112);
  code.copy(image, 256);
  return image;
}
async function upload(page: Page, name: string, buffer: Buffer) {
  await page.goto("/");
  await expect(page.getByRole("button", { name: "Open binary" })).toBeEnabled();
  await page
    .getByLabel("Choose an x86-64 ELF binary")
    .setInputFiles({ name, mimeType: "application/octet-stream", buffer });
  await expect(page).toHaveURL(/run=/, { timeout: 120000 });
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
  await expect(page.locator(".stage-button")).toHaveCount(13);
  await expect(page.getByTestId("group-audit")).toHaveAttribute(
    "data-state",
    "Not exercised",
  );
  await page.screenshot({
    path: info.outputPath("idle-desktop.png"),
    fullPage: true,
  });
  await page.getByRole("button", { name: "Run Rust server" }).click();
  await expect(page).toHaveURL(/run=/);
  await expect(page.getByText("Live stream", { exact: true })).toBeVisible();
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
  expect(final.verification?.verified_responses).toBe(15);
  expect(final.verification?.first_connection_responses).toBe(12);
  expect(final.verification?.reconnects_on_first_connection).toBe(0);
  expect(final.counters.cache_invalidations).toBeGreaterThan(0);
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
  for (const group of Object.values(final.groups))
    await expect(page.getByTestId(`group-${group.id}`)).toHaveAttribute(
      "data-state",
      groupState(group, final.status),
    );
  expect(final.groups.audit.count).toBe(0);
  await page.getByTestId("group-audit").click();
  await expect(
    page.getByRole("heading", { name: "Not exercised by this runtime" }),
  ).toBeVisible();
  await page.getByTestId("group-decode").click();
  await page.getByRole("button", { name: "View raw JSON" }).click();
  expect(JSON.parse(await page.getByTestId("raw-event").innerText())).toEqual(
    final.groups.decode.output,
  );
  await page.getByTestId("group-artifacts").click();
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
  await page.getByTestId("group-decode").click();
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
    "Not exercised",
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
    await expect(page.getByText("Reconnecting", { exact: true })).toBeVisible();
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
  await expect(page.getByTestId("group-load")).toHaveAttribute(
    "data-state",
    "Failed",
  );
  await expect(page.getByTestId("group-decode")).toHaveAttribute(
    "data-state",
    "Not exercised",
  );
});
