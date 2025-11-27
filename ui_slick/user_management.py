# Modern User Management Panel for Slick UI
from PySide6.QtWidgets import (
    QWidget, QVBoxLayout, QHBoxLayout, QLabel, QLineEdit, QPushButton, QTableView, QHeaderView, QAbstractItemView,
    QStyledItemDelegate, QComboBox, QProgressBar
)
from PySide6.QtCore import Qt, QSortFilterProxyModel, QModelIndex, Signal, QThreadPool
from PySide6.QtGui import QStandardItemModel, QStandardItem, QColor, QFont
from .async_workers import WorkerSignals, QRunnable

class UserLoadWorker(QRunnable):
    """
    QRunnable to load users from the database asynchronously.
    Emits finished(result) or error((exctype, value, traceback)).
    """
    def __init__(self, db_manager):
        super().__init__()
        self.db_manager = db_manager
        self.signals = WorkerSignals()

    def run(self):
        try:
            # Use real DB query
            users = self.db_manager.get_all_users()
            # Map DB fields to UI fields
            result = []
            for u in users:
                role = "Admin" if u.get("is_admin", 0) else "Artist"  # Default to Artist if not admin
                result.append({
                    "username": u.get("username"),
                    "role": role,
                    "id": u.get("id")
                })
            self.signals.finished.emit(result)
        except Exception as e:
            import traceback
            self.signals.error.emit((type(e), e, traceback.format_exc()))

class UserManagement(QWidget):
    """
    Modern User Management panel for Slick UI.
    - Async user loading
    - Avatars (initials), role badges
    - Inline editing (username, role)
    - Search/filter bar
    """
    user_updated = Signal(dict)  # Emits updated user info

    def __init__(self, db_path, parent=None):
        super().__init__(parent)
        self.setObjectName("UserManagement")
        self.db_path = db_path
        self._setup_ui()
        self.threadpool = QThreadPool.globalInstance()
        self._load_users_async()

    def _setup_ui(self):
        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(10)

        # --- Search/Filter Bar ---
        search_bar = QHBoxLayout()
        self.search_input = QLineEdit()
        self.search_input.setPlaceholderText("Search users...")
        self.search_input.textChanged.connect(self._filter_users)
        search_btn = QPushButton("Search")
        search_btn.clicked.connect(self._filter_users)
        search_bar.addWidget(self.search_input)
        search_bar.addWidget(search_btn)
        layout.addLayout(search_bar)

        # --- Add User Button ---
        add_user_bar = QHBoxLayout()
        self.add_user_btn = QPushButton("Add User")
        self.add_user_btn.clicked.connect(self._on_add_user)
        add_user_bar.addWidget(self.add_user_btn)
        add_user_bar.addStretch(1)
        layout.addLayout(add_user_bar)

        # --- User Table ---
        self.model = QStandardItemModel()
        self.model.setHorizontalHeaderLabels(["Avatar", "Username", "Role"])
        self.proxy_model = QSortFilterProxyModel(self)
        self.proxy_model.setSourceModel(self.model)
        self.proxy_model.setFilterKeyColumn(1)  # Username

        self.table = QTableView()
        self.table.setModel(self.proxy_model)
        self.table.setEditTriggers(QAbstractItemView.DoubleClicked | QAbstractItemView.SelectedClicked)
        self.table.setSelectionBehavior(QAbstractItemView.SelectRows)
        self.table.setAlternatingRowColors(True)
        self.table.horizontalHeader().setSectionResizeMode(QHeaderView.Stretch)
        self.table.setItemDelegateForColumn(2, RoleBadgeDelegate(self))
        self.table.setContextMenuPolicy(Qt.CustomContextMenu)
        self.table.customContextMenuRequested.connect(self._on_table_context_menu)
        layout.addWidget(self.table, 1)

        # --- Loading Indicator ---
        self.loading_bar = QProgressBar()
        self.loading_bar.setRange(0, 0)
        self.loading_bar.setVisible(False)
        layout.addWidget(self.loading_bar)

        # --- Status Bar ---
        self.status_label = QLabel("")
        layout.addWidget(self.status_label)

    def _load_users_async(self):
        self.loading_bar.setVisible(True)
        self.status_label.setText("Loading users...")
        worker = UserLoadWorker(self.db_path)
        worker.signals.finished.connect(self._on_users_loaded)
        worker.signals.error.connect(self._on_users_load_error)
        self.threadpool.start(worker)

    def _on_users_loaded(self, users):
        self.loading_bar.setVisible(False)
        self._populate_users(users)

    def _on_users_load_error(self, exc_info):
        self.loading_bar.setVisible(False)
        exctype, value, tb = exc_info
        self.status_label.setText(f"Error loading users: {value}")

    def _populate_users(self, users):
        self.model.removeRows(0, self.model.rowCount())
        for user in users:
            avatar = QStandardItem(self._make_avatar(user["username"]))
            avatar.setEditable(False)
            username = QStandardItem(user["username"])
            role = QStandardItem(user["role"])
            self.model.appendRow([avatar, username, role])
        self.status_label.setText(f"{len(users)} user(s) loaded.")

    def _make_avatar(self, username):
        initials = username[:2].upper()
        return initials

    def _filter_users(self):
        text = self.search_input.text()
        self.proxy_model.setFilterFixedString(text)
        self.status_label.setText(f"Filter: '{text}'")

    def _on_add_user(self):
        from ui.user_management import NewUserDialog  # Classic dialog logic
        from core.database import DatabaseManager
        db = DatabaseManager(self.db_path)
        dialog = NewUserDialog(db, self)
        if dialog.exec():
            self._load_users_async()

    def _on_table_context_menu(self, pos):
        from PySide6.QtWidgets import QMenu
        menu = QMenu(self)
        index = self.table.indexAt(pos)
        if not index.isValid():
            return
        edit_action = menu.addAction("Edit User")
        delete_action = menu.addAction("Delete User")
        action = menu.exec(self.table.viewport().mapToGlobal(pos))
        if action == edit_action:
            self._on_edit_user(index)
        elif action == delete_action:
            self._on_delete_user(index)

    def _on_edit_user(self, index):
        from ui.user_management import EditUserDialog
        from core.database import DatabaseManager
        db = DatabaseManager(self.db_path)
        # Map proxy index to source
        proxy_row = index.row()
        source_row = self.proxy_model.mapToSource(self.proxy_model.index(proxy_row, 1)).row()
        username = self.model.item(source_row, 1).text()
        user = db.get_user_by_username(username)
        if user:
            dialog = EditUserDialog(db, user, self)
            if dialog.exec():
                self._load_users_async()

    def _on_delete_user(self, index):
        from PySide6.QtWidgets import QMessageBox
        from core.database import DatabaseManager
        db = DatabaseManager(self.db_path)
        proxy_row = index.row()
        source_row = self.proxy_model.mapToSource(self.proxy_model.index(proxy_row, 1)).row()
        username = self.model.item(source_row, 1).text()
        user = db.get_user_by_username(username)
        # Prevent deleting currently logged-in user (if available)
        current_username = getattr(self, 'current_user', {}).get('username', None)
        if current_username and user and user.get('username') == current_username:
            QMessageBox.warning(self, "Cannot Delete", "You cannot delete your own account.")
            return
        if user:
            reply = QMessageBox.question(
                self,
                "Confirm Deletion",
                f"Are you sure you want to delete user '{user['username']}'?",
                QMessageBox.Yes | QMessageBox.No,
                QMessageBox.No
            )
            if reply == QMessageBox.Yes:
                try:
                    db.delete_user(user['id'])
                    # Optionally: log the action using classic logic
                    try:
                        from core.user_actions import UserActionLogger
                        action_logger = UserActionLogger()
                        current_username = getattr(self, 'current_user', {}).get('username', None)
                        action_logger.log_user_remove(current_username or "", user['username'])
                    except Exception:
                        pass
                    self._load_users_async()
                except Exception as e:
                    QMessageBox.critical(self, "Database Error", f"Error deleting user: {str(e)}")

class RoleBadgeDelegate(QStyledItemDelegate):
    """
    Custom delegate to render role badges with color and bold font.
    """
    ROLE_COLORS = {
        "Admin": QColor("#d97706"),
        "Artist": QColor("#2563eb"),
        "TD": QColor("#059669"),
    }
    def paint(self, painter, option, index):
        role = index.data()
        color = self.ROLE_COLORS.get(role, QColor("#64748b"))
        painter.save()
        painter.setBrush(color)
        painter.setPen(Qt.NoPen)
        rect = option.rect
        painter.drawRoundedRect(rect, 8, 8)
        painter.setPen(Qt.white)
        font = QFont(option.font)
        font.setBold(True)
        painter.setFont(font)
        painter.drawText(rect, Qt.AlignCenter, role)
        painter.restore()
    def createEditor(self, parent, option, index):
        editor = QComboBox(parent)
        editor.addItems(["Admin", "Artist", "TD"])
        return editor
    def setEditorData(self, editor, index):
        value = index.data()
        idx = editor.findText(value)
        if idx >= 0:
            editor.setCurrentIndex(idx)
    def setModelData(self, editor, model, index):
        model.setData(index, editor.currentText())
