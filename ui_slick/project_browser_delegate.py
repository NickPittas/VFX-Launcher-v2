from PySide6.QtWidgets import QStyledItemDelegate, QApplication
from PySide6.QtGui import QIcon, QPixmap, QPainter
from PySide6.QtCore import Qt, QRect, QEvent
from PySide6.QtSvg import QSvgRenderer
import os
import logging

logger = logging.getLogger(__name__)

class FavoriteStarDelegate(QStyledItemDelegate):
    """
    Custom delegate to draw a star icon for favorite projects in the tree view.
    """
    def __init__(self, favorites_manager, parent=None):
        super().__init__(parent)
        self.favorites_manager = favorites_manager

        # Use SVG icons from icons folder
        star_filled_path = os.path.join(os.path.dirname(__file__), 'icons', 'star_icon.svg')
        star_outline_path = os.path.join(os.path.dirname(__file__), 'icons', 'star_outline_icon.svg')

        # Load filled star icon
        if os.path.exists(star_filled_path):
            self.star_icon = QIcon(star_filled_path)
            logger.info(f"Loaded filled star icon from: {star_filled_path}")
        else:
            logger.warning(f"Star icon not found: {star_filled_path}")
            self.star_icon = QIcon()

        # Load outline star icon
        if os.path.exists(star_outline_path):
            self.star_outline_icon = QIcon(star_outline_path)
            logger.info(f"Loaded outline star icon from: {star_outline_path}")
        else:
            logger.warning(f"Star outline icon not found: {star_outline_path}")
            self.star_outline_icon = QIcon()

    def paint(self, painter, option, index):
        if index.column() == 0:
            # Get project ID from UserRole
            project_id = index.data(Qt.UserRole + 2)
            if project_id is None:
                # Fallback: try to get from display text
                project_id = index.data(Qt.DisplayRole)

            # Check if this project is a favorite
            is_fav = self.favorites_manager.is_favorite(str(project_id))

            # Choose the appropriate icon
            icon = self.star_icon if is_fav else self.star_outline_icon

            # Only draw if icon is not null
            if not icon.isNull():
                # Draw the star icon centered in the cell
                rect = option.rect
                icon_size = min(rect.height() - 4, 20)  # Leave some padding
                x = rect.left() + (rect.width() - icon_size) // 2  # Center horizontally
                y = rect.top() + (rect.height() - icon_size) // 2  # Center vertically
                icon.paint(painter, QRect(x, y, icon_size, icon_size))
            else:
                logger.warning(f"Star icon is null for project {project_id}, is_fav={is_fav}")
        else:
            super().paint(painter, option, index)

    def editorEvent(self, event, model, option, index):
        if index.column() != 0:
            return False
        # Only handle mouse click
        if event.type() == QEvent.MouseButtonRelease and event.button() == Qt.LeftButton:
            project_id = index.data(Qt.UserRole + 2)
            if project_id is None:
                logger.warning("Toggle favorite skipped: project id is None for clicked index")
                return False
            project_id = str(project_id)
            if self.favorites_manager.is_favorite(project_id):
                self.favorites_manager.remove_favorite(project_id)
            else:
                self.favorites_manager.add_favorite(project_id)
            model.dataChanged.emit(index, index)
            return True
        return False
