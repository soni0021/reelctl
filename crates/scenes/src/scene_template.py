"""A chart for reelctl, drawn by manimgx. This file is a fixed template: reelctl puts the
chart's data, as JSON, where the marker below is, and nothing else in it changes.
Every piece of chart text is drawn with m.Text, which does not read markup.
"""

import json

import manimgx as m

# The key colour: reelctl keys it out of the video so the chart sits on the picture.
m.config.background_color = "#ff00ff"

DATA = json.loads(@@REELCTL_PAYLOAD@@)

INK = "#ffffff"
ACCENT = "#ffd23f"
BARS = ["#2ec4b6", "#3a86ff", "#ffbe0b", "#fb5607", "#8ac926"]
ANIMATION_SECONDS = 1.0


def bar_body():
    points = DATA["points"]
    chart = m.BarChart(
        [point["value"] for point in points],
        y_range=DATA["y_range"],
        bar_colors=BARS,
    )
    marks = []
    for bar, point in zip(chart.bars, points):
        marks.append(m.Text(point["label"], font_size=22, color=INK).next_to(bar, m.DOWN))
        marks.append(m.Text(point["shown"], font_size=22, color=INK).next_to(bar, m.UP))
    return chart, marks


def line_body():
    points = DATA["points"]
    last = len(points) - 1
    axes = m.Axes(x_range=[0, last, 1], y_range=DATA["y_range"], x_length=8, y_length=3.5)
    graph = axes.plot_line_graph(
        x_values=list(range(len(points))),
        y_values=[point["value"] for point in points],
        line_color=ACCENT,
    )
    marks = [
        m.Text(points[0]["label"], font_size=22, color=INK).move_to(axes.c2p(0, 0) + m.DOWN * 0.5),
        m.Text(points[last]["label"], font_size=22, color=INK).move_to(
            axes.c2p(last, 0) + m.DOWN * 0.5
        ),
        m.Text(points[last]["shown"], font_size=26, color=ACCENT).move_to(
            axes.c2p(last, points[last]["value"]) + m.UP * 0.5
        ),
    ]
    return m.VGroup(axes, graph), marks


class ReelctlChart(m.Scene):
    def construct(self) -> None:
        title = m.Text(DATA["title"], font_size=40, color=INK)
        if DATA["kind"] == "number-callout":
            big = m.Text(DATA["display"], font_size=96, color=ACCENT)
            caption = m.Text(DATA["points"][0]["label"], font_size=36, color=INK)
            layout = m.VGroup(title, big, caption).arrange(m.DOWN, buff=0.5)
            self.add(layout)
            self.wait(DATA["duration_s"])
            return
        if DATA["kind"] == "bar":
            body, marks = bar_body()
        else:
            body, marks = line_body()
        # The marks are placed against the chart, so they move with it.
        block = m.VGroup(body, *marks)
        m.VGroup(title, block).arrange(m.DOWN, buff=0.6)
        self.add(title)
        self.play(m.Create(body), run_time=ANIMATION_SECONDS)
        self.add(*marks)
        self.wait(max(DATA["duration_s"] - ANIMATION_SECONDS, 0))
