const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");

const whenCommands = [
  "btt check",
  "btt scaffold",
  "btt packs",
  "btt init",
  "btt help",
] as const;

const itDescriptions = [
  "check test files against their .tree specs",
  "generate a test-file skeleton from a .tree spec",
  "list available language packs",
  "create btt.toml in this project",
  "print this message",
] as const;

interface Branch {
  parent: number;
  angleOff: number;
  len: number;
  depth: number;
  phase: number;
  text: string;
}

interface ComputedBranch {
  sx: number;
  sy: number;
  ang: number;
  ex: number;
  ey: number;
}

interface Tree {
  x: number;
  sc: number;
  branches: Branch[];
  computed: ComputedBranch[];
}

interface FallingGlyph {
  x: number;
  y: number;
  vy: number;
  phase: number;
  ch: string;
}

interface Glyph {
  img: HTMLCanvasElement;
  s: number;
}

function seededRandom(seed: number) {
  let state = seed;
  return () => {
    state = (state * 16807) % 2147483647;
    return (state - 1) / 2147483646;
  };
}

function drawForest() {
  const canvas = document.querySelector<HTMLCanvasElement>("#forest-canvas");
  const forest = document.querySelector<HTMLElement>(".forest");
  const context = canvas?.getContext("2d");
  if (!canvas || !forest || !context) return;

  let width = 0;
  let height = 0;
  let pixelRatio = 1;
  let groundY = 0;
  let trees: Tree[] = [];
  let fallingGlyphs: FallingGlyph[] = [];
  let groundImage: HTMLCanvasElement | null = null;
  let elapsed = 0;
  let previousFrame = 0;
  let animationFrame = 0;
  const glyphs = new Map<string, Glyph>();

  const glyph = (character: string, fontSize: number, weight: number) => {
    const roundedSize = Math.round(fontSize * 2) / 2;
    const key = `${character}|${roundedSize}|${weight}`;
    const cached = glyphs.get(key);
    if (cached) return cached;

    const supersampling = 2;
    const size = Math.ceil(roundedSize * 1.7);
    const image = document.createElement("canvas");
    image.width = size * supersampling;
    image.height = size * supersampling;
    const glyphContext = image.getContext("2d");
    if (!glyphContext) return { img: image, s: size };

    glyphContext.font = `${weight} ${roundedSize * supersampling}px "IBM Plex Mono", monospace`;
    glyphContext.textAlign = "center";
    glyphContext.textBaseline = "middle";
    glyphContext.fillStyle = "#1c1a16";
    glyphContext.fillText(
      character,
      (size * supersampling) / 2,
      (size * supersampling) / 2,
    );

    const rendered = { img: image, s: size };
    glyphs.set(key, rendered);
    return rendered;
  };

  const buildGrove = () => {
    groundY = height * 0.87;
    groundImage = null;
    const groveRandom = seededRandom(137);
    const count = Math.max(13, Math.round(width / 58));
    const specs = Array.from({ length: count }, (_, index) => ({
      fx: (index + 0.15 + groveRandom() * 0.7) / count,
      sc: 0.32 + groveRandom() * 0.78,
      seed: 7 + Math.floor(groveRandom() * 997),
    }));

    trees = specs.map((spec) => {
      const branchRandom = seededRandom(spec.seed);
      const branches: Branch[] = [];
      const grow = (
        parent: number,
        angleOff: number,
        length: number,
        depth: number,
      ) => {
        const index = branches.length;
        let text: string;
        if (depth === 0) {
          text = "btt check · btt scaffold · btt init · btt packs · ";
        } else if (depth === 1) {
          text = `├── ${whenCommands[(index + spec.seed) % whenCommands.length]} `;
        } else {
          const prefix = depth >= 3 ? "└── " : "│ ";
          text = `${prefix}${itDescriptions[(index * 3 + spec.seed) % itDescriptions.length]} `;
        }

        branches.push({
          parent,
          angleOff,
          len: length,
          depth,
          phase: branchRandom() * 6.28,
          text,
        });
        if (depth >= 4 || length < 24 * spec.sc) return;

        const children = depth === 0 ? 3 : branchRandom() < 0.72 ? 2 : 1;
        for (let child = 0; child < children; child += 1) {
          const spread = depth === 0 ? 0.9 : 0.75;
          const offset =
            (child / Math.max(1, children - 1) - 0.5) * spread +
            (branchRandom() - 0.5) * 0.3;
          grow(
            index,
            offset,
            length * (0.62 + branchRandom() * 0.16),
            depth + 1,
          );
        }
      };

      const trunkLength = Math.min(height * 0.3, 210) * spec.sc;
      grow(-1, (branchRandom() - 0.5) * 0.08, trunkLength, 0);
      return {
        x: width * spec.fx,
        sc: spec.sc,
        branches,
        computed: branches.map(() => ({ sx: 0, sy: 0, ang: 0, ex: 0, ey: 0 })),
      };
    });

    const leafRandom = seededRandom(7);
    const leafCharacters = "│├└─·btt·";
    fallingGlyphs = Array.from({ length: 50 }, () => ({
      x: leafRandom() * width,
      y: leafRandom() * groundY,
      vy: 0.25 + leafRandom() * 0.5,
      phase: leafRandom() * 6.28,
      ch: leafCharacters[Math.floor(leafRandom() * 9)] ?? "·",
    }));
  };

  const resize = () => {
    const bounds = forest.getBoundingClientRect();
    pixelRatio = Math.min(1.5, window.devicePixelRatio || 1);
    width = bounds.width;
    height = bounds.height;
    canvas.width = Math.max(1, Math.round(width * pixelRatio));
    canvas.height = Math.max(1, Math.round(height * pixelRatio));
    buildGrove();
  };

  const bakeGround = () => {
    const image = document.createElement("canvas");
    image.width = Math.ceil(width * pixelRatio);
    image.height = Math.ceil(30 * pixelRatio);
    const groundContext = image.getContext("2d");
    if (!groundContext) return;

    groundContext.scale(pixelRatio, pixelRatio);
    groundContext.textAlign = "center";
    groundContext.textBaseline = "middle";
    groundContext.font = '11px "IBM Plex Mono", monospace';
    groundContext.fillStyle = "rgb(28 26 22 / 22%)";
    for (let x = 6; x < width; x += 12) groundContext.fillText("─", x, 8);
    groundContext.fillStyle = "rgb(28 26 22 / 13%)";
    for (let x = 14; x < width; x += 46) {
      groundContext.fillText("·", x, 18 + 5 * Math.sin(x));
    }
    groundImage = image;
  };

  const draw = () => {
    context.setTransform(1, 0, 0, 1, 0, 0);
    context.globalAlpha = 1;
    context.fillStyle = "#faf9f5";
    context.fillRect(0, 0, canvas.width, canvas.height);

    const wind = 0.6 + 0.4 * Math.sin(elapsed * 0.33);
    if (!groundImage) bakeGround();
    if (groundImage) {
      context.drawImage(groundImage, 0, Math.round((groundY - 2) * pixelRatio));
    }

    const tips: Array<[number, number]> = [];
    for (const tree of trees) {
      for (let index = 0; index < tree.branches.length; index += 1) {
        const branch = tree.branches[index];
        const computed = tree.computed[index];
        if (!branch || !computed) continue;

        const sway =
          wind *
          Math.sin(elapsed * 1.1 + branch.phase) *
          (0.015 + branch.depth * 0.022);
        if (branch.parent < 0) {
          computed.sx = tree.x;
          computed.sy = groundY;
          computed.ang = -Math.PI / 2 + branch.angleOff + sway;
        } else {
          const parent = tree.computed[branch.parent];
          if (!parent) continue;
          computed.sx = parent.ex;
          computed.sy = parent.ey;
          computed.ang = parent.ang + branch.angleOff + sway;
        }

        const cosine = Math.cos(computed.ang);
        const sine = Math.sin(computed.ang);
        computed.ex = computed.sx + cosine * branch.len;
        computed.ey = computed.sy + sine * branch.len;
        const baseSizes = [15, 12.5, 11, 9.5, 8.5];
        const fontSize = (baseSizes[branch.depth] ?? 8.5) * (0.8 + tree.sc * 0.25);
        const weight = branch.depth === 0 ? 500 : 400;
        context.globalAlpha = 0.92 - branch.depth * 0.15;
        const step = fontSize * 0.64;
        const characterCount = Math.max(2, Math.floor(branch.len / step));
        const offset = Math.floor(
          elapsed * (3.2 + branch.depth * 1.4) + branch.phase * 5,
        );
        const transformCosine = cosine * pixelRatio;
        const transformSine = sine * pixelRatio;

        for (let characterIndex = 0; characterIndex < characterCount; characterIndex += 1) {
          const character =
            branch.text[(characterIndex + offset) % branch.text.length];
          if (!character || character === " ") continue;
          const renderedGlyph = glyph(character, fontSize, weight);
          const x = computed.sx + cosine * characterIndex * step;
          const y = computed.sy + sine * characterIndex * step;
          context.setTransform(
            transformCosine,
            transformSine,
            -transformSine,
            transformCosine,
            x * pixelRatio,
            y * pixelRatio,
          );
          context.drawImage(
            renderedGlyph.img,
            -renderedGlyph.s / 2,
            -renderedGlyph.s / 2,
            renderedGlyph.s,
            renderedGlyph.s,
          );
        }

        if (branch.depth >= 3) tips.push([computed.ex, computed.ey]);
      }
    }

    context.setTransform(pixelRatio, 0, 0, pixelRatio, 0, 0);
    if (tips.length > 0) {
      for (const leaf of fallingGlyphs) {
        if (!reducedMotion.matches) {
          leaf.y += leaf.vy;
          leaf.x += Math.sin(elapsed * 1.4 + leaf.phase) * 0.5;
        }
        if (leaf.y > groundY + 4) {
          const tip = tips[Math.floor(Math.random() * tips.length)];
          if (!tip) continue;
          leaf.x = tip[0];
          leaf.y = tip[1];
          leaf.vy = 0.25 + Math.random() * 0.5;
        }
        const fade = Math.min(1, (groundY + 4 - leaf.y) / 60);
        context.globalAlpha = 0.4 * fade;
        const renderedGlyph = glyph(leaf.ch, 10, 400);
        context.drawImage(
          renderedGlyph.img,
          leaf.x - renderedGlyph.s / 2,
          leaf.y - renderedGlyph.s / 2,
          renderedGlyph.s,
          renderedGlyph.s,
        );
      }
    }
    context.globalAlpha = 1;
  };

  const loop = (now: number) => {
    animationFrame = window.requestAnimationFrame(loop);
    if (now - previousFrame < 31) return;
    elapsed += (now - previousFrame) / 1000;
    previousFrame = now;
    draw();
  };

  const onResize = () => {
    resize();
    if (reducedMotion.matches) draw();
  };

  resize();
  if (reducedMotion.matches) {
    elapsed = 2.4;
    draw();
  } else {
    animationFrame = window.requestAnimationFrame(loop);
  }

  document.fonts?.ready.then(() => {
    glyphs.clear();
    groundImage = null;
    if (reducedMotion.matches) draw();
  });
  window.addEventListener("resize", onResize);
  window.addEventListener(
    "pagehide",
    () => {
      window.cancelAnimationFrame(animationFrame);
      window.removeEventListener("resize", onResize);
    },
    { once: true },
  );
}

function initialiseCopyButton() {
  const button = document.querySelector<HTMLButtonElement>("#copy-install");
  const status = document.querySelector<HTMLElement>("#copy-status");
  if (!button || !status) return;

  button.addEventListener("click", async () => {
    const command = button.dataset.command;
    if (!command) return;

    try {
      await navigator.clipboard.writeText(command);
      status.textContent = "copied to clipboard";
      button.classList.add("copied");
    } catch {
      const commandText = button.querySelector("span");
      const selection = window.getSelection();
      if (commandText && selection) {
        const range = document.createRange();
        range.selectNodeContents(commandText);
        selection.removeAllRanges();
        selection.addRange(range);
      }
      status.textContent = "copy unavailable — command selected";
    }

    window.setTimeout(() => {
      status.textContent = "";
      button.classList.remove("copied");
    }, 1800);
  });
}

drawForest();
initialiseCopyButton();
