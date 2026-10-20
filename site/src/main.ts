import "./style.css";
import { lessons } from "./lessons";
import { Workbench } from "./workbench";

const $ = <T extends HTMLElement = HTMLElement>(id: string) =>
  document.getElementById(id) as T;
let selected = Math.max(
  0,
  lessons.findIndex((lesson) => location.hash === `#${lesson.id}`),
);
let step = 0;
let timer: ReturnType<typeof setTimeout> | undefined;
let playing = false;
let visible = true;
let view: Workbench | undefined;
let graphicsAvailable = false;
const completed = new Set<number>();
const reduced = matchMedia("(prefers-reduced-motion: reduce)");
const canvas = $<HTMLCanvasElement>("heap-canvas");
const play = $<HTMLButtonElement>("play");
const stepLabels = ["Allocate", "Fill", "Overflow", "Consequence"];

$("lesson-nav").innerHTML = lessons
  .map(
    (lesson, index) =>
      `<button class="lesson-button" data-lesson="${index}" aria-current="${selected === index}" aria-controls="lesson-title"><span class="lesson-number">0${index + 1}</span><span><strong>${lesson.title}</strong><small>${lesson.short}</small></span><span class="lesson-arrow" aria-hidden="true">↗</span></button>`,
  )
  .join("");

function fallback() {
  graphicsAvailable = false;
  $("scene-fallback").hidden = false;
  canvas.style.visibility = "hidden";
  $("reset-view").hidden = true;
  $("orbit-hint").hidden = true;
  document.querySelector(".model-badge")!.textContent = "TEXT WORKSHOP";
}
try {
  view = new Workbench(canvas, fallback);
  graphicsAvailable = true;
  canvas.dataset.ready = "true";
} catch (error) {
  console.info("3D unavailable; the text workshop remains available.", error);
  fallback();
}

canvas.addEventListener("workbenchrestored", () => {
  graphicsAvailable = true;
  $("scene-fallback").hidden = true;
  canvas.style.visibility = "";
  $("reset-view").hidden = false;
  $("orbit-hint").hidden = false;
  document.querySelector(".model-badge")!.innerHTML =
    "<span></span> INTERACTIVE 3D";
  view?.show(lessons[selected], step, false);
});
if (matchMedia("(pointer: coarse)").matches)
  $("orbit-hint").textContent = "Two fingers to look around";

function stop() {
  clearTimeout(timer);
  playing = false;
  play.innerHTML = `<span aria-hidden="true">▶</span> ${step === 3 ? "Replay scenario" : "Play scenario"}`;
  play.setAttribute(
    "aria-label",
    step === 3 ? "Replay scenario" : "Play scenario",
  );
}
function memory() {
  const lesson = lessons[selected];
  const count =
    lesson.id === "callback" ? 16 : lesson.id === "size-wrap" ? 10 : 12;
  $("memory-cells").innerHTML = Array.from({ length: count }, (_, i) => {
    const neighbor = i >= lesson.capacity;
    const filled =
      step >= 1 && !neighbor && !(lesson.id === "size-wrap" && step === 1);
    const corrupt = step >= 2 && neighbor && i < lesson.writes;
    const zero = lesson.id === "off-by-one" && i === 8 && corrupt;
    return `<span class="memory-cell ${neighbor ? "neighbor-cell" : ""} ${i === lesson.capacity ? "first-neighbor" : ""} ${filled ? "filled" : ""} ${corrupt ? "corrupt" : ""}" aria-hidden="true">${zero ? "00" : corrupt ? "×" : filled ? "41" : "·"}</span>`;
  }).join("");
  $("memory-cells").setAttribute(
    "aria-label",
    `${lesson.id === "size-wrap" ? "32-byte" : `${lesson.capacity}-byte`} buffer. ${step >= 2 ? (lesson.id === "off-by-one" ? "One extra zero byte overwrites the next field." : "Overflowing input has overwritten neighboring memory.") : step === 1 && lesson.id !== "size-wrap" ? "The buffer is full. Neighboring memory is unchanged." : "The allocation is empty. Neighboring memory is unchanged."}`,
  );
  $("memory-scale").textContent =
    lesson.id === "size-wrap"
      ? "ONE CELL = 16 BYTES · FIRST 160 SHOWN"
      : "ONE CELL = ONE BYTE · × = OVERWRITE";
  $("buffer-label").textContent =
    lesson.id === "size-wrap"
      ? "Buffer · 32 bytes"
      : `Buffer · ${lesson.capacity} bytes`;
  $("target-label").textContent = lesson.target;
}
function show(animate = true) {
  const lesson = lessons[selected],
    state = lesson.steps[step];
  $("lesson-kicker").textContent =
    `SCENARIO 0${selected + 1} / ${lesson.object.toUpperCase()}`;
  $("lesson-title").textContent = lesson.title;
  $("lesson-subtitle").textContent = lesson.subtitle;
  $("step-marker").textContent = `0${step + 1}`;
  $("step-title").textContent = state.title;
  $("step-text").textContent = state.text;
  $("step-action").textContent = state.action;
  $("step-count").textContent = `${step + 1} / 4`;
  $("scene-status").textContent =
    `0${step + 1} / ${stepLabels[step].toUpperCase()}`;
  $("impact-title").textContent = lesson.impact;
  $("impact-text").textContent = lesson.condition;
  $("repair-text").textContent = lesson.repair;
  $("limit-text").textContent = lesson.limit;
  $("lesson-source").textContent = `${lesson.sourceLabel} ↗`;
  $<HTMLAnchorElement>("lesson-source").href = lesson.source;
  $("scene-wrap").dataset.lesson = lesson.id;
  $("scene-wrap").dataset.step = String(step);
  $("scene-fallback").querySelector("p")!.textContent = state.text;
  document
    .querySelectorAll<HTMLButtonElement>("button[data-step]")
    .forEach((button) =>
      button.setAttribute(
        "aria-pressed",
        String(Number(button.dataset.step) === step),
      ),
    );
  document
    .querySelectorAll<HTMLButtonElement>("button[data-lesson]")
    .forEach((button) =>
      button.setAttribute(
        "aria-current",
        String(Number(button.dataset.lesson) === selected),
      ),
    );
  if (step === 3) completed.add(selected);
  $("explored-count").textContent = `${completed.size} / 5 explored`;
  memory();
  if (graphicsAvailable) view?.show(lesson, step, animate && visible);
  if (!playing) stop();
  if (reduced.matches) motionPreference();
}
function select(index: number, updateURL = true) {
  stop();
  selected = index;
  step = 0;
  if (updateURL) history.replaceState(null, "", `#${lessons[index].id}`);
  show();
}
document
  .querySelectorAll<HTMLButtonElement>("button[data-lesson]")
  .forEach((button) => {
    button.addEventListener("click", () =>
      select(Number(button.dataset.lesson)),
    );
  });
document
  .querySelectorAll<HTMLButtonElement>("button[data-step]")
  .forEach((button) => {
    button.addEventListener("click", () => {
      stop();
      step = Number(button.dataset.step);
      show();
    });
  });
play.addEventListener("click", () => {
  if (playing) {
    stop();
    view?.stopMotion();
    return;
  }
  if (step === 3) step = 0;
  playing = true;
  show();
  play.innerHTML = '<span aria-hidden="true">Ⅱ</span> Pause';
  play.setAttribute("aria-label", "Pause scenario");
  const next = () => {
    if (!visible || document.hidden || reduced.matches) {
      stop();
      return;
    }
    step++;
    show();
    if (step === 3) stop();
    else timer = setTimeout(next, 4800);
  };
  timer = setTimeout(next, 4800);
});
function motionPreference() {
  if (reduced.matches) {
    stop();
    view?.stopMotion();
  }
  play.disabled = reduced.matches;
  if (reduced.matches) {
    play.textContent = "Use step controls";
    play.setAttribute(
      "aria-label",
      "Reduced motion: use the four step controls",
    );
  }
}
reduced.addEventListener("change", motionPreference);
$("reset-view").addEventListener("click", () => view?.reset());
document.addEventListener("visibilitychange", () => {
  if (document.hidden) {
    stop();
    view?.stopMotion();
    motionPreference();
  }
});
const observer = new IntersectionObserver(
  (entries) => {
    visible = entries[0].isIntersecting;
    if (!visible) {
      stop();
      view?.stopMotion();
      motionPreference();
    }
  },
  { threshold: 0.15 },
);
observer.observe($("scene-wrap"));
window.addEventListener("hashchange", () => {
  const index = lessons.findIndex(
    (lesson) => location.hash === `#${lesson.id}`,
  );
  if (index >= 0) select(index, false);
});
window.addEventListener("pagehide", () => {
  stop();
  view?.stopMotion();
});
show(false);
motionPreference();
