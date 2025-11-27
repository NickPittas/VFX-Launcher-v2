# Sidebar navigation for Slick UI
from PySide6.QtWidgets import QWidget, QVBoxLayout, QPushButton, QSizePolicy
from PySide6.QtCore import Signal, Qt, QEvent, QPropertyAnimation, QEasingCurve

class Sidebar(QWidget):
    """
    Sidebar navigation for Slick UI.
    Emits navigation_requested(index) when a button is pressed.
    Accessible, modular, and visually clear.
    Supports collapsing/expanding.
    """
    navigation_requested = Signal(int)
    collapsed_changed = Signal(bool)  # Emits True when collapsed, False when expanded

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setObjectName("Sidebar")
        self.is_collapsed = False
        self.expanded_width = 200
        self.collapsed_width = 60
        self._setup_ui()
        self._setup_accessibility()

    def _setup_ui(self):
        layout = QVBoxLayout(self)
        layout.setContentsMargins(8, 16, 8, 16)
        layout.setSpacing(12)  # Reduced spacing for more compact look

        # Add collapse/expand toggle button at the top
        self.toggle_btn = QPushButton("☰")  # Hamburger menu icon
        self.toggle_btn.setObjectName("SidebarToggle")
        self.toggle_btn.setSizePolicy(QSizePolicy.Expanding, QSizePolicy.Fixed)
        self.toggle_btn.setMinimumHeight(36)
        self.toggle_btn.setMaximumHeight(36)
        self.toggle_btn.setToolTip("Collapse/Expand Sidebar")
        self.toggle_btn.setStyleSheet("""
            QPushButton#SidebarToggle {
                background-color: #1e293b;
                color: #e2e8f0;
                border: none;
                border-radius: 6px;
                padding: 8px 12px;
                font-size: 16px;
                font-weight: 600;
                text-align: center;
            }
            QPushButton#SidebarToggle:hover {
                background-color: #334155;
            }
            QPushButton#SidebarToggle:pressed {
                background-color: #1d4ed8;
            }
        """)
        self.toggle_btn.clicked.connect(self.toggle_collapse)
        layout.addWidget(self.toggle_btn)

        self.buttons = []
        self.nav_items = [
            ("Projects", 0, "Browse and manage your VFX projects", "P"),
            ("Users", 1, "Manage user accounts and permissions", "U"),
            ("Settings", 2, "Configure application settings and paths", "S"),
            ("Logs", 3, "View application logs and debug information", "L"),
        ]
        for label, idx, tooltip, icon in self.nav_items:
            btn = QPushButton(label)
            btn.setObjectName(f"SidebarBtn{label}")
            btn.setSizePolicy(QSizePolicy.Expanding, QSizePolicy.Fixed)
            btn.setFocusPolicy(Qt.StrongFocus)
            btn.setAccessibleName(label)
            btn.setToolTip(tooltip)
            btn.setProperty("label", label)
            btn.setProperty("icon", icon)

            # Make buttons shorter and more slick
            btn.setMinimumHeight(36)
            btn.setMaximumHeight(36)
            btn.setStyleSheet("""
                QPushButton {
                    background-color: #1e293b;
                    color: #e2e8f0;
                    border: none;
                    border-radius: 6px;
                    padding: 8px 12px;
                    font-size: 13px;
                    font-weight: 500;
                    text-align: left;
                }
                QPushButton:hover {
                    background-color: #334155;
                }
                QPushButton[active="true"] {
                    background-color: #2563eb;
                    color: white;
                    font-weight: 600;
                }
                QPushButton:pressed {
                    background-color: #1d4ed8;
                }
            """)

            btn.clicked.connect(lambda _, i=idx: self.navigation_requested.emit(i))
            layout.addWidget(btn)
            self.buttons.append(btn)
        layout.addStretch(1)

        # Set initial width
        self.setFixedWidth(self.expanded_width)

        # Default: highlight first button
        self._highlight_button(0)

    def _setup_accessibility(self):
        # Keyboard navigation: up/down arrow, Enter to activate
        for i, btn in enumerate(self.buttons):
            btn.installEventFilter(self)

    def eventFilter(self, obj, event):
        if event.type() == QEvent.KeyPress:
            idx = self.buttons.index(obj) if obj in self.buttons else -1
            if idx != -1:
                if event.key() == Qt.Key_Down:
                    next_idx = (idx + 1) % len(self.buttons)
                    self.buttons[next_idx].setFocus()
                    self._highlight_button(next_idx)
                    return True
                elif event.key() == Qt.Key_Up:
                    prev_idx = (idx - 1) % len(self.buttons)
                    self.buttons[prev_idx].setFocus()
                    self._highlight_button(prev_idx)
                    return True
                elif event.key() in (Qt.Key_Return, Qt.Key_Enter, Qt.Key_Space):
                    self.navigation_requested.emit(idx)
                    return True
        return super().eventFilter(obj, event)

    def _highlight_button(self, idx):
        for i, btn in enumerate(self.buttons):
            btn.setProperty("active", i == idx)
            btn.style().unpolish(btn)
            btn.style().polish(btn)

    def set_active_button(self, idx):
        """Public method to set the active button"""
        self._highlight_button(idx)

    def toggle_collapse(self):
        """Toggle sidebar between collapsed and expanded states"""
        self.is_collapsed = not self.is_collapsed

        if self.is_collapsed:
            # Collapse: show only first letter
            target_width = self.collapsed_width

            # Update button text to show only first letter
            for i, btn in enumerate(self.buttons):
                # Get the first letter from the nav_items list
                first_letter = self.nav_items[i][3]  # icon is at index 3
                btn.setText(first_letter)
                btn.setStyleSheet(btn.styleSheet() + "\nQPushButton { text-align: center; }")
        else:
            # Expand: show full labels
            target_width = self.expanded_width

            # Update button text to show full label
            for i, btn in enumerate(self.buttons):
                # Get the full label from the nav_items list
                full_label = self.nav_items[i][0]  # label is at index 0
                btn.setText(full_label)
                btn.setStyleSheet(btn.styleSheet().replace("\nQPushButton { text-align: center; }", ""))

        # Animate the width change
        self.animation = QPropertyAnimation(self, b"maximumWidth")
        self.animation.setDuration(200)
        self.animation.setEasingCurve(QEasingCurve.InOutQuad)
        self.animation.setStartValue(self.width())
        self.animation.setEndValue(target_width)

        # Also set minimum width to match
        self.animation.finished.connect(lambda: self.setMinimumWidth(target_width))

        self.animation.start()
        self.collapsed_changed.emit(self.is_collapsed)

    def set_collapsed(self, collapsed):
        """Set the collapsed state without animation"""
        if self.is_collapsed != collapsed:
            self.is_collapsed = collapsed

            if self.is_collapsed:
                self.setFixedWidth(self.collapsed_width)
                for i, btn in enumerate(self.buttons):
                    # Get the first letter from the nav_items list
                    first_letter = self.nav_items[i][3]  # icon is at index 3
                    btn.setText(first_letter)
                    btn.setStyleSheet(btn.styleSheet() + "\nQPushButton { text-align: center; }")
            else:
                self.setFixedWidth(self.expanded_width)
                for i, btn in enumerate(self.buttons):
                    # Get the full label from the nav_items list
                    full_label = self.nav_items[i][0]  # label is at index 0
                    btn.setText(full_label)
                    btn.setStyleSheet(btn.styleSheet().replace("\nQPushButton { text-align: center; }", ""))

            self.collapsed_changed.emit(self.is_collapsed)

    def is_sidebar_collapsed(self):
        """Return whether the sidebar is currently collapsed"""
        return self.is_collapsed
