import React, { useEffect, useLayoutEffect, useRef, useState } from "react";
import { ArchitectureRail } from "./ArchitectureRail";
import { TransportIcon } from "./TransportIcon";
import { TelemetryGauge } from "./Telemetry";
import { CardIllustration, hasStoryData } from "./CardIllustration";
import { FactoryBackdrop } from "./FactoryBackdrop";
import {
  IllustrationEvidence,
  Walkthrough,
  useIllustrationEvidence,
  useWalkthrough,
} from "../walkthrough";
import { Scene, sceneStatus } from "../pipelineScenes";
import { number, Run, timestamp } from "../pipeline";
const idleClock: Walkthrough = { progress: 0, phase: 0, playing: false };

function TimelineStep({
  group,
  run,
  selected,
  context,
  onSelect,
  onFocus,
  enabled,
  confirming,
  clock,
  evidence,
  reduced,
  onEvidence,
  next,
  onAdvance,
  cueAllowed,
}: {
  group: Scene;
  run: Run | null;
  selected: boolean;
  context: string;
  onSelect: () => void;
  onFocus: () => void;
  enabled: boolean;
  confirming: boolean;
  clock: Walkthrough;
  evidence: IllustrationEvidence;
  reduced: boolean;
  onEvidence: () => void;
  next?: Scene;
  onAdvance: () => void;
  cueAllowed: boolean;
}) {
  const element = useRef<HTMLLIElement>(null);
  const readingCenter = useRef<HTMLSpanElement>(null);
  const pointerFocus = useRef(false);
  const [visible, setVisible] = useState(false),
    [prominence, setProminence] = useState(0);
  const [inPosition, setInPosition] = useState(false),
    [advanced, setAdvanced] = useState(false);
  useEffect(() => setAdvanced(false), [selected]);
  const seq = group.last_event?.seq ?? 0;
  const state = group.state;
  useEffect(() => {
    const observer = new IntersectionObserver(([entry]) =>
      setVisible(entry.isIntersecting),
    );
    if (element.current) observer.observe(element.current);
    return () => observer.disconnect();
  }, []);
  useEffect(() => {
    let observer: IntersectionObserver;
    const observe = () => {
      observer?.disconnect();
      // The center 60% of the viewport controls readability, independently of
      // selection and engine activity. No scroll listener or animation loop.
      const inset = Math.round(innerHeight * 0.2);
      observer = new IntersectionObserver(
        ([entry]) => {
          setProminence(entry.intersectionRatio);
        },
        {
          rootMargin: `-${inset}px 0px -${inset}px 0px`,
          threshold: Array.from({ length: 21 }, (_, i) => i / 20),
        },
      );
      if (element.current) observer.observe(element.current);
    };
    observe();
    window.addEventListener("resize", observe);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", observe);
    };
  }, []);
  // Reading owns the illustration's lifecycle. Fast/offscreen engine events
  // must not consume an animation before this card is reached, or restart it
  // while it is being read. Completed and restored runs use the same gate.
  const motion = enabled && visible && selected && group.count > 0;
  const sequenceReady =
    hasStoryData(group, run) && (reduced || clock.progress >= 1);
  useLayoutEffect(() => {
    setInPosition(false);
    if (!selected || !sequenceReady) return;
    let observer: IntersectionObserver;
    const observe = () => {
      observer?.disconnect();
      const marker = readingCenter.current;
      if (!marker) return;
      // Anchor to where the reader stopped, not an exact centered scroll
      // position. One measurement at completion; scrolling only changes the
      // intersection state, which fades the cue without a per-frame listener.
      const top = marker.getBoundingClientRect().top;
      observer = new IntersectionObserver(
        ([entry]) => setInPosition(entry.isIntersecting),
        {
          rootMargin: `-${Math.max(0, top - 24)}px 0px -${Math.max(0, innerHeight - top - 24)}px 0px`,
        },
      );
      observer.observe(marker);
    };
    observe();
    window.addEventListener("resize", observe);
    return () => {
      observer?.disconnect();
      window.removeEventListener("resize", observe);
    };
  }, [selected, sequenceReady]);
  const cueVisible =
    !!next &&
    selected &&
    inPosition &&
    cueAllowed &&
    !advanced &&
    sequenceReady;
  return (
    <li
      ref={element}
      id={`timeline-${group.id}`}
      className={`timeline-step ${selected ? "is-current" : ""}`}
      data-group={group.id}
      data-source-groups={group.sources.map((source) => source.id).join(" ")}
      data-context={context}
      data-state={state}
      data-motion={motion ? "focused" : "off"}
      data-motion-seq={seq || undefined}
      data-prominence={prominence.toFixed(2)}
      data-completed={state === "Completed"}
      style={
        {
          "--card-opacity": 0.42 + Math.min(1, prominence / 0.75) * 0.58,
        } as React.CSSProperties
      }
    >
      <span
        ref={readingCenter}
        className="card-reading-center"
        aria-hidden="true"
      />
      <span className="timeline-node" aria-hidden="true">
        {String(group.order).padStart(2, "0")}
      </span>
      {["Completed", "Observed"].includes(state) && (
        <span
          className="completion-detent"
          aria-label={
            state === "Completed"
              ? "Completed in this run"
              : "Recorded in this run"
          }
        />
      )}
      <div
        className="timeline-card"
        onClick={onSelect}
        onClickCapture={(event) => {
          // Inspecting an object on a neighboring card selects that card too.
          // Its own button then handles the inspection without a restart prompt.
          if (
            !selected &&
            (event.target as Element).closest(".card-story button")
          )
            onFocus();
        }}
        onPointerDownCapture={() => {
          pointerFocus.current = true;
        }}
        onPointerUpCapture={() => {
          pointerFocus.current = false;
        }}
        onPointerCancelCapture={() => {
          pointerFocus.current = false;
        }}
        onFocusCapture={() => {
          // Keyboard/programmatic focus follows the same reading selection.
          // Pointer focus is handled by click to preserve its smooth scroll.
          if (!pointerFocus.current) onFocus();
          pointerFocus.current = false;
        }}
      >
        <button
          type="button"
          className="timeline-stage-button"
          data-testid={`group-${group.id}`}
          data-state={state}
          aria-pressed={selected}
          aria-expanded={confirming}
          aria-controls={confirming ? "restart-confirmation" : undefined}
        >
          <span>
            <strong>{group.label}</strong>
            <small
              title={`Trace groups: ${group.sources.map((source) => source.id).join(", ")}`}
            >
              {group.count
                ? `${number(group.count)} ${group.count === 1 ? "event" : "events"} · ${number(group.completed)} completed ${group.completed === 1 ? "span" : "spans"}`
                : "No events recorded"}
            </small>
          </span>
          <span
            className={`timeline-state state-${state.toLowerCase().replaceAll(" ", "-")}`}
          >
            <i aria-hidden="true" />
            {sceneStatus(group, run?.status)}
          </span>
        </button>
        <div className="stage-illustration" data-stage-visual={group.id}>
          <CardIllustration
            scene={group}
            run={run}
            visible={visible}
            selected={selected}
            clock={selected ? clock : idleClock}
            evidence={evidence}
            reduced={reduced}
            onEvidence={onEvidence}
          />
          <span className="receipt-glow" aria-hidden="true" />
        </div>
        <div className="timeline-provenance">
          <span>
            {group.last_event
              ? `#${seq} · ${timestamp(group.last_event.timestamp_unix_ns)} UTC`
              : "Awaiting an engine event"}
          </span>
          <span>
            {group.last_event?.block_id !== undefined
              ? `Block ${group.last_event.block_id}`
              : group.last_event?.type === "stage"
                ? group.last_event.stage
                : (group.last_event?.type ?? "")}
          </span>
        </div>
      </div>
      {next && (
        <button
          type="button"
          className="continue-cue"
          aria-label={`Continue to ${next.label}`}
          title={`Scroll or continue to ${next.label}`}
          aria-hidden={!cueVisible}
          disabled={!cueVisible}
          tabIndex={cueVisible ? 0 : -1}
          data-visible={cueVisible}
          data-animate={enabled}
          onClick={() => {
            setAdvanced(true);
            onAdvance();
          }}
        >
          <span className="cue-motion">
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path d="m6 5 6 6 6-6M6 13l6 6 6-6" />
            </svg>
          </span>
        </button>
      )}
    </li>
  );
}
export function PipelineTimeline({
  groups,
  run,
  selected,
  onSelect,
  onView,
  controls,
  status,
  obscured,
  paused,
  onPause,
  confirmation,
  confirmationFor,
  onEvidence,
}: {
  groups: Scene[];
  run: Run | null;
  selected: string;
  onSelect: (id: string, revisit: boolean) => void;
  onView: (id: string) => void;
  controls: React.ReactNode;
  status: React.ReactNode;
  obscured: boolean;
  paused: boolean;
  onPause: (value: boolean) => void;
  confirmation?: React.ReactNode;
  confirmationFor: string | null;
  onEvidence: () => void;
}) {
  const [present, setPresent] = useState(true);
  const [reduced, setReduced] = useState(false),
    [visible, setVisible] = useState(true);
  useEffect(() => {
    const media = matchMedia("(prefers-reduced-motion: reduce)");
    const update = () => {
        setReduced(media.matches);
      },
      visibility = () => setVisible(document.visibilityState === "visible");
    update();
    visibility();
    media.addEventListener("change", update);
    document.addEventListener("visibilitychange", visibility);
    return () => {
      media.removeEventListener("change", update);
      document.removeEventListener("visibilitychange", visibility);
    };
  }, []);
  const index = groups.findIndex((g) => g.id === selected);
  const catalog = groups.map((g) => g.id).join(",");
  const observed = useRef("");
  const departed = useRef(new Set<string>());
  const viewCallback = useRef(onView);
  viewCallback.current = onView;
  const center = (id: string, behavior: ScrollBehavior = "instant") => {
    document
      .getElementById(`timeline-${id}`)
      ?.scrollIntoView({ block: "center", behavior });
  };
  // Only a new run, an explicit card click or an Evidence selection may move
  // the viewport. Engine events never select or scroll to a different group.
  useLayoutEffect(() => {
    departed.current.clear();
    observed.current = selected;
    center(selected);
  }, [run?.id, catalog]);
  const wasObscured = useRef(obscured);
  useLayoutEffect(() => {
    if (wasObscured.current && !obscured) {
      observed.current = selected;
      center(selected);
    }
    wasObscured.current = obscured;
  }, [selected, obscured]);
  useEffect(() => {
    if (!run || obscured) return;
    let observer: IntersectionObserver;
    const observe = () => {
      observer?.disconnect();
      const intersecting = new Set<HTMLElement>();
      // A narrow center band makes the map follow the card being read. The
      // observer never scrolls, so wheel, touch, scrollbar and keys stay native.
      const inset = Math.floor(innerHeight * 0.45);
      observer = new IntersectionObserver(
        (entries) => {
          for (const entry of entries) {
            if (entry.isIntersecting)
              intersecting.add(entry.target as HTMLElement);
            else intersecting.delete(entry.target as HTMLElement);
          }
          const nearest = [...intersecting].sort((a, b) => {
            const distance = (el: HTMLElement) => {
              const r = el.getBoundingClientRect();
              return Math.abs(r.top + r.height / 2 - innerHeight / 2);
            };
            return distance(a) - distance(b);
          })[0]?.dataset.group;
          if (nearest && nearest !== observed.current) {
            departed.current.add(observed.current);
            observed.current = nearest;
            viewCallback.current(nearest);
          }
        },
        { rootMargin: `-${inset}px 0px -${inset}px 0px`, threshold: 0 },
      );
      document
        .querySelectorAll<HTMLElement>(".timeline-step")
        .forEach((el) => observer.observe(el));
    };
    observe();
    window.addEventListener("resize", observe);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", observe);
    };
  }, [run?.id, obscured, catalog]);
  function selectCard(id: string) {
    const revisit = departed.current.has(id);
    // Let the viewport observer select the destination when it is actually
    // reached. Preselecting here makes the ring jump forward/back/forward as
    // the outgoing card crosses the center band during smooth scrolling.
    onSelect(id, revisit);
    center(id, reduced ? "instant" : "smooth");
  }
  function focusCard(id: string) {
    if (id !== observed.current) {
      departed.current.add(observed.current);
      observed.current = id;
      viewCallback.current(id);
    }
    center(id);
  }
  const evidence = useIllustrationEvidence(run);
  useEffect(() => {
    const element = document.getElementById(`timeline-${selected}`);
    if (!element) return;
    const observer = new IntersectionObserver(([entry]) =>
      setPresent(entry.isIntersecting),
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, [selected, run?.id]);
  const enabled =
    !paused && !obscured && !reduced && visible && present && !!run;
  const currentScene = groups.find((s) => s.id === selected);
  const clock = useWalkthrough(
    `${run?.id}:${selected}`,
    enabled && !!currentScene && hasStoryData(currentScene, run),
  );
  return (
    <section
      className="pipeline-timeline"
      aria-label="Connected pipeline timeline"
      data-testid="pipeline-timeline"
      data-motion-enabled={enabled}
      data-navigation="manual"
      data-paused={paused}
    >
      <FactoryBackdrop
        key={run?.id ?? "idle"}
        scene={selected}
        active={
          enabled &&
          !confirmation &&
          (groups.find((s) => s.id === selected)?.count ?? 0) > 0
        }
        reduced={reduced}
        suspended={!!confirmation}
        run={run}
      />
      <TelemetryGauge
        run={run}
        groups={groups}
        stage={selected}
        clock={clock}
        enabled={enabled}
        reduced={reduced}
      />
      <ArchitectureRail
        run={run}
        groups={groups}
        stage={selected}
        animate={
          enabled &&
          (groups.find((scene) => scene.id === selected)?.count ?? 0) > 0
        }
      />
      <div
        className="focus-transport"
        data-expanded={!!confirmation}
        role="group"
        aria-label="Execution controls"
      >
        <div className="visually-hidden">{status}</div>
        <div className="timeline-controls">
          {controls}
          {run && (
            <>
              <button
                type="button"
                aria-label={paused ? "Resume animations" : "Pause animations"}
                title={paused ? "Resume animations" : "Pause animations"}
                aria-pressed={paused}
                onClick={() => onPause(!paused)}
              >
                <TransportIcon kind={paused ? "play" : "pause"} />
              </button>
            </>
          )}
        </div>
        {confirmation}
      </div>
      <div className="timeline-layout">
        <ol className="timeline-list" aria-label="Pipeline operation groups">
          {groups.map((group, i) => (
            <TimelineStep
              key={`${run?.id ?? "idle"}:${group.id}`}
              group={group}
              run={run}
              selected={group.id === selected}
              context={
                i === index
                  ? "current"
                  : i === index - 1
                    ? "previous"
                    : i === index + 1
                      ? "next"
                      : "distant"
              }
              onSelect={() => selectCard(group.id)}
              onFocus={() => focusCard(group.id)}
              enabled={enabled}
              confirming={confirmationFor === group.id}
              clock={clock}
              evidence={evidence}
              reduced={reduced}
              onEvidence={onEvidence}
              next={groups[i + 1]}
              cueAllowed={!obscured && visible && !confirmation}
              onAdvance={() => {
                const next = groups[i + 1];
                if (!next) return;
                onSelect(next.id, false);
                // Hiding a focused cue blurs its button during React's commit.
                // Begin scrolling afterwards so that native blur cannot cancel
                // the smooth scroll we just requested.
                requestAnimationFrame(() =>
                  center(next.id, reduced ? "instant" : "smooth"),
                );
              }}
            />
          ))}
        </ol>
      </div>
      <p className="visually-hidden">
        Scroll, click or focus a card to inspect its recorded output. Previously
        visited cards offer a restart confirmation. The engine runs
        continuously; values and engine time update independently from the
        ledger. Illustrations animate only on the card being read, including
        recorded results. {reduced ? "Reduced motion is enabled." : ""}
      </p>
    </section>
  );
}
