/**
 * Ambient field behind the empty-state face.
 *
 * The empty chat is the one screen with no primary action, so its background
 * carries the whole first impression. This one is made of *generations* instead
 * of dots. A layer is born at the face every 11 seconds, grows from the core to
 * the rim over 66 seconds and dies at the edge, so six layers are always in
 * flight at staggered sizes — growth rings laid down by the thing in the middle.
 *
 * A generation does not arrive finished. It is born carrying only its outline
 * and a growth trace, then assembles itself as it lives: its nodes arrive one by
 * one, each one dragging in the chords that join it to the ones already there,
 * and the last node of the lot is the accent-coloured one — the variant that
 * generation picked up. Once the layer has settled it keeps that accent dot as
 * its marker, so the stack of rings reads as a line of generations that differ
 * from one another. Between two births the genome is re-rolled, so no two layers
 * are ever the same picture: the field is not looping, it is inheriting with a
 * variation.
 *
 * Two inks do the work: structure is the neutral ring ink, and the accent marks
 * only what has just evolved — a layer is born accent-coloured and settles to
 * neutral as it grows, and the accent dot stays for life. Colour therefore means
 * "this is new" rather than "decoration".
 *
 * Nothing is measured and nothing animates per frame in JavaScript. Growth,
 * fade, settle and arrival are CSS keyframes on the layer group; JavaScript only
 * reacts to `animationiteration`, which fires at the instant a layer restarts —
 * invisible then, at opacity 0 — so a genome can never be seen swapping in.
 *
 * Under `prefers-reduced-motion: reduce` the loops stop and the six layers rest
 * at the fixed scale/opacity each carries inline: a still stack of growth rings
 * with every node in place, not six ellipses piled on one another.
 */
import { useCallback, useState } from "react";
import type { CSSProperties } from "react";
import "./AmbientField.css";

const TAU = Math.PI * 2;
/** The layer's box in viewBox units; the face's centre is (0, 0) of the field. */
const VIEW_W = 560;
const VIEW_H = 280;

/** Full-size radius of the outermost layer, and the scale a layer is born at. */
const RING_RX = 264;
const RING_RY = 100;
const BORN_SCALE = 0.18;

/** One life: born at the core, settled at the rim, gone. Six lives overlap. */
const LAYER_LIFE = 66;
const LAYER_SLOTS = 6;
/** Seconds between two births — the beat the whole composition shares. */
const BIRTH_GAP = LAYER_LIFE / LAYER_SLOTS;

const round = (value: number) => Math.round(value * 10) / 10;

const fract = (value: number) => value - Math.floor(value);

/** Deterministic 0..1 from two integers — a genome must render identically. */
const hash = (a: number, b: number) => fract(Math.sin(a * 127.1 + b * 311.7) * 43758.5453);

/** The scale a layer sits at when its loop is frozen (reduced motion). */
const restingScale = (slot: number) =>
  round(BORN_SCALE + (1 - BORN_SCALE) * ((slot + 1) / LAYER_SLOTS));

/** Where in the layer's life a node of rank `rank` arrives, as a fraction. */
const arrivalAt = (rank: number) => 0.14 + rank * 0.18;

/**
 * A generation's growth trace: a little over one turn of the same flat ellipse,
 * from near the core out to the rim. It scales with its layer, so the trace
 * reads as the path that generation took on its way out.
 */
const tracePath = (seed: number) => {
  const start = (seed % 12) * (TAU / 12);
  const steps = 40;
  let d = "";
  for (let step = 0; step <= steps; step += 1) {
    const t = step / steps;
    const angle = start + t * 1.15 * TAU;
    const reach = 0.14 + 0.8 * t;
    d += `${step === 0 ? "M" : "L"}${round(Math.cos(angle) * RING_RX * reach)} ${round(
      Math.sin(angle) * RING_RY * reach,
    )}`;
  }
  return d;
};

interface GenerationNode {
  x: number;
  y: number;
  r: number;
  accent: boolean;
  /** Fraction of the layer's life at which this node arrives. */
  at: number;
}

interface GenerationChord {
  x1: number;
  y1: number;
  x2: number;
  y2: number;
  at: number;
}

interface Generation {
  nodes: GenerationNode[];
  chords: GenerationChord[];
  /** Only every other layer carries a trace, so the field never looks spun. */
  trace: string;
}

/**
 * One genome in, one generation out. Node count and seats come from the seed, so
 * mutating the seed is a visible mutation of the layer — which is the point. The
 * last node of the rank is always the accent one: the variant that generation
 * acquired, and the last thing to land.
 */
const generationFor = (slot: number, seed: number): Generation => {
  const count = 2 + (seed % 3);
  const nodes = Array.from({ length: count }, (_, rank) => {
    const angle = hash(seed, rank + 1) * TAU;
    const reach = 0.58 + hash(seed, rank + 5) * 0.34;
    return {
      x: round(Math.cos(angle) * RING_RX * reach),
      y: round(Math.sin(angle) * RING_RY * reach),
      r: rank === count - 1 ? 2 : 1.3,
      accent: rank === count - 1,
      at: arrivalAt(rank),
    };
  });

  // The outline through the nodes in order. Two nodes make one chord; three or
  // more close into a small constellation. A chord arrives with the later of its
  // two ends, so the web grows link by link instead of appearing whole.
  const pairs = count === 2 ? [[0, 1]] : nodes.map((_, index) => [index, (index + 1) % count]);
  const chords = pairs.map(([from, to]) => ({
    x1: nodes[from].x,
    y1: nodes[from].y,
    x2: nodes[to].x,
    y2: nodes[to].y,
    at: Math.max(nodes[from].at, nodes[to].at),
  }));

  return { nodes, chords, trace: slot % 2 === 0 ? tracePath(seed) : "" };
};

/** Seeded once, then re-rolled by the layers themselves as they are reborn. */
const INITIAL_SEEDS = [5, 23, 11, 37, 2, 29];

/** Linear congruential step — small, deterministic, no `Math.random`. */
const mutate = (seed: number) => (seed * 1664525 + 1013904223) % 4294967296;

export function AmbientField() {
  const [seeds, setSeeds] = useState<number[]>(INITIAL_SEEDS);

  // A layer fires this the moment it wraps round to its first keyframe — born
  // again at opacity 0, so the new genome is never seen swapping in.
  const reborn = useCallback((slot: number) => {
    setSeeds((current) =>
      current.map((seed, index) => (index === slot ? mutate(seed) : seed)),
    );
  }, []);

  return (
    <div className="ambient-field" aria-hidden="true">
      {/* The viewBox is anchored at 0 0 on purpose. This WebView resolves
          `transform-origin: center` against the view-box size while ignoring
          the view-box origin, so a centred viewBox would make every layer scale
          about the bottom-right corner. With the origin at 0 0 both readings
          agree, and the content is translated onto the face instead. */}
      <svg viewBox={`0 0 ${VIEW_W} ${VIEW_H}`} focusable="false">
        <g transform={`translate(${VIEW_W / 2} ${VIEW_H / 2})`}>
          {seeds.map((seed, slot) => {
            const generation = generationFor(slot, seed);
            return (
              <g
                key={`layer-${slot}`}
                className="ambient-layer"
                style={
                  {
                    "--ambient-born": slot === 0 ? "0s" : `-${slot * BIRTH_GAP}s`,
                    "--ambient-rest-scale": restingScale(slot),
                    "--ambient-rest-opacity": 0.24 + 0.1 * slot,
                  } as CSSProperties
                }
                onAnimationIteration={() => reborn(slot)}
              >
                <ellipse
                  className={`ambient-ring${slot % 2 === 1 ? " ambient-ring--dotted" : ""}`}
                  rx={RING_RX}
                  ry={RING_RY}
                />
                {generation.trace && <path className="ambient-trace" d={generation.trace} />}
                {generation.chords.map((chord, index) => (
                  <line
                    key={`chord-${index}`}
                    className="ambient-chord"
                    x1={chord.x1}
                    y1={chord.y1}
                    x2={chord.x2}
                    y2={chord.y2}
                    style={{ "--ambient-arrive": `${round(chord.at * LAYER_LIFE)}s` } as CSSProperties}
                  />
                ))}
                {generation.nodes.map((node, index) => (
                  <circle
                    key={`node-${index}`}
                    className={`ambient-node${node.accent ? " ambient-node--accent" : ""}`}
                    cx={node.x}
                    cy={node.y}
                    r={node.r}
                    style={{ "--ambient-arrive": `${round(node.at * LAYER_LIFE)}s` } as CSSProperties}
                  />
                ))}
              </g>
            );
          })}
        </g>
      </svg>
    </div>
  );
}
