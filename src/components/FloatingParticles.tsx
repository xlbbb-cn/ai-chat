/**
 * Ambient particles behind the empty-state face.
 *
 * The dots are placed once with a golden-angle spiral, then each one drifts
 * on a slow damped random walk: a very small random acceleration is fed into
 * a high-inertia velocity, so the path bends in long, gentle arcs instead of
 * turning sharply. When two dots come within `LINK_DISTANCE`, a faint hairline
 * connects them; the links are recomputed from the same positions, so they
 * always end on their dots.
 *
 * Motion is decorative, not a state transition, so there is no easing or
 * period token. Colour comes from particle tokens derived from the palette's
 * muted ink — the accent is never used for decoration. Under
 * `prefers-reduced-motion` the loop is stopped and the field stays still.
 */
import { useEffect, useRef } from "react";
import "./FloatingParticles.css";

const PARTICLE_COUNT = 48;
/** Half-size of the SVG viewBox; the field fits inside with room to wander. */
const VIEW_HALF_X = 280;
const VIEW_HALF_Y = 140;
/** Bounds of the wander, around the face's centre. */
const FIELD_X = 232;
const FIELD_Y = 104;
/** Two dots closer than this (viewBox units) get a link. */
const LINK_DISTANCE = 58;
/** Line pool — large enough that every nearby pair gets its own segment. */
const MAX_LINES = 180;
/** Golden angle — even coverage without a visible lattice. */
const GOLDEN_ANGLE = 2.399963229728653;

/** Tiny impulse per 60Hz frame — the path bends, it does not turn. */
const WANDER_ACCEL = 0.07;
/** Strong inertia: a direction persists for about a second, hence the drift. */
const WANDER_DAMPING = 0.985;
/** Upper bound on travel per frame; keeps the whole field calm. */
const MAX_SPEED = 0.28;
/** One 60Hz frame; long gaps (a hidden tab) are capped rather than replayed. */
const FRAME_MS = 1000 / 60;

interface Particle {
  x: number;
  y: number;
  alpha: number;
  radius: number;
}

interface Walker {
  x: number;
  y: number;
  vx: number;
  vy: number;
}

interface Point {
  x: number;
  y: number;
}

interface Link {
  from: number;
  to: number;
  distance: number;
}

const round = (value: number) => Math.round(value * 10) / 10;

const PARTICLES: Particle[] = Array.from({ length: PARTICLE_COUNT }, (_, index) => {
  // Square-root radius spreads an equal number of dots into each ring of the
  // spiral, so the field is even rather than crowded at the centre.
  const radius = Math.sqrt((index + 0.5) / PARTICLE_COUNT);
  const angle = index * GOLDEN_ANGLE;
  return {
    x: round(Math.cos(angle) * radius * FIELD_X),
    y: round(Math.sin(angle) * radius * FIELD_Y),
    alpha: round(0.55 + ((index * 5) % 7) / 20),
    radius: index % 6 === 0 ? 1.5 : 1,
  };
});

/** Reflect a dot back inside the field instead of letting it drift off-screen. */
function bounce(value: number, velocity: number, limit: number): [number, number] {
  if (value < -limit) return [-limit, Math.abs(velocity) * 0.5];
  if (value > limit) return [limit, -Math.abs(velocity) * 0.5];
  return [value, velocity];
}

function clampVelocity(velocity: number): number {
  return Math.max(-MAX_SPEED, Math.min(MAX_SPEED, velocity));
}

export function FloatingParticles() {
  const dotRefs = useRef<Array<SVGCircleElement | null>>([]);
  const lineRefs = useRef<Array<SVGLineElement | null>>([]);

  useEffect(() => {
    const motion = window.matchMedia("(prefers-reduced-motion: reduce)");
    const walkers: Walker[] = PARTICLES.map((particle) => ({
      x: particle.x,
      y: particle.y,
      vx: 0,
      vy: 0,
    }));
    let frame = 0;
    let lastTime = 0;
    let disposed = false;

    const paint = (points: Point[]) => {
      for (let index = 0; index < PARTICLES.length; index += 1) {
        const dot = dotRefs.current[index];
        if (!dot) continue;
        dot.setAttribute("cx", String(points[index].x));
        dot.setAttribute("cy", String(points[index].y));
      }

      const links: Link[] = [];
      const maxDistanceSq = LINK_DISTANCE * LINK_DISTANCE;
      for (let i = 0; i < points.length; i += 1) {
        for (let j = i + 1; j < points.length; j += 1) {
          const dx = points[i].x - points[j].x;
          const dy = points[i].y - points[j].y;
          const distanceSq = dx * dx + dy * dy;
          if (distanceSq <= maxDistanceSq) {
            links.push({ from: i, to: j, distance: Math.sqrt(distanceSq) });
          }
        }
      }
      // The closest pairs win; a long tail of barely-visible lines would read
      // as a mesh rather than as a few quiet connections.
      links.sort((a, b) => a.distance - b.distance);

      for (let index = 0; index < MAX_LINES; index += 1) {
        const line = lineRefs.current[index];
        if (!line) continue;
        const link = links[index];
        if (!link) {
          line.setAttribute("stroke-opacity", "0");
          continue;
        }
        const from = points[link.from];
        const to = points[link.to];
        line.setAttribute("x1", String(from.x));
        line.setAttribute("y1", String(from.y));
        line.setAttribute("x2", String(to.x));
        line.setAttribute("y2", String(to.y));
        // Fade with distance so a link appears as the dots approach it.
        line.setAttribute(
          "stroke-opacity",
          String(round(1 - link.distance / LINK_DISTANCE)),
        );
      }
    };

    const animate = (time: number) => {
      if (disposed) return;
      const step = lastTime === 0 ? 1 : Math.min(2, (time - lastTime) / FRAME_MS);
      lastTime = time;

      for (const walker of walkers) {
        walker.vx = clampVelocity(
          walker.vx * WANDER_DAMPING + (Math.random() * 2 - 1) * WANDER_ACCEL,
        );
        walker.vy = clampVelocity(
          walker.vy * WANDER_DAMPING + (Math.random() * 2 - 1) * WANDER_ACCEL,
        );
        walker.x += walker.vx * step;
        walker.y += walker.vy * step;
        [walker.x, walker.vx] = bounce(walker.x, walker.vx, FIELD_X);
        [walker.y, walker.vy] = bounce(walker.y, walker.vy, FIELD_Y);
      }

      paint(walkers);
      frame = window.requestAnimationFrame(animate);
    };

    const sync = () => {
      window.cancelAnimationFrame(frame);
      if (motion.matches) {
        // A fixed frame keeps the field and its links visible but still.
        paint(PARTICLES);
        return;
      }
      lastTime = 0;
      frame = window.requestAnimationFrame(animate);
    };

    sync();
    motion.addEventListener("change", sync);
    return () => {
      disposed = true;
      window.cancelAnimationFrame(frame);
      motion.removeEventListener("change", sync);
    };
  }, []);

  return (
    <div className="floating-particles" aria-hidden="true">
      <svg
        viewBox={`${-VIEW_HALF_X} ${-VIEW_HALF_Y} ${VIEW_HALF_X * 2} ${VIEW_HALF_Y * 2}`}
        focusable="false"
      >
        <g className="floating-particle-links">
          {Array.from({ length: MAX_LINES }, (_, index) => (
            <line
              key={index}
              ref={(element) => {
                lineRefs.current[index] = element;
              }}
              className="floating-particle-link"
              strokeOpacity="0"
            />
          ))}
        </g>
        <g className="floating-particle-dots">
          {PARTICLES.map((particle, index) => (
            <circle
              key={index}
              ref={(element) => {
                dotRefs.current[index] = element;
              }}
              className="floating-particle-dot"
              cx={particle.x}
              cy={particle.y}
              r={particle.radius}
              opacity={particle.alpha}
            />
          ))}
        </g>
      </svg>
    </div>
  );
}
