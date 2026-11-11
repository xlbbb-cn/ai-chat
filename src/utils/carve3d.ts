/**
 * `<carve>` — declarative 3D scenes drawn with three.js.
 *
 * The scene is JSON, not code: a model that could hand us JavaScript would
 * turn every reply into a script host, which is exactly what `html: false`
 * exists to prevent. A fixed vocabulary (box / sphere / cylinder / cone /
 * torus / plane / line, placed by `at` and `rot`) covers the diagrams a chat
 * actually needs, and every colour is a token name so the drawing follows the
 * active theme.
 *
 * Rendering uses **one** shared `WebGLRenderer`. A `WebGLRenderer` per figure
 * would run into the browser's WebGL-context limit (about 16) as soon as a
 * conversation holds a few scenes, and each rebuild of the transcript would
 * leak another context. Instead the shared off-screen renderer draws each
 * scene into the figure's own 2D canvas: unlimited figures, one context, and
 * the canvas is already a PNG source for the save button.
 */

import type { WebGLRenderer } from "three";

export interface CarveObject {
  type: string;
  at?: number[];
  rot?: number[];
  size?: number[];
  r?: number;
  h?: number;
  tube?: number;
  color?: string;
  opacity?: number;
  wireframe?: boolean;
  points?: number[][];
}

export interface CarveScene {
  title?: string;
  name?: string;
  camera?: { yaw?: number; pitch?: number; zoom?: number; fov?: number };
  objects?: CarveObject[];
  grid?: boolean;
  axes?: boolean;
}

export interface CarveView {
  /** Degrees around the Y axis. */
  yaw: number;
  /** Degrees above the XZ plane, clamped to ±89. */
  pitch: number;
  zoom: number;
}

/** Objects past this count are dropped: a scene, not a point cloud. */
const MAX_OBJECTS = 200;

const DEFAULT_VIEW: CarveView = { yaw: 35, pitch: 22, zoom: 1 };

function finite(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

function tuple(value: unknown, fallback: [number, number, number]): [number, number, number] {
  if (!Array.isArray(value) || value.length < 3) return fallback;
  return [finite(value[0], fallback[0]), finite(value[1], fallback[1]), finite(value[2], fallback[2])];
}

/** Parse a `<carve>` body; `null` when it is not a scene at all. */
export function parseCarve(source: string): CarveScene | null {
  try {
    const raw = JSON.parse(source) as CarveScene;
    if (!raw || typeof raw !== "object") return null;
    const objects = Array.isArray(raw.objects)
      ? raw.objects.filter((object) => object && typeof object.type === "string").slice(0, MAX_OBJECTS)
      : [];
    if (objects.length === 0) return null;
    return { ...raw, objects };
  } catch {
    return null;
  }
}

export function defaultView(scene: CarveScene): CarveView {
  const camera = scene.camera ?? {};
  return {
    yaw: finite(camera.yaw, DEFAULT_VIEW.yaw),
    pitch: finite(camera.pitch, DEFAULT_VIEW.pitch),
    zoom: Math.min(8, Math.max(0.2, finite(camera.zoom, DEFAULT_VIEW.zoom))),
  };
}

type ThreeModule = typeof import("three");

let rendererPromise: Promise<{ three: ThreeModule; renderer: WebGLRenderer } | null> | null = null;

/**
 * The shared renderer, created on first use and imported lazily so a
 * conversation without a 3D figure never downloads three.js.
 *
 * Returns `null` when WebGL is unavailable (GPU-disabled WebView2, remote
 * session): the card then explains itself instead of throwing on every token.
 */
function glRenderer(): Promise<{ three: ThreeModule; renderer: WebGLRenderer } | null> {
  rendererPromise ??= import("three")
    .then((three) => {
      try {
        const renderer = new three.WebGLRenderer({
          antialias: true,
          alpha: true,
          // The canvas is read back with `drawImage` for every frame and for
          // the saved PNG, in the same task as the render.
          preserveDrawingBuffer: true,
        });
        renderer.setPixelRatio(1);
        renderer.outputColorSpace = three.SRGBColorSpace;
        renderer.toneMapping = three.NoToneMapping;
        return { three, renderer };
      } catch {
        return null;
      }
    })
    .catch(() => null);
  return rendererPromise;
}

/** Resolved theme colours, read from the live element so the scene follows it. */
export interface CarvePalette {
  background: string;
  accent: string;
  success: string;
  warning: string;
  danger: string;
  text: string;
  muted: string;
}

const NAMED_COLORS = ["accent", "success", "warning", "danger", "text", "muted"] as const;

function colorOf(palette: CarvePalette, name: string | undefined): string {
  if (name && (NAMED_COLORS as readonly string[]).includes(name)) {
    return palette[name as (typeof NAMED_COLORS)[number]];
  }
  return palette.accent;
}

/**
 * Draw one scene into `target` (a 2D canvas the figure owns).
 *
 * `target.width/height` are the backing-store pixels, already scaled for the
 * display; the shared renderer is resized to match, so the blit is 1:1.
 */
export async function renderCarve(
  target: HTMLCanvasElement,
  scene: CarveScene,
  view: CarveView,
  palette: CarvePalette,
): Promise<boolean> {
  const ctx = target.getContext("2d");
  if (!ctx) return false;
  const ready = await glRenderer();
  if (!ready) return false;
  const { three, renderer } = ready;

  const width = Math.max(1, target.width);
  const height = Math.max(1, target.height);
  if (width < 2 || height < 2) return false;

  const group = new three.Group();
  const disposables: Array<{ dispose: () => void }> = [];

  const materialFor = (object: CarveObject) => {
    const material = new three.MeshLambertMaterial({
      color: new three.Color(colorOf(palette, object.color)),
      transparent: (object.opacity ?? 1) < 1,
      opacity: Math.min(1, Math.max(0.05, object.opacity ?? 1)),
      wireframe: object.wireframe === true,
      side: three.DoubleSide,
    });
    disposables.push(material);
    return material;
  };

  const place = (
    mesh: import("three").Object3D,
    object: CarveObject,
  ) => {
    const at = tuple(object.at, [0, 0, 0]);
    mesh.position.set(at[0], at[1], at[2]);
    // Degrees: a model writes `"rot": [0, 45, 0]`, not radians.
    const rot = tuple(object.rot, [0, 0, 0]).map((angle) => (angle * Math.PI) / 180);
    mesh.rotation.set(rot[0], rot[1], rot[2]);
    group.add(mesh);
  };

  for (const object of scene.objects ?? []) {
    switch (object.type) {
      case "box": {
        const size = tuple(object.size, [1, 1, 1]).map((value) => Math.max(0.001, Math.abs(value)));
        const geometry = new three.BoxGeometry(size[0], size[1], size[2]);
        disposables.push(geometry);
        place(new three.Mesh(geometry, materialFor(object)), object);
        break;
      }
      case "sphere": {
        const radius = Math.max(0.001, Math.abs(finite(object.r, 0.5)));
        const geometry = new three.SphereGeometry(radius, 28, 18);
        disposables.push(geometry);
        place(new three.Mesh(geometry, materialFor(object)), object);
        break;
      }
      case "cylinder": {
        const radius = Math.max(0.001, Math.abs(finite(object.r, 0.5)));
        const height = Math.max(0.001, Math.abs(finite(object.h, 1)));
        const geometry = new three.CylinderGeometry(radius, radius, height, 32);
        disposables.push(geometry);
        place(new three.Mesh(geometry, materialFor(object)), object);
        break;
      }
      case "cone": {
        const radius = Math.max(0.001, Math.abs(finite(object.r, 0.5)));
        const height = Math.max(0.001, Math.abs(finite(object.h, 1)));
        const geometry = new three.ConeGeometry(radius, height, 32);
        disposables.push(geometry);
        place(new three.Mesh(geometry, materialFor(object)), object);
        break;
      }
      case "torus": {
        const radius = Math.max(0.01, Math.abs(finite(object.r, 0.5)));
        const tube = Math.max(0.001, Math.abs(finite(object.tube, 0.15)));
        const geometry = new three.TorusGeometry(radius, tube, 16, 64);
        disposables.push(geometry);
        place(new three.Mesh(geometry, materialFor(object)), object);
        break;
      }
      case "plane": {
        const size = tuple(object.size, [4, 4, 0]);
        const geometry = new three.PlaneGeometry(
          Math.max(0.001, Math.abs(size[0])),
          Math.max(0.001, Math.abs(size[1])),
        );
        disposables.push(geometry);
        const mesh = new three.Mesh(geometry, materialFor(object));
        // A ground plane is what a `plane` is for; the default XY orientation
        // would stand it up like a wall.
        const rot = tuple(object.rot, [-90, 0, 0]);
        place(mesh, { ...object, rot });
        break;
      }
      case "line": {
        const points = (object.points ?? [])
          .filter((point) => Array.isArray(point) && point.length >= 3)
          .slice(0, 2000)
          .map((point) => new three.Vector3(...tuple(point, [0, 0, 0])));
        if (points.length < 2) break;
        const geometry = new three.BufferGeometry().setFromPoints(points);
        const material = new three.LineBasicMaterial({
          color: new three.Color(colorOf(palette, object.color)),
          transparent: true,
          opacity: Math.min(1, Math.max(0.05, object.opacity ?? 1)),
        });
        disposables.push(geometry, material);
        place(new three.Line(geometry, material), object);
        break;
      }
      default:
        break;
    }
  }

  if (group.children.length === 0) {
    for (const item of disposables) item.dispose();
    return false;
  }

  const box = new three.Box3().setFromObject(group);
  if (!box.isEmpty()) {
    const center = box.getCenter(new three.Vector3());
    const radius = Math.max(0.25, box.getSize(new three.Vector3()).length() / 2);
    // Re-centre on the origin so orbiting turns around the content, not a
    // point the model happened to place things at.
    group.position.sub(center);

    if (scene.grid) {
      const divisions = 10;
      const size = Math.max(2, Math.ceil(radius * 2.6));
      const grid = new three.GridHelper(
        size,
        divisions,
        new three.Color(palette.muted),
        new three.Color(palette.muted),
      );
      const gridMaterial = grid.material as import("three").Material;
      gridMaterial.transparent = true;
      gridMaterial.opacity = 0.22;
      grid.position.y = -radius;
      disposables.push(grid.geometry, gridMaterial);
      group.add(grid);
    }
    if (scene.axes) {
      const length = radius * 1.3;
      const axis = (direction: [number, number, number], color: string) => {
        const geometry = new three.BufferGeometry().setFromPoints([
          new three.Vector3(0, 0, 0),
          new three.Vector3(...direction).multiplyScalar(length),
        ]);
        const material = new three.LineBasicMaterial({
          color: new three.Color(color),
          transparent: true,
          opacity: 0.75,
        });
        disposables.push(geometry, material);
        group.add(new three.Line(geometry, material));
      };
      axis([1, 0, 0], palette.danger);
      axis([0, 1, 0], palette.success);
      axis([0, 0, 1], palette.accent);
    }

    const fov = Math.min(90, Math.max(15, finite(scene.camera?.fov, 35)));
    const camera = new three.PerspectiveCamera(fov, width / height, 0.01, radius * 200);
    const distance = (radius / Math.sin(((fov / 2) * Math.PI) / 180)) * 1.15 / view.zoom;
    const yaw = (view.yaw * Math.PI) / 180;
    const pitch = (Math.min(89, Math.max(-89, view.pitch)) * Math.PI) / 180;
    camera.position.set(
      distance * Math.cos(pitch) * Math.sin(yaw),
      distance * Math.sin(pitch),
      distance * Math.cos(pitch) * Math.cos(yaw),
    );
    camera.lookAt(0, 0, 0);

    const scene3 = new three.Scene();
    scene3.add(new three.AmbientLight(0xffffff, 2.2));
    const key = new three.DirectionalLight(0xffffff, 1.7);
    key.position.set(radius * 2, radius * 3, radius * 2.4);
    scene3.add(key);
    const fill = new three.DirectionalLight(0xffffff, 0.7);
    fill.position.set(-radius * 2.4, radius * 0.6, -radius * 2);
    scene3.add(fill);
    scene3.add(group);

    try {
      renderer.setSize(width, height, false);
      renderer.setClearColor(0x000000, 0);
      renderer.render(scene3, camera);
      ctx.clearRect(0, 0, width, height);
      ctx.fillStyle = palette.background;
      ctx.fillRect(0, 0, width, height);
      ctx.drawImage(renderer.domElement, 0, 0, width, height);
    } finally {
      for (const item of disposables) item.dispose();
    }
    return true;
  }

  for (const item of disposables) item.dispose();
  return false;
}