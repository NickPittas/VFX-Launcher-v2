# New Project Panel for Slick UI
# Creates a project folder named <code>_<client>_<project_name>_<YYYY-MM-DD>
# under a chosen root, pre-populated with the Structure/ subfolder preset.
from PySide6.QtWidgets import (
    QWidget, QVBoxLayout, QHBoxLayout, QLabel, QPushButton, QLineEdit,
    QFormLayout, QMessageBox, QFileDialog, QDateEdit, QListWidget
)
from PySide6.QtCore import QSettings, Qt, QDate
import os
import re
import shutil
import logging
from datetime import date

logger = logging.getLogger(__name__)

# Loose template: leading numeric code + a date anywhere in the name
NAME_RE = re.compile(r"^(\d+)_.*\d{4}-\d{2}-\d{2}")
STRUCTURE_DIR = os.path.join(os.path.dirname(__file__), "Structure")
DEFAULT_ROOT = "/mnt/Projects"


def _subsequence(needle, haystack):
    """True if needle's chars appear in order in haystack (case-insensitive)."""
    it = iter(haystack.lower())
    return all(c in it for c in needle.lower())


class FuzzyLineEdit(QLineEdit):
    """Line edit with subsequence-fuzzy autocomplete popup ('pcy' -> 'Picky')."""

    def __init__(self, parent=None):
        super().__init__(parent)
        self._items = []
        self._popup = QListWidget()
        self._popup.setWindowFlags(Qt.Popup)
        self._popup.setFocusPolicy(Qt.NoFocus)
        self._popup.itemClicked.connect(self._pick)
        self.textEdited.connect(self._refresh_popup)

    def set_items(self, items):
        self._items = items

    def _matches(self, needle):
        if not needle:
            return []
        return [i for i in self._items if _subsequence(needle, i) and i != needle]

    def _refresh_popup(self, text):
        matches = self._matches(text)
        if not matches:
            self._popup.hide()
            return
        self._popup.clear()
        self._popup.addItems(matches)
        self._popup.setCurrentRow(0)
        self._popup.move(self.mapToGlobal(self.rect().bottomLeft()))
        self._popup.resize(self.width(), min(len(matches), 8) * 28 + 8)
        self._popup.show()

    def _pick(self, item):
        self.setText(item.text())
        self._popup.hide()

    def keyPressEvent(self, event):
        if self._popup.isVisible():
            key = event.key()
            if key in (Qt.Key_Down, Qt.Key_Up):
                step = 1 if key == Qt.Key_Down else -1
                self._popup.setCurrentRow((self._popup.currentRow() + step) % self._popup.count())
                return
            if key in (Qt.Key_Return, Qt.Key_Enter, Qt.Key_Tab):
                if self._popup.currentItem():
                    self._pick(self._popup.currentItem())
                return
            if key == Qt.Key_Escape:
                self._popup.hide()
                return
        super().keyPressEvent(event)


class NewProjectPanel(QWidget):
    """
    Create a new project folder from the Structure/ preset.
    - Code: auto-derived (max existing code in root + 1), editable
    - Client: free text with fuzzy autocomplete over previously used clients
    - Project name + date complete the folder name
    """

    def __init__(self, parent=None, settings=None):
        super().__init__(parent)
        self.setObjectName("NewProjectPanel")
        self.settings = settings or QSettings("VFX_Launcher", "SlickUI")
        self._setup_ui()
        self._load_state()

    def _setup_ui(self):
        layout = QVBoxLayout(self)
        layout.setContentsMargins(24, 24, 24, 24)
        layout.setSpacing(16)

        title = QLabel("New Project")
        title.setStyleSheet("font-size: 1.5em; font-weight: bold; margin-bottom: 8px;")
        layout.addWidget(title)

        form = QFormLayout()
        form.setFieldGrowthPolicy(QFormLayout.AllNonFixedFieldsGrow)

        def row_with_button(field, btn_text, slot):
            btn = QPushButton(btn_text)
            btn.setFixedWidth(90)
            btn.clicked.connect(slot)
            row = QHBoxLayout()
            row.addWidget(field)
            row.addWidget(btn)
            return row

        # Root folder
        self.root_input = QLineEdit()
        form.addRow("Root Folder:", row_with_button(self.root_input, "Browse", self._on_browse_root))

        # Code (auto-derived, editable)
        self.code_input = QLineEdit()
        form.addRow("Code:", row_with_button(self.code_input, "Refresh", self._refresh_code))

        # Client: free text with fuzzy autocomplete from history
        self.client_input = FuzzyLineEdit()
        form.addRow("Client:", self.client_input)

        # Project name
        self.name_input = QLineEdit()
        form.addRow("Project Name:", self.name_input)

        # Date (defaults to today, user-changeable)
        self.date_input = QDateEdit(QDate.currentDate())
        self.date_input.setDisplayFormat("yyyy-MM-dd")
        self.date_input.setCalendarPopup(True)
        form.addRow("Date:", self.date_input)

        layout.addLayout(form)

        # Live preview of the final folder name
        self.preview_label = QLabel()
        self.preview_label.setStyleSheet("color: #60a5fa; font-weight: 600;")
        layout.addWidget(self.preview_label)

        create_btn = QPushButton("Create Project")
        create_btn.setMinimumHeight(40)
        create_btn.clicked.connect(self._on_create)
        layout.addWidget(create_btn)
        layout.addStretch(1)

        # Update preview on any field change
        self.root_input.textChanged.connect(self._update_preview)
        self.code_input.textChanged.connect(self._update_preview)
        self.client_input.textChanged.connect(self._update_preview)
        self.name_input.textChanged.connect(self._update_preview)
        self.date_input.dateChanged.connect(self._update_preview)

    def _load_state(self):
        self.root_input.setText(self.settings.value("newProject/rootDir", DEFAULT_ROOT))
        self.client_input.set_items(self.settings.value("newProject/clients", [], type=list))
        self._refresh_code()

    def _next_code(self, root):
        """Highest numeric code among template folders in root, +1."""
        best = 0
        try:
            for entry in os.listdir(root):
                if os.path.isdir(os.path.join(root, entry)):
                    m = NAME_RE.match(entry)
                    if m:
                        best = max(best, int(m.group(1)))
        except OSError as e:
            logger.warning(f"Cannot scan root folder {root}: {e}")
        return str(best + 1).zfill(5)

    def _refresh_code(self):
        # Code derives from the currently chosen root; fall back to the
        # default root only when no valid root folder is chosen.
        root = self.root_input.text().strip()
        if not root or not os.path.isdir(root):
            root = DEFAULT_ROOT
        if os.path.isdir(root):
            self.code_input.setText(self._next_code(root))
        self._update_preview()

    def _project_name(self):
        parts = [
            self.code_input.text().strip(),
            self.client_input.text().strip(),
            self.name_input.text().strip(),
            self.date_input.date().toString("yyyy-MM-dd"),
        ]
        return "_".join(parts)

    def _update_preview(self):
        self.preview_label.setText(f"→ {self._project_name()}")

    def _on_browse_root(self):
        dir_path = QFileDialog.getExistingDirectory(
            self, "Select Root Folder", self.root_input.text() or DEFAULT_ROOT
        )
        if dir_path:
            self.root_input.setText(dir_path)

    def _on_create(self):
        root = self.root_input.text().strip()
        code = self.code_input.text().strip()
        client = self.client_input.text().strip()
        name = self.name_input.text().strip()

        errors = []
        if not root or not os.path.isdir(root):
            errors.append(f"Root folder does not exist: {root}")
        if not code.isdigit():
            errors.append("Code must be a number.")
        if not client:
            errors.append("Client is required.")
        if not name:
            errors.append("Project name is required.")
        for field, value in (("Client", client), ("Project name", name)):
            if "_" in value:
                errors.append(f"{field} cannot contain underscores (used as separator).")
        if errors:
            QMessageBox.warning(self, "Invalid Input", "\n".join(errors))
            return

        dest = os.path.join(root, self._project_name())
        if os.path.exists(dest):
            QMessageBox.warning(self, "Already Exists", f"Folder already exists:\n{dest}")
            return

        try:
            shutil.copytree(STRUCTURE_DIR, dest)
        except FileNotFoundError:
            # Frozen build: Structure/ holds only empty dirs, which PyInstaller drops.
            # Fall back to the bundled manifest generated by build_appimage.sh.
            manifest = os.path.join(os.path.dirname(__file__), 'structure.txt')
            with open(manifest) as f:
                for line in f:
                    d = line.strip()
                    if d:
                        os.makedirs(os.path.join(dest, d), exist_ok=True)
        except Exception as e:
            logger.error(f"Failed to create project: {e}")
            QMessageBox.critical(self, "Create Failed", f"Could not create project:\n{e}")
            return

        # Persist root and remember client
        self.settings.setValue("newProject/rootDir", root)
        clients = self.settings.value("newProject/clients", [], type=list)
        if client not in clients:
            clients.append(client)
            self.settings.setValue("newProject/clients", clients)
            self.client_input.set_items(clients)

        logger.info(f"Created project folder: {dest}")
        QMessageBox.information(self, "Project Created", f"Created:\n{dest}")
        self.name_input.clear()
        self._refresh_code()


if __name__ == "__main__":
    # Self-check: loose regex code derivation, fuzzy proxy, name building, folder creation
    import tempfile
    from PySide6.QtWidgets import QApplication

    with tempfile.TemporaryDirectory() as tmp:
        # Stub modal dialogs so the check runs headless
        QMessageBox.information = QMessageBox.warning = QMessageBox.critical = staticmethod(lambda *a, **k: None)
        for d in ("23003_MNEP_MOYSEIO_2023-01-16", "25041_F&F_EPSA_CANCELLED",
                  "25050_X_Y_2025-08-31_Melissanthi", "not_a_project"):
            os.makedirs(os.path.join(tmp, d))
        app = QApplication([])
        panel = NewProjectPanel(settings=QSettings(os.path.join(tmp, "test.ini"), QSettings.IniFormat))
        # Loose regex: suffixed-date folder counts, CANCELLED (no date) doesn't
        assert panel._next_code(tmp) == "25051", panel._next_code(tmp)

        # Fuzzy matching: subsequence + case-insensitive
        panel.client_input.set_items(["Picky", "Parasol", "F&F"])
        assert panel.client_input._matches("pcy") == ["Picky"]
        assert panel.client_input._matches("f&f") == ["F&F"]
        assert panel.client_input._matches("pso") == ["Parasol"]
        assert panel.client_input._matches("Picky") == []  # exact match: no popup

        panel.root_input.setText(tmp)
        panel.code_input.setText("25051")
        panel.client_input.setText("TestClient")
        panel.name_input.setText("TestProject")
        expected = f"25051_TestClient_TestProject_{date.today():%Y-%m-%d}"
        assert panel._project_name() == expected, panel._project_name()
        panel._on_create()
        dest = os.path.join(tmp, expected)
        assert os.path.isdir(os.path.join(dest, "Projects", "Nuke")), os.listdir(dest)
        assert os.path.isdir(os.path.join(dest, "Audio", "VO"))
        assert "TestClient" in panel.client_input._items

        # Frozen-mode fallback: missing Structure dir -> create from manifest
        STRUCTURE_DIR = os.path.join(tmp, "nonexistent_structure")
        panel.code_input.setText("25051")  # _on_create re-derives the code from /mnt/Projects
        panel.name_input.setText("ManifestProject")
        expected2 = f"25051_TestClient_ManifestProject_{date.today():%Y-%m-%d}"
        panel._on_create()
        dest2 = os.path.join(tmp, expected2)
        assert os.path.isdir(os.path.join(dest2, "Projects", "Nuke")), os.listdir(dest2)
        assert os.path.isdir(os.path.join(dest2, "Audio", "VO"))
        print("self-check OK")
