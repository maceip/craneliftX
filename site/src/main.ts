import "./style.css";
import { chapters, type View } from "./story";
import { MemoryScene, blockKind, COUNT, type Projection } from "./memory-scene";
const el = <T extends HTMLElement = HTMLElement>(id: string) =>
  document.getElementById(id) as T;
const escape = (s: string) =>
  s.replace(
    /[&<>"']/g,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        c
      ]!,
  );
const narrative = el("narrative"),
  rail = el("chapter-rail");
narrative.innerHTML = chapters
  .map(
    (c, i) =>
      '<section class="chapter" id="' +
      c.id +
      '" aria-labelledby="heading-' +
      c.id +
      '">' +
      '<div class="chapter-heading"><div class="chapter-meta"><p class="chapter-kicker">' +
      c.kicker +
      "</p>" +
      (i === 6
        ? '<span class="chapter-part">PART TWO / THE DEFENSES</span>'
        : "") +
      '</div><h2 id="heading-' +
      c.id +
      '">' +
      c.title +
      "</h2></div>" +
      '<div class="terminal actor-' +
      c.actor +
      '"><div class="terminal-bar"><span class="terminal-role">' +
      c.actor.toUpperCase() +
      '</span><span class="terminal-session">ILLUSTRATED SESSION</span><span class="terminal-window" aria-hidden="true"><i></i><i></i></span></div>' +
      '<div class="terminal-body"><div class="terminal-prompt">' +
      (c.actor === "attacker"
        ? "remote / crafted input"
        : "local / service runtime") +
      '</div><div class="terminal-command"><span aria-hidden="true"><span class="prompt-symbol">~&gt;</span><span class="typed-text">' +
      escape(c.command) +
      '</span><i class="cursor"></i></span><span class="sr-only">Command: ' +
      escape(c.command) +
      '</span></div><pre class="terminal-output">' +
      escape(c.output.join("\n")) +
      "</pre></div>" +
      '<div class="terminal-footer"><span class="sr-only">' +
      (c.part === "exploit" ? "UNPROTECTED MODEL" : "LOCKSTEP MODEL") +
      '</span><span class="terminal-metric"><span>' +
      c.metric +
      "</span><strong>" +
      escape(c.value) +
      "</strong></span><span>" +
      String(i).padStart(2, "0") +
      " / 13</span></div></div>" +
      '<p class="chapter-description">' +
      c.description +
      "</p>" +
      "<details><summary>Scope & implementation</summary><p>" +
      c.note +
      '</p><a href="' +
      c.source +
      '">INSPECT THE SOURCE ↗</a></details>' +
      (i === 0
        ? '<p class="scroll-cue">SCROLL TO FOLLOW THE ATTACK ↓</p>'
        : "") +
      "</section>",
  )
  .join("");
rail.innerHTML = chapters
  .map(
    (c, i) =>
      '<a href="#' +
      c.id +
      '" data-part="' +
      c.part +
      '" aria-label="' +
      String(i).padStart(2, "0") +
      ": " +
      c.title +
      '" title="' +
      c.title +
      '"></a>',
  )
  .join("");
const sections = Array.from(document.querySelectorAll<HTMLElement>(".chapter"));
const stage = document.querySelector<HTMLElement>(".memory-stage")!,
  diagram = el("diagram"),
  canvas = el<HTMLCanvasElement>("memory-canvas"),
  fallback = el("flat-memory");
fallback.innerHTML = Array.from({ length: COUNT }, () => "<i></i>").join("");
const reduce = matchMedia("(prefers-reduced-motion: reduce)");
let paused = reduce.matches,
  index = -1,
  typing = 0,
  navigating = false,
  navigationTimer = 0;
let scene: MemoryScene | undefined, projection: Projection | undefined;
const labels: Record<View, [string, string, string]> = {
  service: ["INPUT / 32 B", "CALLBACK", "handle_request"],
  overflow: ["INPUT / 32 B", "OVERWRITTEN CALLBACK", "other_target"],
  data: ["INPUT / 32 B", "PERMISSION: ALLOW", "access granted"],
  metadata: ["INPUT / 32 B", "FALSE CHUNK BOUNDARY", "wrong region"],
  byte: ["INPUT / 32 B", "LOCKED: 01 → 00", "is_locked → false"],
  wrap: ["ALLOCATED / 32 B", "COPY / 288 B →", "corrupted neighbor"],
  tags: ["POINTER TAG A", "TAG B / UNCHANGED", "handle_request"],
  quarantine: ["FREED / OLD TAG A", "NEIGHBOR / LIVE", "stale access ✕"],
  placement: ["BUFFER", "TARGET / NEW SLOT", "unpredictable adjacency"],
  pac: ["ORIGINAL SLOT A", "RAW BYTES / SLOT B", "authentication ✕"],
  cfi: ["INSTRUCTION START", "INTERIOR BYTE +2", "invalid destination ✕"],
  diversify: ["GUEST BLOCK", "TRANSLATED BLOCKS", "fresh layout"],
  isolation: ["GUEST HEAP", "GUEST ALLOCATION", "host / separate"],
  receipt: ["INPUT / 32 B", "CALLBACK / INTACT", "expected fault ✓"],
};
function annotate(p: Projection) {
  projection = p;
  const {
    buffer: [x, y],
    target: [tx, ty],
    width: w,
    height: h,
  } = p;
  const bufferLabel = el("buffer-label"),
    targetLabel = el("target-label");
  const labelX = (label: HTMLElement, desired: number) =>
    Math.max(4, Math.min(desired, w - label.offsetWidth - 4));
  const bufferY =
    chapters[index]?.id === "diversify"
      ? y + 18
      : Math.max(4, y - bufferLabel.offsetHeight - 18);
  bufferLabel.style.transform = `translate(${labelX(bufferLabel, x - 14)}px,${bufferY}px)`;
  targetLabel.style.transform = `translate(${labelX(targetLabel, tx - 6)}px,${Math.min(ty + 24, h - targetLabel.offsetHeight - 40)}px)`;
  const diagramBox = diagram.getBoundingClientRect();
  const stackPointer = document
    .querySelector<HTMLElement>(".stack-pointer")!
    .getBoundingClientRect();
  const codeTarget = el("code-target").getBoundingClientRect();
  const useY = Math.min(ty - 18, codeTarget.bottom - diagramBox.top + 12);
  document
    .getElementById("stack-route")!
    .setAttribute(
      "d",
      "M " +
        (stackPointer.right - diagramBox.left) +
        " " +
        (stackPointer.top + stackPointer.height / 2 - diagramBox.top) +
        " H " +
        (x - 26) +
        " V " +
        y +
        " H " +
        x,
    );
  document
    .getElementById("use-route")!
    .setAttribute(
      "d",
      "M " +
        tx +
        " " +
        (ty - 7) +
        " V " +
        (useY + 12) +
        " Q " +
        tx +
        " " +
        useY +
        " " +
        (tx + 12) +
        " " +
        useY +
        " H " +
        w * 0.86,
    );
  document
    .getElementById("write-route")!
    .setAttribute(
      "d",
      "M " +
        (x - 6) +
        " " +
        (y + 5) +
        " L " +
        (chapters[index]?.id === "tags" ? tx - (tx - x) / 8 - 5 : tx - 12) +
        " " +
        (ty + 5),
    );
}
function finishTyping() {
  if (index >= 0)
    sections[index].querySelector(".typed-text")!.textContent =
      chapters[index].command;
}
function setMotion() {
  el("motion").setAttribute("aria-pressed", String(paused));
  el("motion").setAttribute(
    "aria-label",
    paused ? "Enable typing and transitions" : "Pause typing and transitions",
  );
  el("motion").textContent = paused ? "▷" : "Ⅱ";
  document.body.classList.toggle("motion-paused", paused);
  if (paused) {
    clearTimeout(typing);
    finishTyping();
    scene?.show(chapters[Math.max(index, 0)].id, false);
  }
}
function activate(next: number, animate = true) {
  if (next === index) return;
  clearTimeout(typing);
  finishTyping();
  index = next;
  const c = chapters[index],
    dangerous = c.part === "exploit" && index > 0,
    protectedView = c.part !== "exploit";
  sections.forEach((s, i) => s.classList.toggle("active", i === index));
  Array.from(rail.children).forEach((a, i) =>
    i === index
      ? a.setAttribute("aria-current", "step")
      : a.removeAttribute("aria-current"),
  );
  stage.dataset.part = c.part;
  diagram.dataset.scene = c.id;
  el("scene-title").textContent = c.label;
  el("scene-detail").textContent = c.detail;
  el("scene-count").textContent = String(index).padStart(2, "0") + " / 13";
  el("execution-mode").textContent =
    c.part === "exploit" ? "UNPROTECTED SERVICE" : "LOCKSTEP ENABLED";
  el("verdict").textContent = c.status;
  document.querySelector(".legend > span:last-child")!.lastChild!.textContent =
    c.id === "cfi" ? "target" : c.part === "defense" ? "attempt" : "changed";
  el("scene-verdict").className =
    "scene-verdict " +
    (dangerous
      ? "danger"
      : c.id === "quarantine"
        ? "caution"
        : protectedView
          ? "blocked"
          : "");
  el("verdict-mark").textContent = dangerous ? "↗" : protectedView ? "⊣" : "●";
  el("buffer-label").textContent = labels[c.id][0];
  el("target-label").textContent = labels[c.id][1];
  el("code-value").textContent = labels[c.id][2];
  el("target-label").classList.toggle("danger", dangerous || c.id === "pac");
  el("code-target").className =
    "code-target " + (dangerous ? "danger" : protectedView ? "blocked" : "");
  document.querySelector(".heap-name")!.textContent = [
    "cfi",
    "diversify",
  ].includes(c.id)
    ? "CODE"
    : "HEAP";
  document.querySelector(".model-note")!.textContent =
    c.id === "cfi"
      ? "Illustrated code · each block is 1 byte · known instruction extents"
      : c.id === "diversify"
        ? "Illustrated model · code blocks are schematic · not a live terminal"
        : "Illustrated model · each heap block is 8 bytes · not a live terminal";
  el("code-label").textContent =
    c.id === "diversify" ? "NATIVE COMPILATION" : "LATER USE";
  document.getElementById("use-route")!.style.stroke = dangerous
    ? "#ff644a"
    : "#81b59e";
  document.getElementById("use-route")!.style.opacity = [
    "isolation",
    "placement",
    "quarantine",
  ].includes(c.id)
    ? "0"
    : "1";
  document.getElementById("write-route")!.style.opacity =
    dangerous || ["tags", "pac", "cfi"].includes(c.id) ? "1" : "0";
  el<HTMLButtonElement>("previous").disabled = index === 0;
  el<HTMLButtonElement>("next").disabled = index === chapters.length - 1;
  Array.from(fallback.children).forEach(
    (b, i) => (b.className = blockKind(c.id, i)),
  );
  fallback.dataset.byte = String(c.id === "byte");
  scene?.show(c.id, animate && !paused);
  if (projection) annotate(projection);
  if (animate && !paused && !document.hidden) {
    const node = sections[index].querySelector(".typed-text")!;
    let length = 0;
    const type = () => {
      length += 2;
      node.textContent = c.command.slice(0, length);
      if (length < c.command.length) typing = window.setTimeout(type, 24);
    };
    type();
  }
}
try {
  scene = new MemoryScene(
    canvas,
    (ready) => {
      fallback.hidden = ready;
      canvas.style.visibility = ready ? "visible" : "hidden";
    },
    annotate,
  );
} catch {
  fallback.hidden = false;
  canvas.style.visibility = "hidden";
}
new ResizeObserver(() => {
  if (!fallback.hidden)
    annotate({
      buffer: [diagram.clientWidth * 0.35, diagram.clientHeight * 0.5],
      target: [diagram.clientWidth * 0.65, diagram.clientHeight * 0.48],
      width: diagram.clientWidth,
      height: diagram.clientHeight,
    });
}).observe(diagram);
new IntersectionObserver(
  (entries) => scene?.setVisible(entries[0].isIntersecting),
  { threshold: 0 },
).observe(stage);
document.addEventListener("visibilitychange", () => {
  scene?.setVisible(!document.hidden);
  if (document.hidden) {
    clearTimeout(typing);
    finishTyping();
  }
});
el("motion").addEventListener("click", () => {
  paused = !paused;
  setMotion();
});
reduce.addEventListener("change", () => {
  paused = reduce.matches;
  setMotion();
});
setMotion();
function go(
  next: number,
  behavior: ScrollBehavior = paused ? "instant" : "smooth",
) {
  if (next < 0 || next >= chapters.length) return;
  navigating = true;
  clearTimeout(navigationTimer);
  activate(next);
  sections[next].scrollIntoView({ behavior, block: "start" });
  navigationTimer = window.setTimeout(
    () => {
      navigating = false;
      observeChapters();
    },
    behavior === "instant" ? 50 : 1100,
  );
}
el("previous").addEventListener("click", () => {
  history.pushState(null, "", "#" + chapters[index - 1].id);
  go(index - 1);
});
el("next").addEventListener("click", () => {
  history.pushState(null, "", "#" + chapters[index + 1].id);
  go(index + 1);
});
document.addEventListener("click", (e) => {
  const a = (e.target as Element).closest<HTMLAnchorElement>('a[href^="#"]');
  if (!a) return;
  const n = chapters.findIndex((c) => "#" + c.id === a.getAttribute("href"));
  if (n < 0) return;
  e.preventDefault();
  history.pushState(null, "", "#" + chapters[n].id);
  go(n);
});
window.addEventListener("hashchange", () => {
  const n = chapters.findIndex((c) => "#" + c.id === location.hash);
  if (n >= 0) go(n);
});
let observer: IntersectionObserver;
function observeChapters() {
  observer?.disconnect();
  const mobile = innerWidth <= 760,
    header = document.querySelector<HTMLElement>(".site-header")!.offsetHeight,
    stageHeight = mobile ? stage.offsetHeight : 0;
  const top = Math.min(
    innerHeight - 20,
    header + stageHeight + (mobile ? 50 : innerHeight * 0.23),
  );
  observer = new IntersectionObserver(
    (entries) => {
      if (navigating) return;
      const entry = entries.find((e) => e.isIntersecting);
      if (entry) activate(sections.indexOf(entry.target as HTMLElement));
    },
    {
      rootMargin:
        "-" + top + "px 0px -" + Math.max(0, innerHeight - top - 2) + "px 0px",
      threshold: 0,
    },
  );
  sections.forEach((s) => observer.observe(s));
}
window.addEventListener("resize", observeChapters);
activate(
  Math.max(
    0,
    chapters.findIndex((c) => "#" + c.id === location.hash),
  ),
  false,
);
observeChapters();
if (location.hash && chapters.some((c) => "#" + c.id === location.hash))
  requestAnimationFrame(() => go(index, "instant"));
