"""An aquarium over the editor, to show how a plugin makes flair.

Fish swim across with a couple of animation frames, bubbles rise, clicking a fish makes it
talk and saving the file lets out a burst of bubbles. mog does the moving and animating, so the
plugin only says what to draw. It hides with the rest of the flair in serious mode, and
`flair.disabled = ["plugin.aquarium"]` turns it off.
"""

import random

from mog_plugin import Plugin, span, status

plugin = Plugin()

# each fish as frames facing right, flipped for fish that swim left
FISH = [
    (["><>", "><>"], "orange"),
    (["<><", "<><"], "cyan"),
    (["><(((*>", "><(((o>"], "yellow"),
    (["}<>", ">o>"], "pink"),
]
FLIP = str.maketrans("<>()}{", "><)({}")
QUIPS = ["blub", "hi", "keep going", "nice code", "*glub*", "fish are friends"]

fish = []


def mirror(text):
    return text[::-1].translate(FLIP)


def add_fish(height=20):
    shape, color = random.choice(FISH)
    left = random.random() < 0.5
    frames = [[[span(mirror(f) if left else f, fg=color, bold=True)]] for f in shape]
    name = f"fish{len(fish)}"
    fish.append(name)
    plugin.draw(
        name,
        frames=frames,
        fps=2,
        anchor="top_left",
        x=random.randrange(0, 60),
        y=random.randrange(1, max(2, height - 2)),
        transparent=True,
        clickable=True,
        motion={"dx": -random.uniform(2, 5) if left else random.uniform(2, 5), "edge": "wrap"},
    )


def bubbles(id, x, count=3):
    rows = [[span(random.choice("oO."), fg="blue")] for _ in range(count)]
    plugin.draw(
        id,
        lines=rows,
        anchor="bottom_left",
        x=x,
        transparent=True,
        motion={"dy": -3, "edge": "wrap"},
        restart=True,
    )


@plugin.on("click")
def clicked(event):
    plugin.draw(
        "quip",
        [f" {random.choice(QUIPS)} "],
        anchor="center",
        y=-3,
        fg="bg",
        bg="accent",
        z=5,
    )
    plugin.timer("hush", 1500)


@plugin.on("saved")
def saved(event):
    bubbles("burst", random.randrange(5, 40), count=6)


@plugin.on("timer")
def hush(event):
    if event.get("id") == "hush":
        plugin.clear("quip")
        plugin.timer("hush", 0)


@plugin.command("add", title="Aquarium: Add a fish")
def add(context, args):
    layout = plugin.ask("ui/layout")
    add_fish(layout["editor"]["height"])
    return [status(f"{len(fish)} fish")]


@plugin.command("empty", title="Aquarium: Let the fish go")
def empty(context, args):
    plugin.clear()
    fish.clear()
    return [status("the fish are free")]


for _ in range(3):
    add_fish()
bubbles("bubbles", 12)
plugin.run()
