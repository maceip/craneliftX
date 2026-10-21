import * as THREE from "three";
import type { View } from "./story";
export const COUNT = 96;
export function blockKind(view: View, i: number): string {
  if (view === "wrap" && i >= 42 && i < 74) return "changed";
  if (view === "cfi" && i === 40) return "changed";
  if (view === "pac" && ((i > 38 && i < 42) || (i > 42 && i < 46)))
    return "idle";
  const target = view === "placement" ? 16 : 42;
  if (i >= 38 && i < 42) return view === "quarantine" ? "quarantine" : "owned";
  if (i >= target && i < target + 4) {
    if (["overflow", "wrap"].includes(view) && i < target + 2) return "changed";
    if (["data", "metadata", "pac"].includes(view) && i === target)
      return "changed";
    return "neighbor";
  }
  return "idle";
}
const colors: Record<string, number> = {
  idle: 0x27352f,
  owned: 0x87dfb1,
  neighbor: 0xd0d5ca,
  changed: 0xff472d,
  quarantine: 0xe5bf68,
};
const at = (i: number) =>
  new THREE.Vector3(
    ((i % 12) - 5.5) * 0.82 + 1.1,
    0,
    (Math.floor(i / 12) - 3.5) * 0.88,
  );
export type Projection = {
  buffer: [number, number];
  target: [number, number];
  width: number;
  height: number;
};
export class MemoryScene {
  private renderer: THREE.WebGLRenderer;
  private scene = new THREE.Scene();
  private camera = new THREE.OrthographicCamera();
  private blocks: THREE.InstancedMesh;
  private sliver: THREE.Mesh;
  private barrier: THREE.LineSegments;
  private dummy = new THREE.Object3D();
  private shades = Array.from(
    { length: COUNT },
    () => new THREE.Color(colors.idle),
  );
  private elevations = Array.from({ length: COUNT }, () => 0.15);
  private raf = 0;
  private visible = true;
  private lost = false;
  private view: View = "service";
  private width = 1;
  private height = 1;
  private observer: ResizeObserver;
  constructor(
    private canvas: HTMLCanvasElement,
    private availability: (ready: boolean) => void,
    private project: (p: Projection) => void,
  ) {
    this.renderer = new THREE.WebGLRenderer({
      canvas,
      antialias: true,
      alpha: true,
      powerPreference: "low-power",
    });
    this.renderer.setPixelRatio(Math.min(devicePixelRatio, 1.7));
    this.renderer.setClearColor(0x0b0f11, 0);
    this.renderer.outputColorSpace = THREE.SRGBColorSpace;
    this.blocks = new THREE.InstancedMesh(
      new THREE.BoxGeometry(0.69, 1, 0.72),
      new THREE.MeshStandardMaterial({ roughness: 0.9 }),
      COUNT,
    );
    this.blocks.instanceMatrix.setUsage(THREE.DynamicDrawUsage);
    this.scene.add(
      this.blocks,
      new THREE.HemisphereLight(0xeafff2, 0x1a2820, 2.5),
    );
    const light = new THREE.DirectionalLight(0xffffff, 2);
    light.position.set(-4, 12, 7);
    this.scene.add(light);
    this.sliver = new THREE.Mesh(
      new THREE.BoxGeometry(0.086, 0.36, 0.73),
      new THREE.MeshBasicMaterial({ color: colors.changed }),
    );
    this.scene.add(this.sliver);
    this.barrier = new THREE.LineSegments(
      new THREE.EdgesGeometry(new THREE.BoxGeometry(0.045, 1.05, 0.85)),
      new THREE.LineBasicMaterial({ color: 0xc2ffd6 }),
    );
    this.scene.add(this.barrier);
    this.observer = new ResizeObserver(() => this.resize());
    this.observer.observe(canvas);
    canvas.addEventListener("webglcontextlost", (e) => {
      e.preventDefault();
      this.lost = true;
      this.stop();
      this.availability(false);
    });
    canvas.addEventListener("webglcontextrestored", () => {
      this.lost = false;
      this.availability(true);
      this.resize();
      this.show(this.view, false);
    });
    this.resize();
  }
  private resize() {
    this.width = this.canvas.clientWidth;
    this.height = this.canvas.clientHeight;
    if (!this.width || !this.height || this.lost) return;
    this.renderer.setSize(this.width, this.height, false);
    const aspect = this.width / this.height,
      worldWidth = Math.max(13.5, 9.4 * aspect);
    this.camera.left = -worldWidth / 2;
    this.camera.right = worldWidth / 2;
    this.camera.top = worldWidth / aspect / 2;
    this.camera.bottom = -this.camera.top;
    this.camera.near = 0.1;
    this.camera.far = 100;
    this.camera.position.set(2.8, 14, 15);
    this.camera.lookAt(0, 0, 0);
    this.camera.updateProjectionMatrix();
    this.camera.updateMatrixWorld();
    this.paint();
  }
  stop() {
    cancelAnimationFrame(this.raf);
    this.raf = 0;
    this.canvas.dataset.animating = "false";
  }
  setVisible(visible: boolean) {
    this.visible = visible;
    if (!visible) this.stop();
    else this.show(this.view, false);
  }
  show(view: View, animate: boolean) {
    this.stop();
    this.view = view;
    const start = this.shades.map((c) => c.clone()),
      heights = [...this.elevations];
    const target = Array.from(
      { length: COUNT },
      (_, i) => new THREE.Color(colors[blockKind(view, i)]),
    );
    const draw = (t: number) => {
      for (let i = 0; i < COUNT; i++) {
        this.shades[i].copy(start[i]).lerp(target[i], t);
        this.elevations[i] = THREE.MathUtils.lerp(
          heights[i],
          blockKind(view, i) !== "idle" ? 0.36 : 0.13,
          t,
        );
      }
      this.paint();
    };
    if (!animate || !this.visible || this.lost || document.hidden) {
      draw(1);
      return;
    }
    const started = performance.now();
    this.canvas.dataset.animating = "true";
    const tick = (now: number) => {
      const t = Math.min(1, (now - started) / 700);
      draw(t * t * (3 - 2 * t));
      if (t < 1 && this.visible && !this.lost && !document.hidden)
        this.raf = requestAnimationFrame(tick);
      else this.stop();
    };
    this.raf = requestAnimationFrame(tick);
  }
  private position(i: number) {
    if (this.view === "diversify") {
      if (i >= 38 && i < 42) return at(26 + i - 38);
      if (i >= 42 && i < 46) return at(62 + [2, 0, 3, 1][i - 42]);
    }
    return at(i);
  }
  private paint() {
    if (this.lost || !this.width || !this.height) return;
    for (let i = 0; i < COUNT; i++) {
      this.dummy.position.copy(this.position(i));
      this.dummy.position.y = this.elevations[i] / 2;
      this.dummy.scale.set(1, this.elevations[i], 1);
      if (this.view === "diversify" && i >= 38 && i < 46)
        this.dummy.position.z += ((i % 3) - 1) * 0.14;
      this.dummy.updateMatrix();
      this.blocks.setMatrixAt(i, this.dummy.matrix);
      this.blocks.setColorAt(i, this.shades[i]);
    }
    this.blocks.instanceMatrix.needsUpdate = true;
    this.blocks.instanceColor!.needsUpdate = true;
    this.sliver.visible = this.view === "byte";
    this.sliver.position.copy(at(this.view === "cfi" ? 40 : 42));
    this.sliver.position.x -= 0.3;
    this.sliver.position.y = 0.2;
    this.barrier.visible = [
      "tags",
      "quarantine",
      "pac",
      "cfi",
      "isolation",
      "receipt",
    ].includes(this.view);
    this.barrier.position.copy(at(this.view === "cfi" ? 38 : 42));
    this.barrier.position.x -= 0.41;
    this.barrier.position.y = 0.5;
    this.renderer.render(this.scene, this.camera);
    this.canvas.dataset.frames = String(
      Number(this.canvas.dataset.frames || 0) + 1,
    );
    const point = (i: number): [number, number] => {
      const p = this.position(i);
      p.y = 0.4;
      p.project(this.camera);
      return [((p.x + 1) * this.width) / 2, ((1 - p.y) * this.height) / 2];
    };
    this.project({
      buffer: point(38),
      target: point(
        this.view === "placement" ? 16 : this.view === "cfi" ? 40 : 42,
      ),
      width: this.width,
      height: this.height,
    });
  }
}
