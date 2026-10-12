import { useEffect, useLayoutEffect, useRef } from "react";
import { Run } from "../pipeline";

type Factory = {
  setState: (value: any) => void;
  setRun: (run: Run | null) => void;
  resize: () => void;
  dispose: () => void;
  getState: () => any;
};
const rendererURL = "/dist/factory/factory-renderer.js";
// One renderer moves between card hosts. Its canvas is owned by the renderer,
// not React; moving it preserves the WebGL context and the designer's camera.
export function FactoryBackdrop({
  scene,
  active,
  reduced,
  suspended,
  run,
}: {
  scene: string;
  active: boolean;
  reduced: boolean;
  suspended: boolean;
  run: Run | null;
}) {
  const layer = useRef<HTMLDivElement | null>(null),
    factory = useRef<Factory | null>(null);
  const latest = useRef({ scene, active, reduced, run });
  latest.current = { scene, active, reduced, run };
  const position = () => {
    const host = document.querySelector(
      `#timeline-${latest.current.scene} .timeline-card`,
    );
    if (host && layer.current && layer.current.parentElement !== host) {
      host.appendChild(layer.current);
    }
  };
  useLayoutEffect(() => {
    const wrapper = document.createElement("div"),
      canvas = document.createElement("canvas");
    wrapper.className = "factory-backdrop";
    wrapper.setAttribute("aria-hidden", "true");
    wrapper.appendChild(canvas);
    layer.current = wrapper;
    position();
    let disposed = false;
    void import(rendererURL)
      .then((module) => {
        if (disposed) return;
        const props = latest.current;
        factory.current = module.createFactory(canvas, {
          quality: "backdrop",
          scene: props.scene,
          active: props.active,
          reducedMotion: props.reduced,
        });
        factory.current!.setRun(props.run);
        // Read-only inspection for browser acceptance, not a second controller.
        (canvas as any).factoryState = () => factory.current?.getState();
        wrapper.dataset.ready = "true";
      })
      .catch(() => {
        /* Foreground diagrams remain complete without WebGL. */
      });
    return () => {
      disposed = true;
      factory.current?.dispose();
      factory.current = null;
      wrapper.remove();
      layer.current = null;
    };
  }, []);
  useLayoutEffect(() => {
    // Keep the background on its previous card while the foreground controls
    // expand. Moving/resizing WebGL here can block that short transition.
    if (suspended) {
      factory.current?.setState({ active: false, paused: true });
      return;
    }
    position();
    factory.current?.setState({
      scene,
      active,
      paused: !active,
      reducedMotion: reduced,
    });
  }, [scene, active, reduced, suspended, run?.id]);
  useEffect(() => {
    if (!suspended) factory.current?.setRun(run);
  }, [run, suspended]);
  return null;
}
