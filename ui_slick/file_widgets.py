"""
File Widgets Module - Reusable UI components for file browser
"""
import logging
from PySide6.QtWidgets import (
    QWidget, QHBoxLayout, QLabel, QComboBox, 
    QTreeWidgetItem, QSizePolicy
)
from PySide6.QtCore import Qt

logger = logging.getLogger(__name__)

class VersionedFileWidget:
    """Creates and manages widgets for versioned files"""
    
    @staticmethod
    def create_version_badge(version_text):
        """Create a styled version badge"""
        version_badge = QLabel(version_text)
        version_badge.setObjectName("VersionBadge")
        version_badge.setStyleSheet("background:#2563eb;color:#fff;border-radius:7px;padding:2px 8px;font-size:13px;font-weight:bold;")
        version_badge.setToolTip(f"Current version: {version_text}")
        return version_badge
    
    @staticmethod
    def create_version_selector(versions, base_name):
        """Create a version selector combo box"""
        combo = QComboBox()
        combo.addItems(versions)
        combo.setToolTip(f"Select version for {base_name}")
        
        # Set a reasonable minimum height to prevent text from being cut off
        combo.setMinimumHeight(26)
        
        # Style the combo box for dark theme with increased padding and item height
        combo.setStyleSheet("""
            QComboBox {
                background-color: #333333;
                color: #f0f0f0;
                border: 1px solid #444444;
                border-radius: 4px;
                padding: 4px 8px;  /* Increased padding */
                min-width: 70px;
                min-height: 26px;  /* Minimum height to prevent text clipping */
            }
            QComboBox:hover {
                border: 1px solid #555555;
            }
            QComboBox::drop-down {
                subcontrol-origin: padding;
                subcontrol-position: top right;
                width: 20px;
                border-left: 1px solid #444444;
            }
            QComboBox QAbstractItemView {
                background-color: #333333;
                color: #f0f0f0;
                selection-background-color: #2563eb;
                selection-color: #ffffff;
                border: 1px solid #444444;
                padding: 4px 0px;  /* Add padding to dropdown items */
            }
            QComboBox QAbstractItemView::item {
                min-height: 24px;  /* Ensure dropdown items are tall enough */
                padding: 4px 8px;  /* Add padding to individual items */
            }
        """)
        return combo
    
    @staticmethod
    def create_file_widget(base_name, version_numbers, file_item, tree_widget, file_icon=None):
        """Create a widget for versioned file with name, dropdown and badge"""
        try:
            # Create custom widget for the file item
            file_widget = QWidget()

            # Use QHBoxLayout with ample spacing
            file_layout = QHBoxLayout(file_widget)
            file_layout.setContentsMargins(5, 6, 5, 6)  # Generous margins for better spacing
            file_layout.setSpacing(8)  # Spacing between elements

            # Add icon if provided
            if file_icon and not file_icon.isNull():
                icon_label = QLabel()
                icon_pixmap = file_icon.pixmap(16, 16)  # 16x16 icon size
                icon_label.setPixmap(icon_pixmap)
                icon_label.setFixedSize(16, 16)
                file_layout.addWidget(icon_label)

            # Create a fixed-width QLabel with careful text handling
            # Using a custom approach for maximum text clarity
            name_label = QLabel()

            # Display the base name normally without inserting spaces between characters
            name_label.setText(base_name)
            name_label.setTextFormat(Qt.PlainText)  # Crucial for proper text display
            
            # Set properties for best text display
            name_label.setAlignment(Qt.AlignLeft | Qt.AlignVCenter)
            name_label.setMinimumWidth(250)  # Give plenty of space for the text
            name_label.setMaximumWidth(400)  # But limit maximum width
            name_label.setSizePolicy(QSizePolicy.Preferred, QSizePolicy.Fixed)
            name_label.setFixedHeight(22)  # Fixed height prevents vertical sizing issues
            name_label.setWordWrap(False)  # Critical - prevent any word wrapping
            
            # Apply styling with standard font and normal character spacing
            name_label.setStyleSheet("""
                font-family: 'Segoe UI', Arial, sans-serif;
                font-size: 12px;
                font-weight: normal;
                color: #f0f0f0;
                letter-spacing: normal;
                word-spacing: normal;
                background-color: transparent;
                padding: 2px 5px;
                border: none;
            """)
            
            # Add the label to the layout
            file_layout.addWidget(name_label)
            
            # Add spacer to push version controls to the right
            file_layout.addStretch(1)
            
            # Get the highest version number (last in the sorted list)
            highest_version = version_numbers[-1] if version_numbers else "1"
            
            # Version badge - comes before combo box, initialized with highest version
            version_badge = VersionedFileWidget.create_version_badge(highest_version)
            file_layout.addWidget(version_badge)
            
            # Combo box for versions - fixed width for consistency, positioned at far right
            combo = VersionedFileWidget.create_version_selector(version_numbers, base_name)
            combo.setFixedWidth(80)  # Consistent width for all version selectors
            
            # Set the highest version as the default selected item
            combo.setCurrentIndex(len(version_numbers) - 1 if version_numbers else 0)
            
            file_layout.addWidget(combo)
            
            # Connect version change event
            combo.currentIndexChanged.connect(
                VersionedFileWidget.make_on_version_change(version_badge, combo)
            )
            
            # Add stretch at the end to push everything to the left
            file_layout.addStretch(1)
            
            # Set item height for better visibility
            file_item.setSizeHint(0, file_widget.sizeHint())
            
            # Set the custom widget for the tree item
            tree_widget.setItemWidget(file_item, 0, file_widget)
            return file_widget
            
        except Exception as e:
            logger.error(f"[FileWidgets] Error creating file widget for {base_name}: {str(e)}")
            return None
    
    @staticmethod
    def make_on_version_change(badge, combo):
        """Create a callback function for version changes"""
        def on_version_change(idx):
            badge.setText(combo.currentText())
            badge.setToolTip(f"Current version: {combo.currentText()}")
        return on_version_change
