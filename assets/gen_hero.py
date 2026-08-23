#!/usr/bin/env python3
"""Generate README.md's animated version of the landing-page grove.

The site renders individual cached glyphs on a canvas. README images cannot run
that JavaScript, so this generator translates the same seeded branch geometry,
depth-based sway, flowing branch text, and falling glyphs into SVG/SMIL.
"""

from __future__ import annotations

from dataclasses import dataclass
import html
import math
from pathlib import Path

WIDTH = 1600
HEIGHT = 1140
GROVE_LEFT = 65
GROVE_WIDTH = 1470
REFERENCE_CSS_WIDTH = 1140
STAGE_HEIGHT = 760
GROUND = 730
BACKGROUND = "#faf9f5"
INK = "#1c1a16"
OUTPUT = Path(__file__).with_name("hero.svg")

WHEN = ("btt check", "btt scaffold", "btt packs", "btt init", "btt help")
IT = (
    "check test files against their .tree specs",
    "generate a test-file skeleton from a .tree spec",
    "list available language packs",
    "create btt.toml in this project",
    "print this message",
)


class MinStd:
    """The same Park-Miller PRNG used by the landing-page reference."""

    def __init__(self, seed: int) -> None:
        self.state = seed

    def next(self) -> float:
        self.state = (self.state * 16807) % 2147483647
        return (self.state - 1) / 2147483646


@dataclass
class Branch:
    parent: int
    angle_off: float
    length: float
    depth: int
    phase: float
    text: str
    absolute_angle: float = 0
    end_x: float = 0
    end_y: float = 0


@dataclass
class Tree:
    x: float
    scale: float
    branches: list[Branch]


def make_grove() -> list[Tree]:
    grove_random = MinStd(137)
    # The README artwork is exported wider than the browser's CSS canvas. Tree
    # density is still based on the reference's 1140px layout width.
    count = max(13, round(REFERENCE_CSS_WIDTH / 58))
    specs = [
        (
            (index + 0.15 + grove_random.next() * 0.7) / count,
            0.32 + grove_random.next() * 0.78,
            7 + int(grove_random.next() * 997),
        )
        for index in range(count)
    ]
    trees: list[Tree] = []

    for fraction, scale, seed in specs:
        branch_random = MinStd(seed)
        branches: list[Branch] = []

        def grow(parent: int, angle_off: float, length: float, depth: int) -> None:
            index = len(branches)
            if depth == 0:
                text = "btt check · btt scaffold · btt init · btt packs · "
            elif depth == 1:
                text = f"├── {WHEN[(index + seed) % len(WHEN)]} "
            else:
                prefix = "└── " if depth >= 3 else "│ "
                text = f"{prefix}{IT[(index * 3 + seed) % len(IT)]} "

            branches.append(
                Branch(parent, angle_off, length, depth, branch_random.next() * 6.28, text)
            )
            if depth >= 4 or length < 24 * scale:
                return

            children = 3 if depth == 0 else (2 if branch_random.next() < 0.72 else 1)
            for child in range(children):
                spread = 0.9 if depth == 0 else 0.75
                offset = (
                    (child / max(1, children - 1) - 0.5) * spread
                    + (branch_random.next() - 0.5) * 0.3
                )
                grow(
                    index,
                    offset,
                    length * (0.62 + branch_random.next() * 0.16),
                    depth + 1,
                )

        trunk_length = min(STAGE_HEIGHT * 0.30, 210) * scale
        grow(-1, (branch_random.next() - 0.5) * 0.08, trunk_length, 0)
        tree = Tree(GROVE_LEFT + GROVE_WIDTH * fraction, scale, branches)
        calculate_static_tips(tree)
        trees.append(tree)

    return trees


def calculate_static_tips(tree: Tree) -> None:
    starts: list[tuple[float, float]] = []
    for branch in tree.branches:
        if branch.parent < 0:
            start_x, start_y = tree.x, GROUND
            branch.absolute_angle = -math.pi / 2 + branch.angle_off
        else:
            parent = tree.branches[branch.parent]
            start_x, start_y = parent.end_x, parent.end_y
            branch.absolute_angle = parent.absolute_angle + branch.angle_off
        starts.append((start_x, start_y))
        branch.end_x = start_x + math.cos(branch.absolute_angle) * branch.length
        branch.end_y = start_y + math.sin(branch.absolute_angle) * branch.length


def repeated_text(text: str, required_characters: int) -> str:
    repeats = max(2, math.ceil((required_characters + 2) / len(text)) + 1)
    return html.escape(text * repeats).rstrip()


def sway_animations(branch: Branch) -> str:
    """Express the canvas wind equation as three additive sine waves."""
    amplitude = 0.015 + branch.depth * 0.022
    # (0.6 + 0.4 sin(0.33t)) sin(1.1t + p) expands to these terms.
    components = (
        (0.6, 1.1, branch.phase),
        (0.2, 0.77, branch.phase + math.pi / 2),
        (0.2, 1.43, branch.phase - math.pi / 2),
    )
    animations = []
    for scale, frequency, phase in components:
        degrees = math.degrees(amplitude * scale)
        values = ";".join(
            f"{degrees * math.sin(2 * math.pi * sample / 8):.3f}"
            for sample in range(9)
        )
        duration = 2 * math.pi / frequency
        delay = -((phase % (2 * math.pi)) / frequency)
        animations.append(
            '<animateTransform class="motion" attributeName="transform" '
            'type="rotate" additive="sum" '
            f'values="{values}" dur="{duration:.4f}s" begin="{delay:.4f}s" '
            'repeatCount="indefinite"/>'
        )
    return "".join(animations)


def render_tree(tree: Tree, tree_index: int) -> str:
    children: dict[int, list[int]] = {}
    for index, branch in enumerate(tree.branches):
        children.setdefault(branch.parent, []).append(index)

    def render_branch(index: int) -> str:
        branch = tree.branches[index]
        font_size = (15, 12.5, 11, 9.5, 8.5)[branch.depth] * (
            0.8 + tree.scale * 0.25
        )
        weight = 500 if branch.depth == 0 else 400
        opacity = 0.92 - branch.depth * 0.15
        step = font_size * 0.64
        character_count = max(2, int(branch.length / step))
        rate = 3.2 + branch.depth * 1.4
        text_length = len(branch.text)
        flow_duration = text_length / rate
        branch_id = f"branch-{tree_index}-{index}"
        base_degrees = math.degrees(
            (-math.pi / 2 if branch.parent < 0 else 0) + branch.angle_off
        )
        phase_offset = -((branch.phase * 5) % text_length) / rate
        flow_values = ";".join(
            f"{-offset * step:.2f}" for offset in range(text_length + 1)
        )
        text = repeated_text(branch.text, character_count)

        descendants = "".join(render_branch(child) for child in children.get(index, []))
        return f'''<g transform="rotate({base_degrees:.3f})">
          {sway_animations(branch)}
          <path id="{branch_id}" d="M0 0H{branch.length:.2f}" fill="none"/>
          <text font-size="{font_size:.2f}" font-weight="{weight}" opacity="{opacity:.2f}" letter-spacing="0.04em" xml:space="preserve">
            <textPath href="#{branch_id}" startOffset="0">{text}
              <animate class="motion" attributeName="startOffset" values="{flow_values}"
                calcMode="discrete" dur="{flow_duration:.4f}s" begin="{phase_offset:.4f}s" repeatCount="indefinite"/>
            </textPath>
          </text>
          <g transform="translate({branch.length:.2f} 0)">{descendants}</g>
        </g>'''

    trunks = "".join(render_branch(index) for index in children.get(-1, []))
    return f'<g transform="translate({tree.x:.2f} {GROUND})">{trunks}</g>'


def render_falling_glyphs(trees: list[Tree]) -> str:
    tips = [
        (branch.end_x, branch.end_y)
        for tree in trees
        for branch in tree.branches
        if branch.depth >= 3
    ]
    leaf_random = MinStd(7)
    characters = "│├└─·btt·"
    leaves: list[str] = []

    for index in range(50):
        # Consume the same five random values as the canvas initializer.
        leaf_random.next()
        leaf_random.next()
        velocity = 0.25 + leaf_random.next() * 0.5
        phase = leaf_random.next() * 6.28
        character = characters[int(leaf_random.next() * 9)]

        tip = tips[(index * 47 + 11) % len(tips)]
        start_x, start_y = tip
        fall_distance = max(40, GROUND + 4 - start_y)
        duration = max(8, fall_distance / (velocity * 30))
        delay = -(phase / 6.28) * duration
        positions = []
        for sample in range(41):
            elapsed = duration * sample / 40
            x = start_x + (15 / 1.4) * (
                math.cos(phase) - math.cos(elapsed * 1.4 + phase)
            )
            y = start_y + fall_distance * sample / 40
            positions.append(f"{x:.2f} {y:.2f}")
        fade_start = max(0, (fall_distance - 60) / fall_distance)
        leaves.append(
            f'''<text class="falling-glyph motion" x="0" y="0">{html.escape(character)}
              <animateTransform attributeName="transform" type="translate" values="{';'.join(positions)}"
                dur="{duration:.2f}s" begin="{delay:.2f}s" repeatCount="indefinite"/>
              <animate attributeName="opacity" values="0.4;0.4;0" keyTimes="0;{fade_start:.3f};1"
                dur="{duration:.2f}s" begin="{delay:.2f}s" repeatCount="indefinite"/>
            </text>'''
        )
    return "".join(leaves)


def ground_markup() -> str:
    dashes = "".join(
        f'<text x="{x}" y="{GROUND + 6}" class="ground-dash">─</text>'
        for x in range(GROVE_LEFT, GROVE_LEFT + GROVE_WIDTH, 12)
    )
    dots = "".join(
        f'<text x="{x}" y="{GROUND + 18 + 5 * math.sin(x):.2f}" class="ground-dot">·</text>'
        for x in range(GROVE_LEFT + 8, GROVE_LEFT + GROVE_WIDTH, 46)
    )
    return dashes + dots


trees = make_grove()
grove = "".join(render_tree(tree, index) for index, tree in enumerate(trees))
falling_glyphs = render_falling_glyphs(trees)

checker_lines = (
    "checker",
    "├── when the file matches the spec",
    "│   ├── it reports no findings",
    "│   └── it sees through wrapper blocks",
    "├── when a test is missing",
    "│   └── it reports it with the spec line",
    "├── when the file has extra tests",
    "│   └── it reports each extra node",
    "└── when sibling order differs",
    "    └── it reports an order finding",
)
checker_tspans = "".join(
    f'<tspan x="920" dy="{0 if index == 0 else 23}">{html.escape(line)}</tspan>'
    for index, line in enumerate(checker_lines)
)

svg = f'''<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{HEIGHT}" viewBox="0 0 {WIDTH} {HEIGHT}">
  <style>
    .grove {{ fill: {INK}; font-family: "IBM Plex Mono", "Courier New", monospace; }}
    .falling-glyph {{ font: 400 10px "IBM Plex Mono", "Courier New", monospace; }}
    .ground-dash {{ fill: rgb(28 26 22 / 22%); font: 11px "IBM Plex Mono", "Courier New", monospace; }}
    .ground-dot {{ fill: rgb(28 26 22 / 13%); font: 11px "IBM Plex Mono", "Courier New", monospace; }}
  </style>
  <rect width="{WIDTH}" height="{HEIGHT}" fill="{BACKGROUND}"/>
  <clipPath id="forest-clip"><rect x="40" y="22" width="1520" height="740"/></clipPath>
  <g class="grove" clip-path="url(#forest-clip)">
    {ground_markup()}
    {grove}
    {falling_glyphs}
  </g>

  <g text-anchor="middle" fill="#1c1a16" font-family="Newsreader, Georgia, 'Times New Roman', serif">
    <text x="560" y="890" font-size="34" font-weight="400" letter-spacing="19">btt</text>
    <text x="560" y="930" font-size="17" font-style="italic" font-weight="300" fill="#4a463e">extendible toolbox for a branch tree testing; understand</text>
    <text x="560" y="951" font-size="17" font-style="italic" font-weight="300" fill="#4a463e">the code's tests at a glance</text>
  </g>
  <text x="920" y="825" font-family="'IBM Plex Mono', 'Courier New', monospace" font-size="14" line-height="1.55" fill="#4a463e">{checker_tspans}</text>
</svg>
'''

OUTPUT.write_text(svg, encoding="utf-8")
print(f"wrote {OUTPUT.relative_to(OUTPUT.parent.parent)}, {len(svg) // 1024} KB")
