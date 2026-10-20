import * as THREE from "three";
import { OrbitControls } from "three/addons/controls/OrbitControls.js";
import { RoundedBoxGeometry } from "three/addons/geometries/RoundedBoxGeometry.js";
import type { Lesson } from "./lessons";

const C = {
  paper: 0xfbfaf6,
  base: 0xd8decb,
  edge: 0xb7c1a7,
  cream: 0xeee4cc,
  wood: 0xb39a75,
  orange: 0xd97b3b,
  tape: 0xf3c590,
  dark: 0x354b3d,
  teal: 0x638a75,
  pale: 0xa9c0a8,
  red: 0xb53e22,
  ink: "#344337",
};

export class Workbench {
  private scene = new THREE.Scene();
  private camera = new THREE.OrthographicCamera(-6, 6, 4, -4, 0.1, 100);
  private renderer: THREE.WebGLRenderer;
  private controls: OrbitControls;
  private content = new THREE.Group();
  private resizeObserver: ResizeObserver;
  private frame = 0;
  private alive = true;
  private lost = false;
  private generation = 0;
  private writeEdge = -0.15;
  private reduced = matchMedia("(prefers-reduced-motion: reduce)");
  private moving: {
    mesh: THREE.Object3D;
    final: THREE.Vector3;
    from: THREE.Vector3;
    delay: number;
  }[] = [];

  constructor(
    private canvas: HTMLCanvasElement,
    private onUnavailable: () => void,
  ) {
    this.renderer = new THREE.WebGLRenderer({
      canvas,
      antialias: true,
      alpha: true,
    });
    this.renderer.setPixelRatio(Math.min(devicePixelRatio, 1.75));
    this.renderer.setClearColor(C.paper, 0);
    this.renderer.shadowMap.enabled = true;
    this.renderer.shadowMap.type = THREE.PCFSoftShadowMap;
    this.renderer.outputColorSpace = THREE.SRGBColorSpace;
    this.renderer.toneMapping = THREE.ACESFilmicToneMapping;
    this.renderer.toneMappingExposure = 1.05;
    this.scene.add(new THREE.HemisphereLight(0xfffcf0, 0x829279, 1.7));
    const sun = new THREE.DirectionalLight(0xffefd5, 2.4);
    sun.position.set(-4, 9, 6);
    sun.castShadow = true;
    sun.shadow.mapSize.set(2048, 2048);
    sun.shadow.camera.left = -9;
    sun.shadow.camera.right = 9;
    sun.shadow.camera.top = 9;
    sun.shadow.camera.bottom = -9;
    sun.shadow.bias = -0.001;
    sun.shadow.normalBias = 0.02;
    this.scene.add(sun);
    const fill = new THREE.DirectionalLight(0xd7e6e2, 0.8);
    fill.position.set(5, 4, -4);
    this.scene.add(fill);
    const floor = new THREE.Mesh(
      new THREE.PlaneGeometry(200, 200),
      new THREE.ShadowMaterial({ opacity: 0.12 }),
    );
    floor.rotation.x = -Math.PI / 2;
    floor.position.y = -0.65;
    floor.receiveShadow = true;
    this.scene.add(floor, this.content);
    this.controls = new OrbitControls(this.camera, canvas);
    this.controls.enableDamping = false;
    this.controls.enableZoom = false;
    this.controls.enablePan = false;
    this.controls.minPolarAngle = 0.35;
    this.controls.maxPolarAngle = 1.15;
    this.controls.minAzimuthAngle = -0.6;
    this.controls.maxAzimuthAngle = 0.9;
    this.controls.rotateSpeed = 0.5;
    // Leave vertical one-finger movement to the page on touch screens.
    this.controls.touches.ONE = THREE.TOUCH.PAN;
    this.controls.touches.TWO = THREE.TOUCH.DOLLY_ROTATE;
    canvas.style.touchAction = "pan-y";
    this.controls.addEventListener("change", this.render);
    this.reset();
    this.resizeObserver = new ResizeObserver(() => this.resize());
    this.resizeObserver.observe(canvas);
    canvas.addEventListener("webglcontextlost", this.contextLost);
    canvas.addEventListener("webglcontextrestored", this.contextRestored);
    document.addEventListener("visibilitychange", this.visibility);
    this.reduced.addEventListener("change", this.stopMotion);
    this.resize();
  }

  private contextLost = (event: Event) => {
    event.preventDefault();
    this.lost = true;
    this.stopMotion();
    this.onUnavailable();
  };
  private contextRestored = () => {
    this.lost = false;
    this.canvas.dispatchEvent(new Event("workbenchrestored"));
    this.render();
  };
  private visibility = () => {
    if (document.hidden) this.stopMotion();
    else this.render();
  };
  private resize() {
    const { width, height } = this.canvas.getBoundingClientRect();
    if (!width || !height) return;
    const aspect = width / height;
    const half = Math.max(3.35, 6.4 / aspect);
    this.camera.left = -half * aspect;
    this.camera.right = half * aspect;
    this.camera.top = half;
    this.camera.bottom = -half;
    this.camera.updateProjectionMatrix();
    this.renderer.setSize(width, height, false);
    this.render();
  }
  reset() {
    this.camera.position.set(7.7, 9.8, 12.5);
    this.controls.target.set(0, 0.25, 0);
    this.controls.update();
  }
  private render = () => {
    if (this.alive && !this.lost && !document.hidden)
      this.renderer.render(this.scene, this.camera);
  };
  stopMotion = () => {
    cancelAnimationFrame(this.frame);
    this.frame = 0;
    for (const item of this.moving) item.mesh.position.copy(item.final);
    this.moving = [];
    this.render();
  };

  private disposeGroup(group: THREE.Object3D) {
    group.traverse((object) => {
      if (
        object instanceof THREE.Mesh ||
        object instanceof THREE.Sprite ||
        object instanceof THREE.Line
      ) {
        if ("geometry" in object) object.geometry.dispose();
        const materials = Array.isArray(object.material)
          ? object.material
          : [object.material];
        materials.forEach((material) => {
          if ("map" in material && material.map instanceof THREE.Texture)
            material.map.dispose();
          material.dispose();
        });
      }
    });
  }
  private box(
    w: number,
    h: number,
    d: number,
    color: number,
    x: number,
    y: number,
    z: number,
    parent: THREE.Object3D = this.content,
    bevel = 0.06,
  ) {
    const geometry = bevel
      ? new RoundedBoxGeometry(w, h, d, 2, Math.min(bevel, h / 3, w / 3, d / 3))
      : new THREE.BoxGeometry(w, h, d);
    const material = new THREE.MeshStandardMaterial({ color, roughness: 0.78 });
    const mesh = new THREE.Mesh(geometry, material);
    mesh.position.set(x, y, z);
    mesh.castShadow = true;
    mesh.receiveShadow = true;
    parent.add(mesh);
    return mesh;
  }
  private label(
    text: string,
    x: number,
    y: number,
    z: number,
    width = 2,
    color = C.ink,
    background = "#f8f6eb",
  ) {
    const canvas = document.createElement("canvas");
    canvas.width = 768;
    canvas.height = 132;
    const ctx = canvas.getContext("2d")!;
    ctx.fillStyle = background;
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    ctx.strokeStyle = "#c7cebb";
    ctx.lineWidth = 4;
    ctx.strokeRect(2, 2, 764, 128);
    ctx.font = `600 ${Math.min(76, Math.floor(730 / (text.length * 0.61)))}px monospace`;
    ctx.fillStyle = color;
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.fillText(text, 384, 67);
    const texture = new THREE.CanvasTexture(canvas);
    texture.colorSpace = THREE.SRGBColorSpace;
    const material = new THREE.SpriteMaterial({
      map: texture,
      depthTest: false,
      toneMapped: false,
    });
    const sprite = new THREE.Sprite(material);
    sprite.position.set(x, y, z);
    sprite.scale.set(width, (width * 132) / 768, 1);
    sprite.renderOrder = 5;
    this.content.add(sprite);
    return sprite;
  }
  private line(
    points: THREE.Vector3[],
    color: number,
    radius = 0.028,
    dashed = false,
  ) {
    if (dashed) {
      const geometry = new THREE.BufferGeometry().setFromPoints(points);
      const line = new THREE.Line(
        geometry,
        new THREE.LineDashedMaterial({ color, dashSize: 0.13, gapSize: 0.1 }),
      );
      line.computeLineDistances();
      this.content.add(line);
      return;
    }
    const curve = new THREE.CatmullRomCurve3(points);
    const mesh = new THREE.Mesh(
      new THREE.TubeGeometry(curve, 25, radius, 6, false),
      new THREE.MeshStandardMaterial({ color, roughness: 0.6 }),
    );
    this.content.add(mesh);
  }
  private tray(x: number, width: number, color: number) {
    this.box(width, 0.16, 2.9, color, x, 0.27, 0);
    this.box(width, 0.18, 0.09, color, x, 0.43, -1.42);
    this.box(width, 0.18, 0.09, color, x, 0.43, 1.42);
    this.box(0.09, 0.18, 2.9, color, x - width / 2, 0.43, 0);
    this.box(0.09, 0.18, 2.9, color, x + width / 2, 0.43, 0);
    this.box(0.65, 0.13, 0.1, C.dark, x, 0.39, 1.5);
  }
  private parcel(
    x: number,
    z: number,
    index: number,
    corrupt = false,
    zero = false,
    animate = false,
  ) {
    const group = new THREE.Group();
    group.position.set(x, 0.38, z);
    this.content.add(group);
    this.box(
      0.77,
      0.55,
      0.74,
      zero ? C.cream : corrupt ? C.red : C.orange,
      0,
      0.3,
      0,
      group,
    );
    this.box(0.14, 0.56, 0.755, zero ? C.red : C.tape, 0, 0.3, 0, group, 0.008);
    this.box(0.38, 0.2, 0.008, 0xfff5dc, 0, 0.32, 0.378, group, 0.005);
    for (let n = 0; n < 5; n++)
      this.box(
        0.018 + (n % 2) * 0.012,
        0.12,
        0.011,
        C.dark,
        -0.11 + n * 0.05,
        0.32,
        0.385,
        group,
        0,
      );
    if (zero) this.label("NUL / 00", x, 1.32, z, 1.12, "#9f3e24");
    if (animate)
      this.moving.push({
        mesh: group,
        final: group.position.clone(),
        from: new THREE.Vector3(
          corrupt || zero ? this.writeEdge - 0.65 : x,
          1.8,
          z,
        ),
        delay: index * 75,
      });
    return group;
  }
  private boundary(x: number, bad: boolean) {
    for (let z = -1.7; z < 1.75; z += 0.32)
      this.box(
        0.045,
        0.015,
        0.17,
        bad ? C.red : C.wood,
        x,
        0.38,
        z,
        this.content,
        0,
      );
    const markerZ = x < -1 ? 1.55 : -1.75;
    this.line(
      [new THREE.Vector3(x, 0.4, markerZ), new THREE.Vector3(x, 1.8, markerZ)],
      bad ? C.red : C.wood,
      0.02,
      true,
    );
    this.label(
      "BUFFER ENDS",
      x,
      2.02,
      markerZ,
      1.85,
      bad ? "#a83e25" : "#776347",
    );
    if (bad) {
      this.line(
        [
          new THREE.Vector3(x - 0.7, 1.25, 0.2),
          new THREE.Vector3(x, 1.75, 0.2),
          new THREE.Vector3(x + 0.7, 1.25, 0.2),
        ],
        C.red,
        0.037,
      );
      const head = new THREE.Mesh(
        new THREE.ConeGeometry(0.12, 0.3, 3),
        new THREE.MeshStandardMaterial({ color: C.red }),
      );
      head.position.set(x + 0.74, 1.2, 0.2);
      head.rotation.z = -2.25;
      this.content.add(head);
    }
  }

  show(lesson: Lesson, step: number, animate = true) {
    this.stopMotion();
    this.generation++;
    this.disposeGroup(this.content);
    this.content.clear();
    const bad = step >= 2;
    const consequence = step === 3;
    const small = lesson.id === "size-wrap";
    // A miniature industrial workbench: feet, laminated top, metal rail and scale.
    this.box(10.8, 0.28, 5.5, C.base, 0, -0.02, 0);
    this.box(10.85, 0.09, 5.55, C.edge, 0, -0.2, 0);
    for (const x of [-4.55, 4.55])
      for (const z of [-1.9, 1.9]) {
        this.box(0.35, 0.4, 0.35, C.dark, x, -0.4, z);
        const bolt = new THREE.Mesh(
          new THREE.CylinderGeometry(0.075, 0.075, 0.02, 10),
          new THREE.MeshStandardMaterial({
            color: 0x849076,
            metalness: 0.5,
            roughness: 0.4,
          }),
        );
        bolt.position.set(x, 0.135, z > 0 ? 2.45 : -2.45);
        this.content.add(bolt);
      }
    for (let i = 0; i < 35; i++)
      this.box(
        0.016,
        0.01,
        i % 5 ? 0.1 : 0.2,
        0x95a087,
        -4.8 + i * 0.28,
        0.13,
        2.49,
        this.content,
        0,
      );
    this.tray(small ? -3.6 : -2.6, small ? 2.0 : 4.6, C.cream);
    this.tray(2.15, 3.7, C.pale);
    const edge = small ? -2.5 : -0.15;
    this.writeEdge = edge;
    this.boundary(edge, bad);
    this.label(
      small ? "BUFFER / 32 B" : "BUFFER / 8 B",
      small ? -3.65 : -2.9,
      0.52,
      2,
      2.3,
    );
    this.label(lesson.targetLabel, 2.25, 0.52, 2, 2.3, "#355d49", "#e7ecdf");
    if (step === 0 && !small) {
      for (let i = 0; i < 3; i++) {
        const pending = this.parcel(-3.9 + i * 0.85, -2.1, i);
        pending.position.y = 0.13;
      }
      this.label("INCOMING DATA", -3.0, 1.2, -2.2, 2.1, "#915331");
    }
    // Empty inset marks make capacity visible before any input arrives.
    const slots = small ? 2 : 8;
    for (let i = 0; i < slots; i++) {
      const x = small ? -3.6 : -4.12 + (i % 4) * 1.03;
      const z = i < (small ? 1 : 4) ? -0.59 : 0.59;
      this.box(0.83, 0.025, 0.8, 0xd4c8ab, x, 0.37, z, this.content, 0.04);
      if (step >= 1 && !(small && step === 1))
        this.parcel(x, z, i, false, false, animate && step === 1);
    }
    if (lesson.id === "neighbor") {
      for (let i = 0; i < 4; i++) {
        const x = 1.15 + (i % 2) * 1.4,
          z = i < 2 ? -0.6 : 0.6;
        this.box(1.1, 0.15, 0.8, C.teal, x, 0.46, z);
        this.box(0.65, 0.025, 0.08, 0xd9e4cc, x, 0.55, z + 0.12);
        if (bad) this.parcel(x, z, i, true, false, animate);
      }
      this.label(
        consequence ? "ALLOW / CORRUPTED" : "ACCESS: DENY",
        2.15,
        1.7,
        -1.15,
        2.6,
        consequence ? "#a03921" : "#355d49",
      );
    } else if (lesson.id === "callback") {
      // Two rails, one stored selector: an indirect call, not injected code.
      for (const side of [-1, 1]) {
        const target = new THREE.Vector3(3.5, 0.54, side * 0.83);
        this.line(
          [
            new THREE.Vector3(0.7, 0.54, 0),
            new THREE.Vector3(1.7, 0.54, 0),
            target,
          ],
          side === (bad ? 1 : -1) ? (bad ? C.red : C.teal) : 0xa1b29b,
          0.06,
        );
        this.box(
          0.85,
          0.5,
          0.55,
          side === 1 ? 0xa87e69 : C.teal,
          3.5,
          0.72,
          side * 0.83,
        );
      }
      this.box(0.55, 0.18, 0.65, C.dark, 1.1, 0.52, 0);
      const lever = this.box(
        0.13,
        0.85,
        0.13,
        bad ? C.red : C.teal,
        1.1,
        0.95,
        0,
      );
      lever.rotation.x = bad ? -0.65 : 0.65;
      this.label(
        bad ? "ADDRESS REPLACED" : "STORED ADDRESS",
        1.8,
        1.95,
        -1.3,
        2.65,
        bad ? "#a33e23" : "#355d49",
      );
      if (consequence) this.parcel(3.45, 0.83, 0, true, false, animate);
      else if (bad) this.parcel(0.6, 0.55, 0, true, false, animate);
      this.label("INTENDED", 3.6, 1.2, -1, 1.35);
      this.label("SUBSTITUTED", 3.55, 1.2, 1.18, 1.65, "#9f4930");
    } else if (lesson.id === "metadata") {
      const board = new THREE.Group();
      board.position.set(1.25, 0.4, -0.15);
      board.rotation.x = -0.35;
      this.content.add(board);
      this.box(1.35, 1.65, 0.12, C.wood, 0, 0.7, 0, board);
      this.box(1.1, 1.35, 0.025, 0xfff6dd, 0, 0.68, 0.08, board);
      this.box(0.5, 0.18, 0.07, C.dark, 0, 1.46, 0.1, board);
      for (let i = 0; i < 4; i++)
        this.box(
          i === 1 && bad ? 0.94 : 0.7,
          0.07,
          0.025,
          i === 1 && bad ? C.red : C.teal,
          0,
          1.03 - i * 0.25,
          0.105,
          board,
          0,
        );
      for (let i = 0; i < 3; i++)
        this.box(1.15, 0.22, 0.65, C.teal, 3.3, 0.5 + i * 0.23, -0.6);
      this.line(
        [
          new THREE.Vector3(1.3, 0.48, 0.6),
          new THREE.Vector3(2.0, 0.48, 0.8),
          new THREE.Vector3(consequence ? 2.4 : 3.3, 0.48, -0.2),
        ],
        bad ? C.red : C.teal,
        0.04,
      );
      if (bad) this.parcel(0.5, 0.65, 0, true, false, animate);
      if (consequence) this.box(2.4, 0.07, 0.9, C.red, 2.45, 0.39, 1.0);
      this.label(
        consequence
          ? "FALSE CHUNK BOUNDARY"
          : bad
            ? "RECORD OVERWRITTEN"
            : "SIZE / NEXT CHUNK",
        2.4,
        2.25,
        -0.8,
        2.9,
        bad ? "#a33e23" : "#355d49",
      );
    } else if (lesson.id === "off-by-one") {
      this.box(1.25, 0.25, 1.15, C.teal, 1.2, 0.5, 0);
      const lock = new THREE.Mesh(
        new THREE.TorusGeometry(0.32, 0.08, 8, 24, Math.PI),
        new THREE.MeshStandardMaterial({
          color: bad ? C.red : C.dark,
          metalness: 0.5,
          roughness: 0.4,
        }),
      );
      lock.position.set(bad ? 1.42 : 1.2, 0.96, 0);
      lock.rotation.z = bad ? -0.7 : 0;
      this.content.add(lock);
      this.box(0.6, 0.5, 0.35, bad ? C.red : C.dark, 1.2, 0.75, 0.04);
      if (bad) this.parcel(0.45, 0.8, 0, false, true, animate);
      this.label(
        bad ? "00 / UNLOCKED" : "01 / LOCKED",
        2.3,
        1.9,
        -0.9,
        2.3,
        bad ? "#a33e23" : "#355d49",
      );
      // Spare neutral space emphasizes that only one byte changes.
      for (let i = 0; i < 3; i++)
        this.box(0.25, 0.4, 0.8, C.pale, 2.5 + 0.4 * i, 0.55, 0.2);
    } else {
      // A mechanical order counter above a conspicuously smaller tray.
      this.box(2.2, 0.85, 0.5, C.dark, -2.9, 1.2, -1.4);
      for (let i = 0; i < 3; i++)
        this.box(0.55, 0.62, 0.04, C.cream, -3.6 + i * 0.7, 1.2, -1.1);
      this.label(
        step === 0 ? "288 NEEDED" : "032 ALLOCATED",
        -2.9,
        1.3,
        -1.0,
        2.15,
      );
      this.label("8-BIT SIZE COUNTER", -3.0, 2.15, -1.6, 2.5);
      for (let i = 0; i < 4; i++)
        this.box(
          1.0,
          0.2,
          0.8,
          C.teal,
          1.3 + (i % 2) * 1.3,
          0.47,
          i < 2 ? -0.6 : 0.6,
        );
      if (bad)
        for (let i = 0; i < 8; i++)
          this.parcel(
            -1.7 + (i % 4) * 1.05,
            i < 4 ? -0.6 : 0.6,
            i,
            true,
            false,
            animate,
          );
      this.label(
        consequence ? "NEIGHBOR CORRUPTED" : "18 × 16 = 288",
        2.1,
        1.9,
        -1.5,
        2.7,
        consequence ? "#a33e23" : "#355d49",
      );
    }
    this.render();
    if (
      animate &&
      !this.reduced.matches &&
      !document.hidden &&
      !this.lost &&
      this.moving.length
    ) {
      const start = performance.now();
      const generation = this.generation;
      const tick = (now: number) => {
        if (!this.alive || generation !== this.generation) return;
        let complete = true;
        for (const item of this.moving) {
          const t = Math.min(1, Math.max(0, (now - start - item.delay) / 650));
          item.mesh.position.lerpVectors(
            item.from,
            item.final,
            1 - (1 - t) ** 3,
          );
          if (t < 1) complete = false;
        }
        this.render();
        if (!complete) this.frame = requestAnimationFrame(tick);
        else {
          this.frame = 0;
          this.moving = [];
        }
      };
      this.frame = requestAnimationFrame(tick);
    } else this.stopMotion();
  }
  dispose() {
    this.stopMotion();
    this.alive = false;
    this.resizeObserver.disconnect();
    this.controls.dispose();
    this.canvas.removeEventListener("webglcontextlost", this.contextLost);
    this.canvas.removeEventListener(
      "webglcontextrestored",
      this.contextRestored,
    );
    document.removeEventListener("visibilitychange", this.visibility);
    this.reduced.removeEventListener("change", this.stopMotion);
    this.disposeGroup(this.scene);
    this.renderer.dispose();
  }
}
