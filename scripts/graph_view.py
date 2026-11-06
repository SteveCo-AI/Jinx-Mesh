"""Read-only resident graph viewer. Snapshots arrive through a private stdin pipe."""
import json
import math
import queue
import sys
import threading
from PySide6.QtCore import Qt, QTimer, QPointF
from PySide6.QtGui import QColor, QPainter, QPen
from PySide6.QtWidgets import QApplication, QWidget, QVBoxLayout, QLabel

updates = queue.Queue(maxsize=1)


def receive():
    for line in sys.stdin:
        try:
            data = json.loads(line)
        except ValueError:
            continue
        try:
            updates.get_nowait()
        except queue.Empty:
            pass
        updates.put(data)
    updates.put(None)


class Graph(QWidget):
    def __init__(self):
        super().__init__()
        self.data = {"locked": True, "nodes": [], "edges": []}
        self.selected = None
        self.positions = {}
        self.setMinimumSize(600, 440)
        self.setMouseTracking(True)

    def paintEvent(self, event):
        painter = QPainter(self)
        painter.setRenderHint(QPainter.Antialiasing)
        painter.fillRect(self.rect(), QColor("#101820"))
        nodes = self.data.get("nodes", [])
        radius = max(60, min(self.width(), self.height()) / 2 - 70)
        self.positions = {}
        # Stable order keeps the drawing deterministic; no continuous animation.
        for i, node in enumerate(nodes):
            angle = i * 2 * math.pi / max(1, len(nodes))
            self.positions[node["id"]] = QPointF(self.width()/2 + radius*math.cos(angle), self.height()/2 + radius*math.sin(angle))
        neighbors = {self.selected}
        for edge in self.data.get("edges", []):
            a, b = edge["from"], edge["to"]
            if a not in self.positions or b not in self.positions:
                continue
            active = self.selected in (a, b)
            if active:
                neighbors.update((a, b))
            painter.setPen(QPen(QColor("#f7c66a" if active else "#365366"), 2 if active else 1))
            start, end = self.positions[a], self.positions[b]
            painter.drawLine(start, end)
            # Arrow indicates learned word order.
            delta = end - start
            length = math.hypot(delta.x(), delta.y())
            if length > 1:
                ux, uy = delta.x()/length, delta.y()/length
                tip = end - QPointF(ux*9, uy*9)
                painter.drawLine(tip, tip - QPointF(ux*7-uy*4, uy*7+ux*4))
                painter.drawLine(tip, tip - QPointF(ux*7+uy*4, uy*7-ux*4))
        for node in nodes:
            point = self.positions[node["id"]]
            highlighted = node["id"] in neighbors
            painter.setPen(Qt.NoPen)
            painter.setBrush(QColor("#f7c66a" if highlighted else "#60c8b4"))
            painter.drawEllipse(point, 6, 6)
            if len(nodes) <= 35 or highlighted:
                painter.setPen(QColor("#ffffff"))
                painter.drawText(point + QPointF(9, -6), node["label"][:35])
        if not nodes:
            painter.setPen(QColor("#c9d4df"))
            painter.drawText(self.rect(), Qt.AlignCenter, "Gesperrt – im Chat /login eingeben" if self.data.get("locked") else "Noch keine geladenen Wortknoten")

    def mousePressEvent(self, event):
        point = event.position()
        nearest = min(self.positions, key=lambda key: (self.positions[key]-point).manhattanLength(), default=None)
        self.selected = nearest if nearest and (self.positions[nearest]-point).manhattanLength() < 25 else None
        self.update()

    def mouseMoveEvent(self, event):
        point = event.position()
        for node in self.data.get("nodes", []):
            if (self.positions.get(node["id"], QPointF(-1000,-1000))-point).manhattanLength() < 18:
                self.setToolTip(f'{node["label"]}\nStärke: {node["strength"]:.3f}')
                return
        self.setToolTip("")


app = QApplication(sys.argv)
window = QWidget()
window.setWindowTitle("Jinx-Mesh · Aktives Netzwerk")
layout = QVBoxLayout(window)
status = QLabel("Warte auf Graphdaten …")
status.setWordWrap(True)
layout.addWidget(status)
graph = Graph()
layout.addWidget(graph)
layout.addWidget(QLabel("Geladener Ausschnitt · Pfeile: Wortfolge · Knoten anklicken: Verbindungen hervorheben"))
window.resize(780, 650)


def refresh():
    try:
        data = updates.get_nowait()
    except queue.Empty:
        return
    if data is None:
        app.quit()
        return
    if data == graph.data:
        return
    graph.data = data
    progress = data.get("status") or {}
    phase = {"complete": "vollständig", "budget_limited": "wird schrittweise geladen"}.get(progress.get("phase"), "bereit")
    status.setText("Gesperrt" if data.get("locked") else f'{len(data["nodes"])} geladene Knoten / {data.get("total", 0)} gespeichert · {len(data["edges"])} sichtbare Verbindungen\nAktivierung: {phase} · {progress.get("pages_loaded", 0)} Seiten · {progress.get("sources_loaded", 0)} Archivepisoden')
    graph.update()


threading.Thread(target=receive, daemon=True).start()
timer = QTimer()
timer.timeout.connect(refresh)
timer.start(200)
window.show()
app.exec()
