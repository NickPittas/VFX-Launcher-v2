# Modern Project Browser for Slick UI
import os
from PySide6.QtWidgets import (
    QWidget, QVBoxLayout, QHBoxLayout, QLabel, QLineEdit, QPushButton, QTreeView, QHeaderView, QAbstractItemView,
    QStyledItemDelegate, QFrame, QMenu, QProgressBar, QCheckBox, QTabWidget
)
from PySide6.QtCore import Qt, QSortFilterProxyModel, QModelIndex, Signal, QThreadPool, QItemSelectionModel
from PySide6.QtGui import QStandardItemModel, QStandardItem
from .async_workers import ProjectLoadWorker
from .version_delegate import VersionDelegate
from .favorites_manager import FavoritesManager
from .project_browser_delegate import FavoriteStarDelegate
from .file_context_menu import open_in_file_manager
import logging
logger = logging.getLogger(__name__)

class IdFilterProxyModel(QSortFilterProxyModel):
    """
    Proxy filter for the Favorites/Recents trees: accepts a row when the project ID
    stored at Qt.UserRole + 2 on column 0 is in the allowed set AND the project name
    (column 1, DisplayRole) contains the name filter (case-insensitive).
    """

    def __init__(self, parent=None):
        super().__init__(parent)
        self._allowed_ids = set()
        self._name_filter = ""

    def set_allowed_ids(self, ids):
        self._allowed_ids = set(ids)
        self.endFilterChange()  # re-run filterAcceptsRow (invalidateFilter is deprecated in Qt 6.10+)

    def set_name_filter(self, text):
        self._name_filter = (text or "").lower()
        self.endFilterChange()

    def filterAcceptsRow(self, source_row, source_parent):
        model = self.sourceModel()
        id_data = model.index(source_row, 0, source_parent).data(Qt.UserRole + 2)
        try:
            project_id = int(id_data)
        except (TypeError, ValueError):
            return False
        if project_id not in self._allowed_ids:
            return False
        if self._name_filter:
            name = model.index(source_row, 1, source_parent).data()
            if not name or self._name_filter not in str(name).lower():
                return False
        return True

class ProjectBrowser(QWidget):
    """
    Modern Project Browser panel for Slick UI.
    - Search/filter bar
    - Familiar navigation (tree view)
    - Modular card/table layout
    - Async/project loading hooks (to be connected)
    """
    project_selected = Signal(dict)  # Emits selected project info
    file_selected = Signal(list, list)  # Emits (nuke_files, aep_files) when a project is selected
    scan_finished = Signal(str)  # Emits project_path after scan completes

    def __init__(self, db_manager, user_data=None, parent=None):
        super().__init__(parent)
        self.setObjectName("ProjectBrowser")
        self.db_manager = db_manager
        self.db_path = db_manager.db_path
        self.user_data = user_data or {"username": "default", "id": 1}
        self.favorites_manager = FavoritesManager(self.user_data["username"])
        self.show_favorites_only = False
        self.show_recents_only = False
        self._setup_ui()
        self.threadpool = QThreadPool.globalInstance()
        self._load_projects_async()

    def _on_add_project(self):
        from PySide6.QtWidgets import QMessageBox
        import os
        from core.database import DatabaseManager
        from .file_dialogs import pick_directory
        folder = pick_directory(self, "Select Project Folder")
        if not folder:
            return
        name = os.path.basename(folder)
        db = DatabaseManager(self.db_path)
        try:
            db.create_project(name, folder)
            self.status_label.setText(f"Project '{name}' added.")
            self._load_projects_async()
            # Immediately scan new project for files
            self._scan_project_files_async(folder)
        except Exception as e:
            QMessageBox.critical(self, "Error", f"Failed to add project: {e}")
            self.status_label.setText(f"Error adding project: {e}")

    def _on_refresh_projects(self):
        logger.info("[DIAG] Refresh button clicked. Reloading project list from database.")
        self._load_projects_async()

    def _on_remove_project(self, tree=None):
        from PySide6.QtWidgets import QMessageBox
        from core.database import DatabaseManager
        # clicked() passes a checked bool; only accept an actual tree view
        if not isinstance(tree, QTreeView):
            tree = self.tree
        selection = tree.selectionModel().selectedRows()
        if not selection:
            QMessageBox.warning(self, "Remove Project", "No project selected.")
            return
        reply = QMessageBox.question(self, "Remove Project", "Are you sure you want to remove the selected project(s)?")
        if reply != QMessageBox.Yes:
            return
        db = DatabaseManager(self.db_path)
        failed = []
        for ix in selection:
            # Get project ID directly from model, using the same column as get_all_projects()
            row = self._map_to_source(ix).row()
            project_id_item = self.model.item(row, 0)
            try:
                project_id = int(project_id_item.text())
            except Exception as e:
                logger.error(f"[UI] Could not parse project ID from row {row}: {e}")
                failed.append(self.model.item(row, 1).text())
                continue
            result = db.delete_project(project_id)
            logger.info(f"[UI] delete_project({project_id}) result: {result}")
            if not result:
                failed.append(self.model.item(row, 1).text())
        # Always reload the project list from DB after any removal attempt
        self._load_projects_async()
        if failed:
            self.status_label.setText(f"Failed to remove: {', '.join(failed)}")
        else:
            self.status_label.setText(f"Removed {len(selection)} project(s).")

    def _on_archive_projects(self, indexes):
        """
        Archive one or more selected projects (stub for now).
        """
        from PySide6.QtWidgets import QMessageBox
        if not indexes:
            return
        names = [self.model.item(self._map_to_source(ix).row(), 1).text() for ix in indexes]
        if len(names) == 1:
            msg = f"Are you sure you want to archive project '{names[0]}'?"
        else:
            msg = f"Are you sure you want to archive {len(names)} projects?\n" + ", ".join(names[:5]) + ("..." if len(names) > 5 else "")
        reply = QMessageBox.question(self, "Archive Projects", msg, QMessageBox.Yes | QMessageBox.No)
        if reply == QMessageBox.Yes:
            # Archive logic (stub)
            self.status_label.setText(f"Archived {len(names)} project(s). (Not implemented)")

    def _on_version_changed(self, topLeft, bottomRight, roles):
        # Only respond to Version column changes
        if topLeft.column() == 2:
            project_name = self.model.item(topLeft.row(), 0).text()
            new_version = self.model.item(topLeft.row(), 2).text()
            logger.info(f"[DIAG] Version changed for '{project_name}': {new_version}")
            # TODO: Save version change to database if needed

    def _show_context_menu(self, tree, pos):
        from PySide6.QtWidgets import QMenu
        selected_indexes = tree.selectionModel().selectedRows()
        if not selected_indexes:
            return
        menu = QMenu(self)
        # Open only works for single selection
        if len(selected_indexes) == 1:
            menu.addAction("Open", lambda: self._on_project_activated(selected_indexes[0]))
        # Remove and Archive work for multi-select
        menu.addAction(
            "Remove" if len(selected_indexes) == 1 else f"Remove {len(selected_indexes)} Projects",
        lambda: self._on_remove_project(tree))
        menu.addAction(
            "Archive" if len(selected_indexes) == 1 else f"Archive {len(selected_indexes)} Projects",
            lambda: self._on_archive_projects(selected_indexes))
        menu.addAction("Duplicate", lambda: self.status_label.setText("Duplicate (not implemented)"))
        menu.addSeparator()
        # Open Containing Folder only works for single selection
        if len(selected_indexes) == 1:
            menu.addAction("Open Containing Folder", lambda: self._on_open_containing_folder(selected_indexes[0]))
        menu.exec(tree.viewport().mapToGlobal(pos))

# NOTE: If you see 'Unknown property transition' warnings in the terminal, your QSS stylesheet is using the CSS 'transition' property, which is not supported by Qt. Remove 'transition' lines from your QSS to silence these warnings.

    def _setup_ui(self):
        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(10)

        # Top controls: Add/Remove Project buttons
        add_bar = QHBoxLayout()
        self.add_project_btn = QPushButton("Add Project")
        self.add_project_btn.setStyleSheet("QPushButton { background-color: #2563eb; color: white; padding: 8px 16px; font-weight: bold; }")
        self.add_project_btn.setToolTip("Add a new project folder to the database")
        self.add_project_btn.clicked.connect(self._on_add_project)
        add_bar.addWidget(self.add_project_btn)

        self.remove_project_btn = QPushButton("Remove Project")
        self.remove_project_btn.setStyleSheet("QPushButton { background-color: #2563eb; color: white; padding: 8px 16px; font-weight: bold; }")
        self.remove_project_btn.setToolTip("Remove the selected project from the database")
        self.remove_project_btn.clicked.connect(self._on_remove_project)
        add_bar.addWidget(self.remove_project_btn)

        add_bar.addStretch(1)
        layout.addLayout(add_bar)

        # Search bar
        search_bar = QHBoxLayout()
        self.search_input = QLineEdit()
        self.search_input.setPlaceholderText("Search projects...")
        self.search_input.setMaximumWidth(300)
        self.search_input.textChanged.connect(self._filter_projects)
        search_bar.addWidget(self.search_input)
        search_bar.addStretch(1)
        layout.addLayout(search_bar)

        # Tab widget for All/Favorites/Recents
        self.tab_widget = QTabWidget()
        self.tab_widget.currentChanged.connect(self._on_tab_changed)
        # Create model and per-tree proxies for filtering
        self.model = QStandardItemModel()
        self.model.setHorizontalHeaderLabels(["★", "Name", "Type", "Last Modified", "Path"])
        self.proxy_model = QSortFilterProxyModel(self)
        self.proxy_model.setSourceModel(self.model)
        self.proxy_model.setFilterKeyColumn(1)  # Filter by project name (now column 1)

        # Favorites/Recents tabs: each tree filters by an allowed project-ID set plus name text
        self.favorites_proxy = IdFilterProxyModel(self)
        self.favorites_proxy.setSourceModel(self.model)
        self.recents_proxy = IdFilterProxyModel(self)
        self.recents_proxy.setSourceModel(self.model)

        # Create tree view for "All Projects" tab
        self.tree = self._create_tree_view(self.proxy_model)
        all_projects_widget = QWidget()
        all_layout = QVBoxLayout(all_projects_widget)
        all_layout.setContentsMargins(0, 0, 0, 0)
        all_layout.addWidget(self.tree)
        self.tab_widget.addTab(all_projects_widget, "All Projects")

        # Create tree view for "Favorites" tab (will share same model but filtered)
        self.favorites_tree = self._create_tree_view(self.favorites_proxy)
        favorites_widget = QWidget()
        fav_layout = QVBoxLayout(favorites_widget)
        fav_layout.setContentsMargins(0, 0, 0, 0)
        fav_layout.addWidget(self.favorites_tree)
        self.tab_widget.addTab(favorites_widget, "Favorites")

        # Create tree view for "Recents" tab
        self.recents_tree = self._create_tree_view(self.recents_proxy)
        recents_widget = QWidget()
        recents_layout = QVBoxLayout(recents_widget)
        recents_layout.setContentsMargins(0, 0, 0, 0)
        recents_layout.addWidget(self.recents_tree)
        self.tab_widget.addTab(recents_widget, "Recents")

        layout.addWidget(self.tab_widget, 1)

        # --- Bottom Buttons: Refresh and Scan ---
        bottom_bar = QHBoxLayout()

        self.refresh_project_btn = QPushButton("Refresh Projects")
        self.refresh_project_btn.setStyleSheet("QPushButton { background-color: #2563eb; color: white; padding: 8px 16px; font-weight: bold; }")
        self.refresh_project_btn.setToolTip("Reload the project list from the database")
        self.refresh_project_btn.clicked.connect(self._on_refresh_projects)
        bottom_bar.addWidget(self.refresh_project_btn)

        self.rescan_files_btn = QPushButton("Scan Project Files")
        self.rescan_files_btn.setStyleSheet("QPushButton { background-color: #2563eb; color: white; padding: 8px 16px; font-weight: bold; }")
        self.rescan_files_btn.setToolTip("Scan the selected project for .nk and .aep files")
        self.rescan_files_btn.clicked.connect(self._on_rescan_files_clicked)
        bottom_bar.addWidget(self.rescan_files_btn)

        bottom_bar.addStretch(1)
        layout.addLayout(bottom_bar)

        # --- Loading Indicator ---
        self.loading_bar = QProgressBar()
        self.loading_bar.setRange(0, 0)  # Indeterminate
        self.loading_bar.setVisible(False)
        layout.addWidget(self.loading_bar)

        # --- Status Bar ---
        self.status_label = QLabel("")
        layout.addWidget(self.status_label)

    def _create_tree_view(self, proxy):
        """Create a configured tree view backed by the given proxy model"""
        tree = QTreeView(self)
        tree.setModel(proxy)
        tree.setEditTriggers(QAbstractItemView.DoubleClicked | QAbstractItemView.SelectedClicked)
        tree.setSelectionBehavior(QAbstractItemView.SelectRows)
        tree.setSelectionMode(QAbstractItemView.ExtendedSelection)
        tree.setAlternatingRowColors(True)
        tree.setRootIsDecorated(True)
        tree.setUniformRowHeights(True)

        # Make columns resizable
        tree.header().setSectionResizeMode(QHeaderView.Interactive)
        tree.header().setDefaultSectionSize(100)
        tree.header().resizeSection(0, 40)    # Star column
        tree.header().resizeSection(1, 500)   # Name column - wider for long project names
        tree.header().resizeSection(2, 100)   # Type column (hidden)
        tree.header().resizeSection(3, 100)   # Last Modified column (hidden)
        tree.header().resizeSection(4, 400)   # Path column - wider for long paths

        # Hide Type and Last Modified columns
        tree.setColumnHidden(2, True)
        tree.setColumnHidden(3, True)

        # Allow last visible column to stretch
        tree.header().setStretchLastSection(True)

        # Connect signals with closures binding this tree and its proxy,
        # so every handler acts on the tree the signal originated from
        tree.doubleClicked.connect(
            lambda index, t=tree: self._on_project_activated(index))
        tree.setContextMenuPolicy(Qt.CustomContextMenu)
        tree.customContextMenuRequested.connect(
            lambda pos, t=tree: self._show_context_menu(t, pos))
        tree.selectionModel().selectionChanged.connect(
            lambda selected, deselected, t=tree: self._on_tree_selection_changed(t, selected, deselected))

        # Attach FavoriteStarDelegate to star column
        star_delegate = FavoriteStarDelegate(self.favorites_manager, tree)
        tree.setItemDelegateForColumn(0, star_delegate)

        return tree

    def _on_tab_changed(self, index):
        """Handle tab change to filter projects"""
        if index == 0:
            # All Projects - show all
            self.show_favorites_only = False
            self.show_recents_only = False
            self._filter_projects()
        elif index == 1:
            # Favorites - show only favorites
            self.show_favorites_only = True
            self.show_recents_only = False
            self._filter_projects()
        elif index == 2:
            # Recents - show recently accessed projects
            self.show_favorites_only = False
            self.show_recents_only = True
            self._filter_projects()

    def _on_rescan_files_clicked(self):
        """Perform a full scan of the currently selected project for files"""
        tree = self._active_tree()
        selection = tree.selectionModel().selectedRows()
        if not selection:
            self.status_label.setText("No project selected to scan.")
            return
            
        src_index = self._map_to_source(selection[0])
        # Always use the last column for path
        project_path = self.model.item(src_index.row(), self.model.columnCount() - 1).text()
        
        # Verify project path exists
        if not os.path.isdir(project_path):
            from PySide6.QtWidgets import QMessageBox
            QMessageBox.warning(self, "Invalid Project Path", f"The project path does not exist: {project_path}")
            return
            
        # Set status before starting scan
        self.status_label.setText(f"Scanning project at {project_path}...")
        
        # Start full scan (both folder matching and file scanning)
        self._scan_project_files_async(project_path)


    def _load_projects_async(self):
        """
        Asynchronously load projects using ProjectLoadWorker and QThreadPool.
        Shows a loading indicator while running, updates UI on completion.
        """
        self.loading_bar.setVisible(True)
        self.status_label.setText("Loading projects...")
        worker = ProjectLoadWorker(self.db_path)
        worker.signals.finished.connect(self._on_projects_loaded)
        worker.signals.error.connect(self._on_projects_load_error)
        self.threadpool.start(worker)

    # _fetch_projects is now handled by async worker

    def _on_projects_loaded(self, projects):
        """
        Called when async project loading finishes successfully.
        """
        self.loading_bar.setVisible(False)
        self._populate_projects(projects)

    def _on_projects_load_error(self, exc_info):
        """
        Called if async project loading fails.
        """
        self.loading_bar.setVisible(False)
        exctype, value, tb = exc_info
        self.status_label.setText(f"Error loading projects: {value}")

    def _populate_projects(self, projects):
        logger.debug(f"[DIAG] _populate_projects projects={projects}")
        self.model.removeRows(0, self.model.rowCount())
        for proj in projects:
            logger.debug(f"[DIAG] proj={proj}")
            star_item = QStandardItem(str(proj["id"]))
            star_item.setEditable(False)
            star_item.setData(proj["id"], Qt.UserRole + 2)  # Project ID for delegate
            name_item = QStandardItem(proj["name"])
            name_item.setData(False, Qt.UserRole + 1)  # Not a file
            name_item.setData(proj["id"], Qt.UserRole + 2)
            name_item.setData(proj["path"], Qt.UserRole + 3)
            type_item = QStandardItem(str(proj.get("type", "Unknown")))
            lastmod_item = QStandardItem(str(proj.get("last_modified", "N/A")))
            path_item = QStandardItem(proj["path"])
            self.model.appendRow([star_item, name_item, type_item, lastmod_item, path_item])
        # Log all items in the model for validation
        for row in range(self.model.rowCount()):
            name_item = self.model.item(row, 1)
            logger.debug(f"[DIAG] Model row {row}: name={name_item.text()}, type={type(name_item)}, id={name_item.data(Qt.UserRole + 2)}, path={name_item.data(Qt.UserRole + 3)}")
        # Log current selection after population
        if self.tree and self.tree.selectionModel():
            logger.debug(f"[DIAG] Current selected rows after population: {self.tree.selectionModel().selectedRows()}")
            # Programmatically select the first row if available, to ensure selection works
            if self.model.rowCount() > 0:
                ix = self.proxy_model.index(0, 1)
                from PySide6.QtCore import QItemSelectionModel
                if ix.isValid():
                    self.tree.selectionModel().select(ix, QItemSelectionModel.Select | QItemSelectionModel.Rows)
                    logger.debug(f"[DIAG] Programmatically selected row 0: {self.tree.selectionModel().selectedRows()} (index: {ix})")
                else:
                    logger.error(f"[DIAG] Programmatic selection index is invalid: {ix}")
        self.status_label.setText(f"{len(projects)} project(s) loaded.")
        self.tree.expandAll()
        logger.debug(f"[DIAG] _populate_projects: tree={self.tree}, model={self.model}, proxy_model={self.proxy_model}")

        # Reset horizontal scroll to left
        self.tree.horizontalScrollBar().setValue(0)

    def _normalize_project_ids(self, values):
        """
        Normalize project IDs to a set of ints.

        Favorites/recents persist IDs as JSON strings and may contain legacy
        non-numeric entries (e.g. 'None'); invalid values are skipped.
        """
        ids = set()
        for value in values:
            try:
                ids.add(int(value))
            except (TypeError, ValueError):
                logger.warning(f"[ProjectBrowser] Skipping invalid project ID: {value!r}")
        return ids

    def _filter_projects(self):
        """
        Update each tree's own proxy. Changing tabs or typing in the search
        box never alters another tree's filtering semantics:
        - All Projects: search regex on the name column (DisplayRole)
        - Favorites/Recents: allowed project-ID set + name filter
        """
        text = self.search_input.text()

        # All Projects tab: regex text search on project name
        self.proxy_model.setFilterRole(Qt.DisplayRole)
        self.proxy_model.setFilterKeyColumn(1)  # Name column
        self.proxy_model.setFilterRegularExpression(text)

        # Favorites tab: allowed ID set (+ name filter)
        fav_ids = self._normalize_project_ids(self.favorites_manager.get_favorites())
        logger.debug(f"[ProjectBrowser] Filtering favorites to IDs: {fav_ids}")
        self.favorites_proxy.set_allowed_ids(fav_ids)
        self.favorites_proxy.set_name_filter(text)

        # Recents tab: refresh the recent ID set from the DB while the tab is active
        recent_count = 0
        if self.show_recents_only:
            try:
                from core.database import DatabaseManager
                db = DatabaseManager(self.db_path)
                recent_projects = db.get_recent_projects(self.user_data.get("id", 1), limit=20)
                recent_ids = self._normalize_project_ids(p["id"] for p in recent_projects)
            except Exception as e:
                logger.error(f"[ProjectBrowser] Failed to load recent projects: {e}")
                recent_ids = set()
            recent_count = len(recent_ids)
            logger.debug(f"[ProjectBrowser] Filtering recents to IDs: {recent_ids}")
            self.recents_proxy.set_allowed_ids(recent_ids)
            self.recents_proxy.set_name_filter(text)

        if hasattr(self, 'status_label'):
            if self.show_recents_only:
                self.status_label.setText(f"Showing {recent_count} recent projects")
            elif self.show_favorites_only:
                self.status_label.setText("Showing favorites only")
            elif text:
                self.status_label.setText(f"Filter: '{text}'")
            else:
                self.status_label.setText("Showing all projects")

    # Removed _on_scan_files_clicked as it's redundant with _on_rescan_files_clicked

    @staticmethod
    def _map_to_source(index):
        """Map a view index from any of the per-tree proxies to the source model (identity if already a source index)."""
        model = index.model()
        if isinstance(model, QSortFilterProxyModel):
            return model.mapToSource(index)
        return index

    def _project_from_view_index(self, index):
        """Build the project dict for the source row behind a proxy index from any tree."""
        src_index = self._map_to_source(index)
        # Column 0: Star (contains ID as text)
        # Column 1: Name
        # Column 2: Type
        # Column 3: Last Modified
        # Column 4: Path
        return {
            "id": int(self.model.item(src_index.row(), 0).text()),
            "name": self.model.item(src_index.row(), 1).text(),
            "type": self.model.item(src_index.row(), 2).text(),
            "last_modified": self.model.item(src_index.row(), 3).text(),
            "path": self.model.item(src_index.row(), 4).text(),
        }

    def _active_tree(self):
        """Return the tree view of the currently active tab."""
        trees = (self.tree, self.favorites_tree, self.recents_tree)
        index = self.tab_widget.currentIndex()
        if 0 <= index < len(trees):
            return trees[index]
        return self.tree

    def _on_tree_selection_changed(self, tree, selected, deselected):
        # Emit project_selected for the first selected row (if any)
        indexes = tree.selectionModel().selectedRows()
        if not indexes:
            return
        proj = self._project_from_view_index(indexes[0])
        logger.debug(f"[DIAG] Emitting project_selected: {proj}")
        self.project_selected.emit(proj)
        if hasattr(self, 'status_label'):
            self.status_label.setText(f"Selected: {proj['name']}")

        # Track this as a recent project
        self._track_recent_project(proj["id"])

    def _on_project_activated(self, index: QModelIndex):
        """
        Handle double-click or activation of a project row.
        """
        proj = self._project_from_view_index(index)
        logger.debug(f"[DIAG] Emitting project_selected: {proj}")
        self.project_selected.emit(proj)
        if hasattr(self, 'status_label'):
            self.status_label.setText(f"Selected: {proj['name']}")
        # Start async scan for files in the selected project

    def _scan_project_files_async(self, project_path):
        """
        Asynchronously scan the project folder for .nk and .aep files and emit file_selected.
        Uses the new Scanner module for robust scanning with proper path normalization and DB integration.
        Shows a ScanProgressDialog with progress, ETA, and logs.
        """
        import os
        import logging
        from ui_slick.scan_progress_dialog import ScanManager
        from core.database import DatabaseManager
        
        # Validate project path
        if not project_path or not os.path.isdir(project_path):
            self.status_label.setText(f"Invalid project path: {project_path}")
            return
            
        logging.info(f"[ProjectBrowser] Starting scan for project path: {project_path}")
        
        # Create database manager for the scan
        db_manager = DatabaseManager(self.db_path)
        
        # Create scan manager and prepare for scan
        self._scan_manager = ScanManager(db_manager, self)
        self._scan_incremental_files = []
        
        # Define scan completion callback
        def on_scan_finished(scanned_project_path, found_files):
            logging.info(f"[ProjectBrowser] Scan complete for {scanned_project_path}. Found {len(found_files)} files")
            
            # Log a breakdown of files by type for diagnostics
            nuke_files = [f for f in found_files if f.get('filepath', '').lower().endswith('.nk')]
            aep_files = [f for f in found_files if f.get('filepath', '').lower().endswith('.aep')]
            logging.info(f"[ProjectBrowser] File breakdown: {len(nuke_files)} Nuke files, {len(aep_files)} AEP files")
            
            # Log some sample paths to help diagnose scanning issues
            if found_files:
                logging.info(f"[ProjectBrowser] Sample files found:\n" + 
                            '\n'.join([f['filepath'] for f in found_files[:5]]) + 
                            (f"\n...and {len(found_files)-5} more" if len(found_files) > 5 else ""))
            
            # Update UI
            self.status_label.setText(f"Found {len(found_files)} files.")
            
            # Emit scan_finished to update file browser from DB
            self.scan_finished.emit(scanned_project_path)
            
            # Also send incremental file update signals
            nuke_files_incr = [f for f in found_files if f.get('filepath', '').lower().endswith('.nk')]
            aep_files_incr = [f for f in found_files if f.get('filepath', '').lower().endswith('.aep')]
            self.file_selected.emit(nuke_files_incr, aep_files_incr)
        
        # Start the scan
        self._scan_manager.scan_project(project_path, on_scan_finished)

    def _on_open_containing_folder(self, index):
        """
        Open the folder containing the selected project in the system file explorer.
        """
        from PySide6.QtWidgets import QMessageBox
        # Map the index (from whichever tree/proxy it came) to the source model
        src_index = self._map_to_source(index)
        # Get the path column (always last column)
        path_item = self.model.item(src_index.row(), self.model.columnCount() - 1)

        if not path_item:
            QMessageBox.warning(self, "Open Folder", "No path found for this project.")
            return
        folder_path = path_item.text()
        if not folder_path or not os.path.isdir(folder_path):
            QMessageBox.warning(self, "Open Folder", f"Project folder does not exist:\n{folder_path}")
            return
        try:
            open_in_file_manager(folder_path)
        except Exception as e:
            QMessageBox.critical(self, "Open Folder", f"Failed to open folder:\n{e}")

    def _track_recent_project(self, project_id):
        """
        Track a project as recently accessed in the database.

        Args:
            project_id (int): Project ID to track
        """
        try:
            from core.database import DatabaseManager
            db = DatabaseManager(self.db_path)
            user_id = self.user_data.get("id", 1)
            db.add_recent_project(user_id, project_id)
            logger.debug(f"[ProjectBrowser] Tracked project {project_id} as recent for user {user_id}")
        except Exception as e:
            logger.error(f"[ProjectBrowser] Error tracking recent project: {str(e)}")

    def get_selected_project_id(self):
        """Get the currently selected project ID"""
        try:
            selection = self._active_tree().selectionModel().selectedRows()
            if selection:
                index = selection[0]
                src_index = self._map_to_source(index)
                project_id = int(self.model.item(src_index.row(), 0).text())
                return project_id
        except Exception as e:
            logger.error(f"Error getting selected project ID: {e}")
        return None

    def select_project_by_id(self, project_id):
        """Select a project by its ID"""
        try:
            # Convert to int if it's a string
            if isinstance(project_id, str):
                project_id = int(project_id)

            # Search through the model to find the project
            for row in range(self.model.rowCount()):
                item = self.model.item(row, 0)  # Star column has ID
                if item and int(item.text()) == project_id:
                    # Map to proxy model
                    source_index = self.model.index(row, 1)  # Name column
                    proxy_index = self.proxy_model.mapFromSource(source_index)

                    # Select the row
                    self.tree.selectionModel().select(
                        proxy_index,
                        QItemSelectionModel.ClearAndSelect | QItemSelectionModel.Rows
                    )

                    # Scroll to the selected item
                    self.tree.scrollTo(proxy_index)

                    logger.info(f"Selected project ID: {project_id}")
                    return True

            logger.warning(f"Project ID {project_id} not found in tree")
        except Exception as e:
            logger.error(f"Error selecting project by ID: {e}")
        return False

    def save_column_state(self):
        """Save the column widths and state"""
        try:
            header = self.tree.header()
            return header.saveState()
        except Exception as e:
            logger.error(f"Error saving column state: {e}")
            return None

    def restore_column_state(self, state):
        """Restore the column widths and state"""
        try:
            if state:
                header = self.tree.header()
                result = header.restoreState(state)
                logger.info(f"Project browser column state restored: {result}")

                # Ensure star column is always visible
                self.tree.setColumnHidden(0, False)

                # Ensure Type and Last Modified columns stay hidden
                self.tree.setColumnHidden(2, True)
                self.tree.setColumnHidden(3, True)

                # Reset horizontal scroll to left
                self.tree.horizontalScrollBar().setValue(0)
            else:
                logger.warning("No column state to restore for project browser")
        except Exception as e:
            logger.error(f"Error restoring column state: {e}")
